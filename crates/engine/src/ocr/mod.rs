//! Handwritten mathematics to LaTeX, by asking a service the owner has chosen.
//!
//! It is off until it is turned on, and while it is off nothing here makes a request: the
//! check is in [`Session::math_ocr_job`] and again in [`recognize`], not only in the window.
//!
//! The work is in two halves so the window never waits on the network. The first, on the
//! main thread, turns strokes into a picture and gathers the settings and the key into an
//! [`OcrJob`]. The second, [`recognize`], takes only the job and an HTTP client, so it can
//! run on a worker thread.
//!
//! A key belongs to one service at one address. Each service keeps its own, and a key is
//! forgotten when the address it was given for changes, so it is never sent anywhere else.
//! Keys travel in headers, never in an address, and no error text repeats a key or a reply.

pub(crate) mod commands;
pub mod local_model;
mod provider;
pub mod raster;
pub mod vision_post;

pub use vision_post::{VisionObservation, observations_to_latex, observations_to_latex_with_aspect, observations_to_text};

use crate::Session;
use crate::sync::{SyncHttpClient, is_loopback_http};
use nocturne_ink::Stroke;
use nocturne_store::Settings;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

pub const MATH_OCR_ENABLED_KEY: &str = "math_ocr_enabled";
pub const HANDWRITING_OCR_ENABLED_KEY: &str = "handwriting_ocr_enabled";
const PROVIDER_KEY: &str = "ocr_provider";
const ENDPOINT_KEY: &str = "ocr_endpoint";
const MODEL_KEY: &str = "ocr_model";
const MATHPIX_APP_ID_SECRET: &str = "ocr_app_id_mathpix";

/// What kind of handwritten content OCR should recognize.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OcrMode {
    #[default]
    MathLatex,
    HandwritingText,
}

impl OcrMode {
    /// Stable identifier used in commands and settings (`"math"` or `"handwriting"`).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            OcrMode::MathLatex => "math",
            OcrMode::HandwritingText => "handwriting",
        }
    }

    /// Parses `"math"` / `"latex"` or `"handwriting"` / `"text"` (case-insensitive).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "math" | "latex" => Some(OcrMode::MathLatex),
            "handwriting" | "text" => Some(OcrMode::HandwritingText),
            _ => None,
        }
    }
}

/// Platform-native on-device OCR backend (for example Apple Vision `VNRecognizeTextRequest` on
/// iOS/iPadOS/macOS).
pub trait NativeOcrBackend: Send + Sync {
    /// Returns `true` when this native backend is available at runtime.
    fn is_supported(&self) -> bool {
        true
    }

    /// Runs on-device recognition on PNG image bytes and returns bounding-box observations in
    /// top-left normalized `[0.0, 1.0]` coordinates.
    fn recognize_png(&self, png_bytes: &[u8]) -> Result<Vec<VisionObservation>, OcrError>;

    /// Runs on-device recognition tuned for `mode` (for example disabling language correction for
    /// `OcrMode::MathLatex` and enabling it for `OcrMode::HandwritingText`).
    ///
    /// Defaults to [`Self::recognize_png`] so existing implementations keep compiling.
    fn recognize_png_with_mode(&self, png_bytes: &[u8], _mode: OcrMode) -> Result<Vec<VisionObservation>, OcrError> {
        self.recognize_png(png_bytes)
    }
}

/// Fallback native OCR backend used when no platform implementation is installed.
pub struct UnsupportedNativeOcrBackend;

impl NativeOcrBackend for UnsupportedNativeOcrBackend {
    fn is_supported(&self) -> bool {
        false
    }

    fn recognize_png(&self, _png_bytes: &[u8]) -> Result<Vec<VisionObservation>, OcrError> {
        Err(OcrError::Unsupported)
    }
}

/// Who is asked to read the handwriting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrProvider {
    /// Built-in Apple Vision on-device recognition (iOS / iPadOS / macOS).
    AppleVision,
    /// Built-in downloadable on-device Pix2Text MFR 1.5 ONNX math model (MIT, all non-WASM platforms).
    LocalModel,
    Anthropic,
    Gemini,
    OpenAi,
    Mathpix,
    /// Any server that speaks the chat-completions protocol, such as one on this machine.
    Custom,
}

#[allow(clippy::derivable_impls)]
impl Default for OcrProvider {
    fn default() -> Self {
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            Self::AppleVision
        }
        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        {
            Self::Anthropic
        }
    }
}

