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
mod provider;
pub mod raster;

use crate::Session;
use crate::sync::{SyncHttpClient, is_loopback_http};
use nocturne_ink::Stroke;
use nocturne_store::Settings;
use std::fmt;

pub const MATH_OCR_ENABLED_KEY: &str = "math_ocr_enabled";
const PROVIDER_KEY: &str = "ocr_provider";
const ENDPOINT_KEY: &str = "ocr_endpoint";
const MODEL_KEY: &str = "ocr_model";
const MATHPIX_APP_ID_SECRET: &str = "ocr_app_id_mathpix";

/// Who is asked to read the handwriting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OcrProvider {
    #[default]
    Anthropic,
    Gemini,
    OpenAi,
    Mathpix,
    /// Any server that speaks the chat-completions protocol, such as one on this machine.
    Custom,
}

impl OcrProvider {
    pub const ALL: [OcrProvider; 5] = [OcrProvider::Anthropic, OcrProvider::Gemini, OcrProvider::OpenAi, OcrProvider::Mathpix, OcrProvider::Custom];

    pub fn id(self) -> &'static str {
        match self {
            OcrProvider::Anthropic => "anthropic",
            OcrProvider::Gemini => "gemini",
            OcrProvider::OpenAi => "openai",
            OcrProvider::Mathpix => "mathpix",
            OcrProvider::Custom => "custom",
        }
    }

    pub fn parse(text: &str) -> Option<OcrProvider> {
        OcrProvider::ALL.into_iter().find(|p| p.id().eq_ignore_ascii_case(text.trim()))
    }

    /// Where the service lives when no other address is given. A custom one has none.
    pub fn default_endpoint(self) -> &'static str {
        match self {
            OcrProvider::Anthropic => "https://api.anthropic.com",
            OcrProvider::Gemini => "https://generativelanguage.googleapis.com",
            OcrProvider::OpenAi => "https://api.openai.com/v1",
            OcrProvider::Mathpix => "https://api.mathpix.com",
            OcrProvider::Custom => "",
        }
    }

    /// The model asked when none is named. Mathpix has no models, and a custom server's
    /// are its own.
    pub fn default_model(self) -> &'static str {
        match self {
            OcrProvider::Anthropic => "claude-opus-5-5",
            OcrProvider::Gemini => "gemini-4-hydrogen",
            OcrProvider::OpenAi => "gpt-6.2-astra",
            OcrProvider::Mathpix | OcrProvider::Custom => "",
        }
    }

    /// The name its key is kept under. One per service, so keys are never shared.
    pub fn secret_key(self) -> String {
        format!("ocr_api_key_{}", self.id())
    }
}

/// What the settings file says. `endpoint` and `model` are empty for the service's own.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OcrConfig {
    pub enabled: bool,
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
            provider: OcrProvider::parse(&text(PROVIDER_KEY)).unwrap_or_default(),
            // A settings file that was meddled with cannot point a key at an unsafe address.
            endpoint: if endpoint_is_safe(&endpoint) { endpoint } else { String::new() },
            model: text(MODEL_KEY),
        }
    }

    pub(crate) fn save(&self, settings: &Settings) -> Result<(), nocturne_store::StoreError> {
        settings.set(MATH_OCR_ENABLED_KEY, if self.enabled { "true" } else { "false" })?;
        settings.set(PROVIDER_KEY, self.provider.id())?;
        settings.set(ENDPOINT_KEY, &self.endpoint)?;
        settings.set(MODEL_KEY, &self.model)
    }

    pub fn effective_endpoint(&self) -> &str {
        if self.endpoint.is_empty() { self.provider.default_endpoint() } else { &self.endpoint }
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

/// Why there is no LaTeX. None of these carries a key or the text of a reply.
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
    /// The answer was cut off before any LaTeX.
    Truncated,
    /// Another HTTP status.
    Status(u16),
    /// The request did not get there or back.
    Transport(String),
    /// The reply had no LaTeX in it.
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
    pub provider: OcrProvider,
    pub endpoint: String,
    pub model: String,
    pub key: String,
    pub app_id: String,
    pub png: Vec<u8>,
}

impl fmt::Debug for OcrJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OcrJob")
            .field("enabled", &self.enabled)
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("has_key", &!self.key.is_empty())
            .field("png_bytes", &self.png.len())
            .finish()
    }
}

impl Session {
    pub fn ocr_config(&self) -> OcrConfig {
        OcrConfig::load(&self.settings)
    }

    /// Turns equation recognition on or off. False if the settings file could not be written.
    pub fn set_math_ocr_enabled(&mut self, enabled: bool) -> bool {
        self.settings.set(MATH_OCR_ENABLED_KEY, if enabled { "true" } else { "false" }).is_ok()
    }

    /// True if the chosen service has all it needs: an address, a model, and its key.
    pub fn math_ocr_ready(&self) -> bool {
        self.ocr_parts().is_ok()
    }

    fn ocr_parts(&self) -> Result<(OcrConfig, String, String), OcrError> {
        let cfg = self.ocr_config();
        if !cfg.enabled {
            return Err(OcrError::Disabled);
        }
        if !self.sync_http().is_supported() {
            return Err(OcrError::Unsupported);
        }
        if cfg.effective_endpoint().is_empty() {
            return Err(OcrError::NotConfigured("the service's address"));
        }
        if cfg.effective_model().is_empty() && cfg.provider != OcrProvider::Mathpix {
            return Err(OcrError::NotConfigured("the name of a model"));
        }
        let secrets = self.sync_secrets();
        let secret = |name: &str| secrets.get_secret(name).ok().flatten().unwrap_or_default();
        let key = secret(&cfg.provider.secret_key());
        if key.is_empty() && cfg.provider != OcrProvider::Custom {
            return Err(OcrError::NotConfigured("a key"));
        }
        let app_id = if cfg.provider == OcrProvider::Mathpix { secret(MATHPIX_APP_ID_SECRET) } else { String::new() };
        if cfg.provider == OcrProvider::Mathpix && app_id.is_empty() {
            return Err(OcrError::NotConfigured("an app id"));
        }
        Ok((cfg, key, app_id))
    }

    /// The main-thread half: checks the switch and the settings, and draws the picture.
    pub fn math_ocr_job(&self, strokes: &[Stroke]) -> Result<OcrJob, OcrError> {
        let (cfg, key, app_id) = self.ocr_parts()?;
        let png = raster::png(strokes)?;
        Ok(OcrJob {
            enabled: true,
            provider: cfg.provider,
            endpoint: cfg.effective_endpoint().to_string(),
            model: cfg.effective_model().to_string(),
            key,
            app_id,
            png,
        })
    }
}

/// The worker half: sends the picture and reads the LaTeX out of the reply.
pub fn recognize(http: &dyn SyncHttpClient, job: &OcrJob) -> Result<String, OcrError> {
    if !job.enabled {
        return Err(OcrError::Disabled);
    }
    if !http.is_supported() {
        return Err(OcrError::Unsupported);
    }
    if !endpoint_is_safe(&job.endpoint) {
        return Err(OcrError::NotConfigured("the service's address"));
    }
    let reply = http.request(provider::request(job)).map_err(|e| OcrError::Transport(without_secrets(&e.to_string(), job)))?;
    match reply.status {
        200..=299 => provider::answer(job.provider, &reply.body),
        401 | 403 => Err(OcrError::Auth),
        429 => Err(OcrError::RateLimited),
        other => Err(OcrError::Status(other)),
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
        // The opening fence may name a language; the body starts on the next line.
        let body = rest.split_once('\n').map_or("", |(_, body)| body);
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
}