impl OcrProvider {
    pub const ALL: [OcrProvider; 7] = [
        OcrProvider::AppleVision,
        OcrProvider::LocalModel,
        OcrProvider::Anthropic,
        OcrProvider::Gemini,
        OcrProvider::OpenAi,
        OcrProvider::Mathpix,
        OcrProvider::Custom,
    ];

    pub fn id(self) -> &'static str {
        match self {
            OcrProvider::AppleVision => "apple_vision",
            OcrProvider::LocalModel => "local_model",
            OcrProvider::Anthropic => "anthropic",
            OcrProvider::Gemini => "gemini",
            OcrProvider::OpenAi => "openai",
            OcrProvider::Mathpix => "mathpix",
            OcrProvider::Custom => "custom",
        }
    }

    pub fn parse(text: &str) -> Option<OcrProvider> {
        match text.trim().to_ascii_lowercase().as_str() {
            "apple_vision" | "apple-vision" | "vision" => Some(OcrProvider::AppleVision),
            "local_model" | "local-model" | "local" | "pix2text" => Some(OcrProvider::LocalModel),
            other => OcrProvider::ALL.into_iter().find(|p| p.id() == other),
        }
    }

    /// True when recognition runs entirely on-device without sending images over the network.
    #[must_use]
    pub fn is_on_device(self) -> bool {
        matches!(self, OcrProvider::AppleVision | OcrProvider::LocalModel)
    }

    /// True when this provider requires an API key in the secret store.
    #[must_use]
    pub fn requires_api_key(self) -> bool {
        matches!(self, OcrProvider::Anthropic | OcrProvider::Gemini | OcrProvider::OpenAi | OcrProvider::Mathpix)
    }

    /// Where the service lives when no other address is given. On-device and custom providers have none.
    pub fn default_endpoint(self) -> &'static str {
        match self {
            OcrProvider::AppleVision | OcrProvider::LocalModel => "",
            OcrProvider::Anthropic => "https://api.anthropic.com",
            OcrProvider::Gemini => "https://generativelanguage.googleapis.com",
            OcrProvider::OpenAi => "https://api.openai.com/v1",
            OcrProvider::Mathpix => "https://api.mathpix.com",
            OcrProvider::Custom => "",
        }
    }

    /// The model asked when none is named. Mathpix, Apple Vision, and custom servers have none.
    pub fn default_model(self) -> &'static str {
        match self {
            OcrProvider::LocalModel => local_model::MODEL_ID,
            OcrProvider::Anthropic => "claude-opus-5-5",
            OcrProvider::Gemini => "gemini-4-hydrogen",
            OcrProvider::OpenAi => "gpt-6.2-astra",
            OcrProvider::AppleVision | OcrProvider::Mathpix | OcrProvider::Custom => "",
        }
    }

    /// The name its key is kept under. One per service, so keys are never shared.
    pub fn secret_key(self) -> String {
        format!("ocr_api_key_{}", self.id())
    }

    /// Settings key storing the exact endpoint URL to which this provider's saved secret is bound.
    pub(crate) fn key_endpoint_setting_key(self) -> String {
        format!("ocr_key_endpoint_{}", self.id())
    }
}

/// Returns `true` when the saved secret for `provider` is either unbound (legacy/direct store) or
/// explicitly bound to `effective_endpoint`.
pub(crate) fn key_matches_endpoint(settings: &Settings, provider: OcrProvider, effective_endpoint: &str) -> bool {
    match settings.get(&provider.key_endpoint_setting_key()).map(|v| v.trim().to_string()) {
        Some(bound) if !bound.is_empty() => bound == effective_endpoint,
        _ => true,
    }
}

/// What the settings file says. `endpoint` and `model` are empty for the service's own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OcrConfig {
    pub enabled: bool,
    pub handwriting_enabled: bool,
    pub provider: OcrProvider,
    pub endpoint: String,
    pub model: String,
}

impl OcrConfig {
    pub fn load(settings: &Settings) -> OcrConfig {
        let text = |key: &str| settings.get(key).map(|v| v.trim().to_string()).unwrap_or_default();
        let endpoint = text(ENDPOINT_KEY);
        OcrConfig {
            enabled: text(MATH_OCR_ENABLED_KEY) == "true",
            handwriting_enabled: text(HANDWRITING_OCR_ENABLED_KEY) == "true",
            provider: OcrProvider::parse(&text(PROVIDER_KEY)).unwrap_or_default(),
            // A settings file that was meddled with cannot point a key at an unsafe address.
            endpoint: if endpoint_is_safe(&endpoint) { endpoint } else { String::new() },
            model: text(MODEL_KEY),
        }
    }

    pub(crate) fn save(&self, settings: &Settings) -> Result<(), nocturne_store::StoreError> {
        settings.set(MATH_OCR_ENABLED_KEY, if self.enabled { "true" } else { "false" })?;
        settings.set(HANDWRITING_OCR_ENABLED_KEY, if self.handwriting_enabled { "true" } else { "false" })?;
        settings.set(PROVIDER_KEY, self.provider.id())?;
        settings.set(ENDPOINT_KEY, &self.endpoint)?;
        settings.set(MODEL_KEY, &self.model)
    }

    pub fn effective_endpoint(&self) -> &str {
        if self.provider.is_on_device() {
            ""
        } else if self.endpoint.is_empty() {
            self.provider.default_endpoint()
        } else {
            &self.endpoint
        }
    }

    pub fn effective_model(&self) -> &str {
        if self.model.is_empty() { self.provider.default_model() } else { &self.model }
    }
}

/// True for `https://host...` and for plain `http` to this machine only.
pub fn endpoint_is_safe(url: &str) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    match url.strip_prefix("https://") {
        Some(rest) => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            !authority.is_empty() && !authority.contains('@')
        }
        None => is_loopback_http(url),
    }
}

/// Why there is no recognition result. None of these carries a key or the text of a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OcrError {
    /// Recognition is switched off.
    Disabled,
    /// Something the service needs has not been given; the word says what.
    NotConfigured(&'static str),
    /// This build cannot make requests or keep keys.
    Unsupported,
    /// There were no strokes to send.
    NoInk,
    /// The picture could not be made.
    Picture,
    /// The service did not accept the key.
    Auth,
    RateLimited,
    /// The service declined to answer.
    Refused,
    /// The answer was cut off before any output.
    Truncated,
    /// Another HTTP status.
    Status(u16),
    /// The request did not get there or back.
    Transport(String),
    /// The reply had no usable text or LaTeX in it.
    BadReply,
}

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OcrError::Disabled => f.write_str("equation recognition is turned off"),
            OcrError::NotConfigured(what) => write!(f, "equation recognition needs {what} first"),
            OcrError::Unsupported => f.write_str("equation recognition is not available in this build"),
            OcrError::NoInk => f.write_str("there is no ink to read"),
            OcrError::Picture => f.write_str("the ink could not be made into a picture"),
            OcrError::Auth => f.write_str("the recognition service did not accept the key"),
            OcrError::RateLimited => f.write_str("the recognition service is busy; try again shortly"),
            OcrError::Refused => f.write_str("the recognition service declined to read this"),
            OcrError::Truncated => f.write_str("the recognition service's answer was cut off"),
            OcrError::Status(code) => write!(f, "the recognition service answered with status {code}"),
            OcrError::Transport(why) => write!(f, "the recognition service could not be reached: {why}"),
            OcrError::BadReply => f.write_str("the recognition service sent no LaTeX"),
        }
    }
}

impl std::error::Error for OcrError {}

/// Everything one request needs, and nothing that borrows from the session.
#[derive(Clone)]
pub struct OcrJob {
    pub enabled: bool,
    pub mode: OcrMode,
    pub provider: OcrProvider,
    pub endpoint: String,
    pub model: String,
    pub key: String,
    pub app_id: String,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub native_ocr: Option<Arc<dyn NativeOcrBackend>>,
    pub model_dir: Option<PathBuf>,
}

impl OcrJob {
    /// Executes this OCR job on the current thread (typically a worker thread).
    pub fn execute(&self, http: &dyn SyncHttpClient) -> Result<String, OcrError> {
        recognize(http, self)
    }
}

impl fmt::Debug for OcrJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OcrJob")
            .field("enabled", &self.enabled)
            .field("mode", &self.mode)
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("has_key", &!self.key.is_empty())
            .field("png_bytes", &self.png.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("has_native_ocr", &self.native_ocr.is_some())
            .field("has_model_dir", &self.model_dir.is_some())
            .finish()
    }
}

impl Session {
    pub fn ocr_config(&self) -> OcrConfig {
        OcrConfig::load(&self.settings)
    }

    /// Storage directory for the downloadable on-device Pix2Text MFR 1.5 math model.
    #[must_use]
    pub fn ocr_model_dir(&self) -> PathBuf {
        local_model::default_model_dir(&self.settings)
    }

    /// Turns equation recognition on or off. False if the settings file could not be written.
    pub fn set_math_ocr_enabled(&mut self, enabled: bool) -> bool {
        self.settings.set(MATH_OCR_ENABLED_KEY, if enabled { "true" } else { "false" }).is_ok()
    }

    /// Turns general handwriting-to-text recognition on or off. False if the settings file could not be written.
    pub fn set_handwriting_ocr_enabled(&mut self, enabled: bool) -> bool {
        self.settings.set(HANDWRITING_OCR_ENABLED_KEY, if enabled { "true" } else { "false" }).is_ok()
    }

    /// True if `provider` has a saved API key valid for its effective endpoint.
    #[must_use]
    pub fn ocr_has_key(&self, provider: OcrProvider) -> bool {
        if provider.is_on_device() {
            return false;
        }
        let cfg = self.ocr_config();
        let ep = if cfg.provider == provider { cfg.effective_endpoint() } else { provider.default_endpoint() };
        if cfg.provider == provider && !key_matches_endpoint(&self.settings, provider, ep) {
            return false;
        }
        self.sync_secrets().get_secret(&provider.secret_key()).ok().flatten().is_some()
    }

    /// True if Mathpix has a saved `app_id` valid for its effective endpoint.
    #[must_use]
    pub fn ocr_has_mathpix_app_id(&self) -> bool {
        let cfg = self.ocr_config();
        if cfg.provider == OcrProvider::Mathpix && !key_matches_endpoint(&self.settings, OcrProvider::Mathpix, cfg.effective_endpoint()) {
            return false;
        }
        self.sync_secrets().get_secret(MATHPIX_APP_ID_SECRET).ok().flatten().is_some()
    }

    /// True when `provider` has every credential it requires (for Mathpix: BOTH `app_key` and `app_id`).
    #[must_use]
    pub fn ocr_credentials_complete(&self, provider: OcrProvider) -> bool {
        match provider {
            OcrProvider::AppleVision | OcrProvider::LocalModel | OcrProvider::Custom => true,
            OcrProvider::Mathpix => self.ocr_has_key(OcrProvider::Mathpix) && self.ocr_has_mathpix_app_id(),
            OcrProvider::Anthropic | OcrProvider::Gemini | OcrProvider::OpenAi => self.ocr_has_key(provider),
        }
    }

    /// True if the chosen service has all it needs for math OCR: an address, a model, and its key (or on-device readiness).
    pub fn math_ocr_ready(&self) -> bool {
        self.ocr_parts_for_mode(OcrMode::MathLatex).is_ok()
    }

    /// True if handwriting-to-text recognition is enabled and the chosen service (or native fallback
    /// when the math-only local model is selected) can handle general handwriting.
    pub fn handwriting_ocr_ready(&self) -> bool {
        self.ocr_parts_for_mode(OcrMode::HandwritingText).is_ok()
    }

    fn ocr_parts_for_mode(&self, mode: OcrMode) -> Result<(OcrConfig, String, String), OcrError> {
        let cfg = self.ocr_config();
        let enabled = match mode {
            OcrMode::MathLatex => cfg.enabled,
            OcrMode::HandwritingText => cfg.handwriting_enabled,
        };
        if !enabled {
            return Err(OcrError::Disabled);
        }
        match cfg.provider {
            OcrProvider::AppleVision => {
                if self.native_ocr_backend().is_some_and(|b| b.is_supported()) {
                    return Ok((cfg, String::new(), String::new()));
                }
                return Err(OcrError::Unsupported);
            }
            OcrProvider::LocalModel => {
                if mode == OcrMode::HandwritingText {
                    // The downloaded local Pix2Text MFR model is math-only; delegate to the
                    // platform-native recognizer (Apple Vision on iPadOS) when available.
                    if self.native_ocr_backend().is_some_and(|b| b.is_supported()) {
                        return Ok((cfg, String::new(), String::new()));
                    }
                    return Err(OcrError::Unsupported);
                }
                if !local_model::is_supported_on_platform() {
                    return Err(OcrError::Unsupported);
                }
                if !local_model::is_model_installed(&self.ocr_model_dir()) {
                    return Err(OcrError::NotConfigured("the local math model"));
                }
                return Ok((cfg, String::new(), String::new()));
            }
            _ => {}
        }
        if !self.sync_http().is_supported() {
            return Err(OcrError::Unsupported);
        }
        let endpoint = cfg.effective_endpoint();
        if endpoint.is_empty() {
            return Err(OcrError::NotConfigured("the service's address"));
        }
        if cfg.effective_model().is_empty() && cfg.provider != OcrProvider::Mathpix {
            return Err(OcrError::NotConfigured("the name of a model"));
        }
        let secrets = self.sync_secrets();
        let bound_ok = key_matches_endpoint(&self.settings, cfg.provider, endpoint);
        let secret = |name: &str| if bound_ok { secrets.get_secret(name).ok().flatten().unwrap_or_default() } else { String::new() };
        let key = secret(&cfg.provider.secret_key());
        if key.is_empty() && cfg.provider.requires_api_key() {
            return Err(OcrError::NotConfigured("a key"));
        }
        let app_id = if cfg.provider == OcrProvider::Mathpix { secret(MATHPIX_APP_ID_SECRET) } else { String::new() };
        if cfg.provider == OcrProvider::Mathpix && app_id.is_empty() {
            return Err(OcrError::NotConfigured("an app id"));
        }
        Ok((cfg, key, app_id))
    }

    /// The main-thread half: checks the switch and the settings for `mode`, and draws the picture.
    pub fn start_ocr_job(&self, strokes: &[Stroke], mode: OcrMode) -> Result<OcrJob, OcrError> {
        let (cfg, key, app_id) = self.ocr_parts_for_mode(mode)?;
        let rendered = raster::render_png(strokes)?;
        Ok(OcrJob {
            enabled: true,
            mode,
            provider: cfg.provider,
            endpoint: cfg.effective_endpoint().to_string(),
            model: cfg.effective_model().to_string(),
            key,
            app_id,
            png: rendered.png,
            width: rendered.width,
            height: rendered.height,
            native_ocr: self.native_ocr_backend(),
            model_dir: Some(self.ocr_model_dir()),
        })
    }

    /// Starts an OCR job in [`OcrMode::MathLatex`] mode.
    pub fn start_math_ocr_job(&self, strokes: &[Stroke]) -> Result<OcrJob, OcrError> {
        self.start_ocr_job(strokes, OcrMode::MathLatex)
    }

    /// The main-thread half for equation recognition (delegates to [`Self::start_ocr_job`]).
    pub fn math_ocr_job(&self, strokes: &[Stroke]) -> Result<OcrJob, OcrError> {
        self.start_ocr_job(strokes, OcrMode::MathLatex)
    }
}

fn recognize_native(job: &OcrJob) -> Result<String, OcrError> {
    let backend = job.native_ocr.as_deref().filter(|b| b.is_supported()).ok_or(OcrError::Unsupported)?;
    let observations = backend.recognize_png_with_mode(&job.png, job.mode)?;
    let output = match job.mode {
        OcrMode::MathLatex => {
            let aspect = if job.height > 0 { job.width as f32 / job.height as f32 } else { 1.0 };
            clean_latex(&vision_post::observations_to_latex_with_aspect(&observations, aspect))
        }
        OcrMode::HandwritingText => vision_post::observations_to_text(&observations),
    };
    if output.is_empty() { Err(OcrError::BadReply) } else { Ok(output) }
}

/// The worker half: runs on-device recognition or sends the picture and reads the output out of the reply.
pub fn recognize(http: &dyn SyncHttpClient, job: &OcrJob) -> Result<String, OcrError> {
    if !job.enabled {
        return Err(OcrError::Disabled);
    }
    match job.provider {
        OcrProvider::AppleVision => recognize_native(job),
        OcrProvider::LocalModel => {
            if job.mode == OcrMode::HandwritingText {
                return recognize_native(job);
            }
            if !local_model::is_supported_on_platform() {
                return Err(OcrError::Unsupported);
            }
            let dir = job.model_dir.as_deref().ok_or(OcrError::NotConfigured("the local math model"))?;
            if !local_model::is_model_installed(dir) {
                return Err(OcrError::NotConfigured("the local math model"));
            }
            let raw = local_model::recognize_png(dir, &job.png)?;
            let latex = clean_latex(&raw);
            if latex.is_empty() { Err(OcrError::BadReply) } else { Ok(latex) }
        }
        _ => {
            if !http.is_supported() {
                return Err(OcrError::Unsupported);
            }
            if !endpoint_is_safe(&job.endpoint) {
                return Err(OcrError::NotConfigured("the service's address"));
            }
            let req = provider::request(job)?;
            let reply = http.request(req).map_err(|e| OcrError::Transport(without_secrets(&e.to_string(), job)))?;
            match reply.status {
                200..=299 => provider::answer(job.provider, job.mode, &reply.body),
                401 | 403 => Err(OcrError::Auth),
                429 => Err(OcrError::RateLimited),
                other => Err(OcrError::Status(other)),
            }
        }
    }
}

fn without_secrets(text: &str, job: &OcrJob) -> String {
    let mut out = text.to_string();
    for secret in [&job.key, &job.app_id] {
        if !secret.is_empty() {
            out = out.replace(secret.as_str(), "***");
        }
    }
    out
}

/// One line of bare LaTeX from whatever a service wrapped it in: code fences, dollar signs
/// and `\[ \]` go, and line breaks become spaces, because a math block is one line.
pub fn clean_latex(raw: &str) -> String {
    let mut text = raw.trim();
    if let Some(rest) = text.strip_prefix("```") {
        let body = if let Some((_, after_nl)) = rest.split_once('\n') {
            after_nl
        } else {
            // Single-line fenced block: strip closing ``` and any leading language tag
            // (e.g. ```latex x^2```, ```tex x^2```, ```math x^2```, or ```x^2```).
            let inner = rest.trim_end().strip_suffix("```").unwrap_or(rest).trim();
            if matches!(inner.to_ascii_lowercase().as_str(), "latex" | "tex" | "math") {
                ""
            } else if let Some((tag, after_tag)) = inner.split_once(char::is_whitespace)
                && matches!(tag.to_ascii_lowercase().as_str(), "latex" | "tex" | "math")
            {
                after_tag
            } else {
                inner
            }
        };
        text = body.trim_end().strip_suffix("```").unwrap_or(body).trim();
    }
    loop {
        let inner = [("$$", "$$"), ("\\[", "\\]"), ("\\(", "\\)"), ("$", "$")].into_iter().find_map(|(open, close)| {
            let inner = text.strip_prefix(open)?.strip_suffix(close)?;
            // `$a$ and $b$` is not one formula in dollars; leave it as it is.
            let lone_dollar = inner.replace("\\$", "").contains('$');
            (!(open == "$" && lone_dollar)).then_some(inner.trim())
        });
        match inner {
            Some(inner) => text = inner,
            None => break,
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Standard base64 (RFC 4648) with padding.
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(T[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EngineError, Output, Params};
    use nocturne_ink::{Dab, Rgb, Tool};
    use nocturne_store::testing::TempDir;
    use serde_json::json;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (input, want) in
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(base64(input.as_bytes()), want);
        }
    }

    #[test]
    fn only_https_and_this_machine_are_safe_addresses() {
        for ok in ["https://api.example.test", "https://h/v1", "http://127.0.0.1:8080", "http://localhost:1/v1"] {
            assert!(endpoint_is_safe(ok), "{ok}");
        }
        for bad in ["", "https://", "https://user@h", "http://example.test", "https://h/ v1", "http://127.0.0.1.evil.test", "file:///etc"] {
            assert!(!endpoint_is_safe(bad), "{bad}");
        }
    }

    #[test]
    fn ocr_mode_id_and_parse_round_trip() {
        assert_eq!(OcrMode::default(), OcrMode::MathLatex);
        assert_eq!(OcrMode::MathLatex.id(), "math");
        assert_eq!(OcrMode::HandwritingText.id(), "handwriting");
        assert_eq!(OcrMode::parse("math"), Some(OcrMode::MathLatex));
        assert_eq!(OcrMode::parse(" LaTeX "), Some(OcrMode::MathLatex));
        assert_eq!(OcrMode::parse("handwriting"), Some(OcrMode::HandwritingText));
        assert_eq!(OcrMode::parse("TEXT"), Some(OcrMode::HandwritingText));
        assert_eq!(OcrMode::parse("diagram"), None);
    }

    #[test]
    fn handwriting_ocr_toggle_command_and_config_persistence() {
        let t = TempDir::new("ocr-handwriting-toggle");
        let settings_path = t.path().join("settings").join("s.toml");
        let mut s = Session::new(&t.path().join("notebooks"), Settings::at(settings_path.clone())).expect("session");

        // Starts disabled by default.
        assert!(!s.ocr_config().handwriting_enabled);

        // Toggle with no params flips to true and persists.
        let out = s.run("ink.handwriting_ocr_toggle", &Params::new()).expect("toggle on");
        let Output::Json(val) = out else { panic!("expected json") };
        assert_eq!(val, json!({ "handwriting_ocr_enabled": true }));
        assert!(OcrConfig::load(&Settings::at(settings_path.clone())).handwriting_enabled);

        // Explicit false persists.
        let mut p = Params::new();
        p.insert("enabled".to_string(), "false".to_string());
        let out = s.run("ink.handwriting_ocr_toggle", &p).expect("toggle off");
        let Output::Json(val) = out else { panic!("expected json") };
        assert_eq!(val, json!({ "handwriting_ocr_enabled": false }));
        assert!(!OcrConfig::load(&Settings::at(settings_path.clone())).handwriting_enabled);

        // Invalid value is rejected.
        let mut bad = Params::new();
        bad.insert("enabled".to_string(), "maybe".to_string());
        assert!(matches!(s.run("ink.handwriting_ocr_toggle", &bad), Err(EngineError::BadParam(_))));

        // Direct Session setter also persists.
        assert!(s.set_handwriting_ocr_enabled(true));
        assert!(OcrConfig::load(&Settings::at(settings_path)).handwriting_enabled);
    }

    #[test]
    fn local_model_handwriting_delegates_to_native_backend_or_returns_unsupported() {
        struct ModeAwareNativeBackend;
        impl NativeOcrBackend for ModeAwareNativeBackend {
            fn recognize_png(&self, _png_bytes: &[u8]) -> Result<Vec<VisionObservation>, OcrError> {
                Ok(vec![VisionObservation::new("fallback", 0.1, 0.1, 0.2, 0.1, 0.99)])
            }

            fn recognize_png_with_mode(&self, _png_bytes: &[u8], mode: OcrMode) -> Result<Vec<VisionObservation>, OcrError> {
                match mode {
                    OcrMode::MathLatex => Ok(vec![VisionObservation::new("x", 0.1, 0.4, 0.1, 0.2, 0.99)]),
                    OcrMode::HandwritingText => Ok(vec![
                        VisionObservation::new("Hello", 0.05, 0.10, 0.20, 0.08, 0.99),
                        VisionObservation::new("notes", 0.30, 0.10, 0.20, 0.08, 0.99),
                    ]),
                }
            }
        }

        let t = TempDir::new("ocr-local-handwriting-fallback");
        let mut s = Session::new(&t.path().join("notebooks"), Settings::at(t.path().join("settings").join("s.toml"))).expect("session");
        let mut stroke = Stroke::new(Tool::Pen, Rgb::BLACK, 2.0);
        stroke.dabs = vec![Dab::new(10.0, 10.0, 1.0), Dab::new(80.0, 10.0, 1.0)].into();

        assert!(s.set_handwriting_ocr_enabled(true));
        let mut cfg_p = Params::new();
        cfg_p.insert("provider".to_string(), "local_model".to_string());
        s.run("ink.math_ocr_configure", &cfg_p).expect("configure local_model");

        // Without native backend: LocalModel cannot handle HandwritingText.
        assert!(matches!(s.start_ocr_job(&[stroke.clone()], OcrMode::HandwritingText), Err(OcrError::Unsupported)));

        // With native backend (e.g. Apple Vision on iPadOS): LocalModel delegates HandwritingText cleanly!
        s.set_native_ocr_backend(Arc::new(ModeAwareNativeBackend));
        let job = s.start_ocr_job(&[stroke], OcrMode::HandwritingText).expect("handwriting job via native fallback");
        assert_eq!(job.mode, OcrMode::HandwritingText);
        assert!(job.width > 0 && job.height > 0);
        let text = job.execute(&*s.sync_http()).expect("recognized text");
        assert_eq!(text, "Hello notes");
    }
}
