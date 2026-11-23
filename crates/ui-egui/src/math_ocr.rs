//! Handwritten equations to LaTeX: lasso pill, waiting/preview modal, and service setup.

use crate::modal::{self, Modal};
use crate::theme::{self, RADIUS_CONTROL, RADIUS_PILL, Tokens};
use egui::{Align2, Context, CornerRadius, Id, Key, Modifiers, Order, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, vec2};
use nocturne_engine::ocr::local_model::{self, DownloadProgress};
use nocturne_engine::ocr::{self, OcrError, OcrJob, OcrProvider};
use nocturne_engine::{OcrMode, SyncError, SyncHttpClient};
use nocturne_ink::Stroke as InkStroke;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::{fmt, thread};

/// The id of the preview's single-line TeX box, so tests and the platform keyboard can find it.
pub const TEX_FIELD: &str = "math-ocr-tex";

/// One recognition request on a worker thread.
pub struct OcrRun {
    rx: Receiver<Result<String, OcrError>>,
}

impl OcrRun {
    pub fn start(ctx: Context, http: Arc<dyn SyncHttpClient>, job: OcrJob) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("nocturne-math-ocr".into())
            .spawn(move || {
                let _ = tx.send(ocr::recognize(&*http, &job));
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { rx })
    }

    #[must_use]
    pub fn try_recv(&self) -> Option<Result<String, OcrError>> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(OcrError::Transport(String::new()))),
        }
    }
}

/// Background downloader for the on-device Pix2Text MFR 1.5 math model.
pub struct ModelDownloadWorker {
    rx: Receiver<Result<(), SyncError>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<DownloadProgress>>,
}

impl ModelDownloadWorker {
    fn initial_progress() -> DownloadProgress {
        DownloadProgress {
            file_index: 0,
            file_count: local_model::MODEL_FILES.len(),
            file_name: local_model::ENCODER_FILE,
            bytes_downloaded: 0,
            total_bytes: local_model::TOTAL_MODEL_BYTES,
        }
    }

    pub fn start(ctx: Context, http: Arc<dyn SyncHttpClient>, model_dir: PathBuf) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let (cancel, progress) = (Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(Self::initial_progress())));
        let (cancel_bg, progress_bg) = (Arc::clone(&cancel), Arc::clone(&progress));
        thread::Builder::new()
            .name("nocturne-ocr-model-dl".into())
            .spawn(move || {
                let res = local_model::download_model(&*http, &model_dir, Some(&cancel_bg), &mut |p| {
                    if let Ok(mut g) = progress_bg.lock() {
                        *g = p;
                    }
                    ctx.request_repaint();
                });
                let _ = tx.send(res);
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { rx, cancel, progress })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn progress(&self) -> DownloadProgress {
        self.progress.lock().map(|g| *g).unwrap_or_else(|_| Self::initial_progress())
    }

    #[must_use]
    pub fn try_recv(&self) -> Option<Result<(), SyncError>> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(SyncError::Transport("model download thread disconnected".into()))),
        }
    }
}

/// Where a conversion has got to.
pub enum Stage {
    Waiting(OcrRun),
    /// The LaTeX to review and edit. `opened` stays false until the box requests focus once.
    Preview {
        tex: String,
        opened: bool,
    },
}

/// One conversion in progress: the ink it is about, and where it stands.
pub struct Conversion {
    pub page: PathBuf,
    pub strokes: Vec<InkStroke>,
    pub bounds: [f32; 4],
    pub mode: OcrMode,
    pub stage: Stage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Keep,
    Cancel,
    Insert,
}

/// What went wrong, in the reader's language. Never a key, never the text of a reply.
#[must_use]
pub fn error_text(err: &OcrError) -> String {
    match err {
        OcrError::Disabled => crate::tl_id!("ui.ocr_error_disabled").to_string(),
        OcrError::NotConfigured(what) if what.contains("local math model") => crate::tl_id!("ui.ocr_error_model_not_installed").to_string(),
        OcrError::NotConfigured(_) => crate::tl_id!("ui.ocr_error_not_configured").to_string(),
        OcrError::Unsupported => crate::tl_id!("ui.ocr_error_unsupported").to_string(),
        OcrError::NoInk | OcrError::Picture => crate::tl_id!("ui.ocr_error_no_ink").to_string(),
        OcrError::Auth => crate::tl_id!("ui.ocr_error_auth").to_string(),
        OcrError::RateLimited => crate::tl_id!("ui.ocr_error_busy").to_string(),
        OcrError::Refused => crate::tl_id!("ui.ocr_error_refused").to_string(),
        OcrError::Truncated => crate::tl_id!("ui.ocr_error_cut_off").to_string(),
        OcrError::Status(code) => crate::i18n::format_message_id("ui.ocr_error_status", &[("code", &code.to_string())]),
        OcrError::Transport(_) => crate::tl_id!("ui.ocr_error_transport").to_string(),
        OcrError::BadReply => crate::tl_id!("ui.ocr_error_bad_reply").to_string(),
    }
}

/// The floating pill over held ink, offering whichever recognition modes are enabled.
pub fn convert_buttons(
    ctx: &Context,
    held_rect: Rect,
    view_rect: Rect,
    math_enabled: bool,
    handwriting_enabled: bool,
    t: &Tokens,
) -> Option<OcrMode> {
    if !math_enabled && !handwriting_enabled {
        return None;
    }
    let h = crate::touch_ux::TOUCH_PILL_HEIGHT;
    let above = held_rect.top() - h - 8.0 >= view_rect.top();
    let at = egui::pos2(held_rect.center().x, if above { held_rect.top() - 8.0 } else { held_rect.bottom() + 8.0 });
    let mut chosen = None;
    egui::Area::new(Id::unique("math-ocr-convert"))
        .order(Order::Foreground)
        .pivot(if above { Align2::CENTER_BOTTOM } else { Align2::CENTER_TOP })
        .fixed_pos(at)
        .constrain_to(view_rect)
        .show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(t.surface_elevated)
                .stroke(Stroke::new(1.0, t.border))
                .shadow(t.shadow_popup())
                .inner_margin(egui::Margin::symmetric(10, 4))
                .corner_radius(CornerRadius::same(RADIUS_PILL));
            frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    if math_enabled {
                        let label = RichText::new(crate::tl_id!("ui.convert_to_latex")).color(t.text);
                        if ui.add(egui::Button::new(label).frame(false).min_size(vec2(120.0, h - 8.0))).clicked() {
                            chosen = Some(OcrMode::MathLatex);
                        }
                    }
                    if math_enabled && handwriting_enabled {
                        ui.separator();
                    }
                    if handwriting_enabled {
                        let label = RichText::new(crate::tl_id!("ui.ocr_convert_to_text")).color(t.text);
                        let resp = ui
                            .add(egui::Button::new(label).frame(false).min_size(vec2(110.0, h - 8.0)))
                            .on_hover_text(crate::tl_id!("ui.ocr_convert_to_text_tip"));
                        if resp.clicked() {
                            chosen = Some(OcrMode::HandwritingText);
                        }
                    }
                });
            });
        });
    chosen
}

/// The pill over held ink. `held` is where the held ink is on screen. True when pressed.
pub fn convert_button(ctx: &Context, held: Rect, view: Rect, t: &Tokens) -> bool {
    convert_buttons(ctx, held, view, true, false, t).is_some()
}

/// True when the TeX is something the typesetter reads in full, not its plain-text fallback.
#[must_use]
pub fn typesets(ui: &Ui, tex: &str) -> bool {
    !tex.trim().is_empty() && crate::mathview::measure(ui, tex, 18.0, true).engine == nocturne_math::Engine::Ratex
}

fn modal_title(mode: OcrMode) -> &'static str {
    match mode {
        OcrMode::MathLatex => crate::tl_id!("ui.convert_to_latex"),
        OcrMode::HandwritingText => crate::tl_id!("ui.ocr_text_window_title"),
    }
}

/// Draws the conversion: the waiting card or the preview modal. Says what the writer chose.
pub fn show(ctx: &Context, conv: &mut Conversion, t: &Tokens) -> Outcome {
    let mode = conv.mode;
    match &mut conv.stage {
        Stage::Waiting(_) => show_waiting(ctx, mode, t),
        Stage::Preview { tex, opened } => show_preview(ctx, mode, tex, opened, t),
    }
}

fn show_waiting(ctx: &Context, mode: OcrMode, t: &Tokens) -> Outcome {
    let mut outcome = Outcome::Keep;
    let inner_w = (modal::responsive_width(ctx.viewport_rect().width(), 360.0) - 32.0).max(160.0);
    // Uses the shared modal frame without a blocking full-screen scrim so the writer can still
    // alter strokes mid-request (tested by ink_changed_mid_request_replaces_nothing).
    egui::Area::new(Id::unique("math-ocr-window")).order(Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).movable(false).show(
        ctx,
        |ui| {
            modal::frame(t).show(ui, |ui| {
                ui.set_width(inner_w);
                ui.set_max_width(inner_w);
                ui.visuals_mut().override_text_color = Some(t.text);
                if modal::header(ui, modal_title(mode), true, t) {
                    outcome = Outcome::Cancel;
                }
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(crate::tl_id!("ui.reading_the_handwriting")).color(t.text));
                });
                ui.add_space(10.0);
                modal::hairline(ui, t);
                ui.add_space(10.0);
                if modal::secondary_button(ui, crate::tl_id!("ui.cancel"), t).clicked() {
                    outcome = Outcome::Cancel;
                }
            });
        },
    );
    if outcome == Outcome::Keep && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        outcome = Outcome::Cancel;
    }
    outcome
}

fn show_preview(ctx: &Context, mode: OcrMode, text: &mut String, opened: &mut bool, t: &Tokens) -> Outcome {
    crate::ribbon::mark_popup_open(ctx);
    let usable = !text.trim().is_empty();
    let res = Modal::new("math-ocr-window", modal_title(mode)).backdrop_id("math-ocr-window-backdrop").width(400.0).show_with_footer(
        ctx,
        t,
        |ui| {
            let tex_id = Id::unique(TEX_FIELD);
            match mode {
                OcrMode::MathLatex => {
                    if text.contains(['\r', '\n']) {
                        *text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
                    }
                    let preview = egui::Frame::NONE
                        .fill(t.code_bg)
                        .stroke(Stroke::new(1.0, t.border_subtle))
                        .inner_margin(egui::Margin::same(10))
                        .corner_radius(CornerRadius::same(RADIUS_CONTROL));
                    preview.show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        crate::mathview::display(ui, text, 20.0, t.strong);
                    });
                    ui.add_space(8.0);
                    ui.label(RichText::new(crate::tl_id!("ui.latex_source")).color(t.text));
                    if !*opened && ui.is_enabled() && !ui.is_sizing_pass() {
                        ui.memory_mut(|m| m.request_focus(tex_id));
                        *opened = true;
                    }
                    ui.add(egui::TextEdit::singleline(text).id(tex_id).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                    if text.contains(['\r', '\n']) {
                        *text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
                    }
                    if !text.trim().is_empty() && !typesets(ui, text) {
                        ui.add_space(4.0);
                        ui.colored_label(t.warn, crate::tl_id!("ui.latex_not_typeset"));
                    }
                }
                OcrMode::HandwritingText => {
                    ui.label(RichText::new(crate::tl_id!("ui.ocr_text_edit_label")).color(t.text));
                    ui.add_space(4.0);
                    if !*opened && ui.is_enabled() && !ui.is_sizing_pass() {
                        ui.memory_mut(|m| m.request_focus(tex_id));
                        *opened = true;
                    }
                    ui.add(egui::TextEdit::multiline(text).id(tex_id).desired_rows(4).desired_width(f32::INFINITY));
                }
            }
        },
        |ui| {
            let mut out = Outcome::Keep;
            let insert_label = match mode {
                OcrMode::MathLatex => crate::tl_id!("ui.insert_equation"),
                OcrMode::HandwritingText => crate::tl_id!("ui.ocr_insert_text"),
            };
            ui.horizontal_wrapped(|ui| {
                if modal::secondary_button(ui, crate::tl_id!("ui.cancel"), t).clicked() {
                    out = Outcome::Cancel;
                }
                if modal::primary_button_enabled(ui, insert_label, usable, t).clicked() {
                    out = Outcome::Insert;
                }
            });
            out
        },
    );
    let mut outcome = res.inner.1;
    if outcome == Outcome::Keep && res.closed {
        outcome = Outcome::Cancel;
    }
    outcome
}

/// What the service setup screen is showing and what has been typed into it.
#[derive(Clone, Default)]
pub struct Setup {
    pub provider: OcrProvider,
    pub endpoint: String,
    pub model: String,
    pub key: String,
    pub app_id: String,
    pub has_key: [bool; OcrProvider::ALL.len()],
    pub has_app_id: bool,
    pub model_installed: bool,
    pub native_ocr_available: bool,
    pub error: Option<String>,
}

impl fmt::Debug for Setup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Setup")
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("has_typed_key", &!self.key.is_empty())
            .field("has_typed_app_id", &!self.app_id.is_empty())
            .field("has_key", &self.has_key)
            .field("has_app_id", &self.has_app_id)
            .field("model_installed", &self.model_installed)
            .field("native_ocr_available", &self.native_ocr_available)
            .field("has_error", &self.error.is_some())
            .finish()
    }
}

impl Setup {
    #[must_use]
    pub fn from_session(session: &nocturne_engine::Session) -> Self {
        let cfg = session.ocr_config();
        Self {
            provider: cfg.provider,
            endpoint: cfg.endpoint,
            model: cfg.model,
            key: String::new(),
            app_id: String::new(),
            has_key: OcrProvider::ALL.map(|p| session.ocr_has_key(p)),
            has_app_id: session.ocr_has_mathpix_app_id(),
            model_installed: local_model::is_model_installed(&session.ocr_model_dir()),
            native_ocr_available: session.native_ocr_backend().is_some_and(|b| b.is_supported()),
            error: None,
        }
    }

    pub fn refresh_local_status(&mut self, session: &nocturne_engine::Session) {
        self.model_installed = local_model::is_model_installed(&session.ocr_model_dir());
        self.native_ocr_available = session.native_ocr_backend().is_some_and(|b| b.is_supported());
    }

    fn key_kept(&self) -> bool {
        OcrProvider::ALL.iter().position(|p| *p == self.provider).and_then(|i| self.has_key.get(i).copied()).unwrap_or(false)
    }

    #[must_use]
    pub fn kept(&self) -> bool {
        match self.provider {
            OcrProvider::AppleVision | OcrProvider::LocalModel => true,
            OcrProvider::Mathpix => self.key_kept() && self.has_app_id,
            _ => self.key_kept(),
        }
    }

    #[must_use]
    pub fn has_stored_secret(&self) -> bool {
        match self.provider {
            OcrProvider::AppleVision | OcrProvider::LocalModel => false,
            OcrProvider::Mathpix => self.key_kept() || self.has_app_id,
            _ => self.key_kept(),
        }
    }

    #[must_use]
    pub fn can_save(&self, downloading: bool) -> bool {
        match self.provider {
            OcrProvider::AppleVision => self.native_ocr_available,
            OcrProvider::LocalModel => self.model_installed && !downloading,
            _ => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupOutcome {
    Keep,
    Close,
    Save,
    ForgetKey,
    DownloadModel,
    CancelDownload,
    RemoveModel,
}

fn provider_label(provider: OcrProvider) -> &'static str {
    match provider {
        OcrProvider::AppleVision => crate::tl_id!("ui.ocr_provider_apple_vision"),
        OcrProvider::LocalModel => crate::tl_id!("ui.ocr_provider_local_model"),
        OcrProvider::Anthropic => crate::tl_id!("ui.ocr_provider_anthropic"),
        OcrProvider::Gemini => crate::tl_id!("ui.ocr_provider_gemini"),
        OcrProvider::OpenAi => crate::tl_id!("ui.ocr_provider_openai"),
        OcrProvider::Mathpix => crate::tl_id!("ui.ocr_provider_mathpix"),
        OcrProvider::Custom => crate::tl_id!("ui.ocr_provider_custom"),
    }
}

fn progress_bar(ui: &mut Ui, frac: f32, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().max(120.0), 10.0), Sense::hover());
    let r = CornerRadius::same(RADIUS_CONTROL);
    ui.painter().rect_filled(rect, r, t.faint);
    let fill_w = (rect.width() * frac.clamp(0.0, 1.0)).max(0.0);
    if fill_w > 0.5 {
        ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(fill_w, rect.height())), r, theme::mix(t.accent, t.btn_primary_bg, 0.5));
    }
    ui.painter().rect_stroke(rect, r, Stroke::new(1.0, t.border), StrokeKind::Inside);
}

/// Draws the recognition service setup modal.
pub fn show_setup(ctx: &Context, form: &mut Setup, download: Option<DownloadProgress>, t: &Tokens) -> SetupOutcome {
    crate::ribbon::mark_popup_open(ctx);
    let is_on_device = form.provider.is_on_device();
    let has_secret = form.has_stored_secret();
    let can_save = form.can_save(download.is_some());
    let res =
        Modal::new("math-ocr-setup", crate::tl_id!("ui.recognition_service")).backdrop_id("math-ocr-setup-backdrop").width(440.0).show_with_footer(
            ctx,
            t,
            |ui| {
                let mut body_out = SetupOutcome::Keep;
                ui.horizontal_wrapped(|ui| {
                    for provider in OcrProvider::ALL {
                        let chosen = form.provider == provider;
                        let fg = if chosen { t.btn_primary_text } else { t.text };
                        let fill = if chosen { t.btn_primary_bg } else { t.panel };
                        let stroke = if chosen { Stroke::NONE } else { Stroke::new(1.0, t.border) };
                        let btn = egui::Button::new(RichText::new(provider_label(provider)).color(fg))
                            .selected(chosen)
                            .fill(fill)
                            .stroke(stroke)
                            .corner_radius(CornerRadius::same(RADIUS_CONTROL))
                            .min_size(vec2(68.0, 32.0));
                        if ui.add(btn).clicked() && !chosen {
                            form.provider = provider;
                            form.endpoint.clear();
                            form.model.clear();
                            form.key.clear();
                            form.app_id.clear();
                            form.error = None;
                        }
                    }
                });
                ui.add_space(10.0);
                match form.provider {
                    OcrProvider::AppleVision => {
                        ui.label(RichText::new(crate::tl_id!("ui.ocr_apple_vision_desc")).color(t.muted));
                        ui.add_space(6.0);
                        let (msg, col) = if form.native_ocr_available {
                            (crate::tl_id!("ui.ocr_apple_vision_ready"), t.ok)
                        } else {
                            (crate::tl_id!("ui.ocr_apple_vision_unavailable"), t.warn)
                        };
                        ui.label(RichText::new(msg).color(col));
                    }
                    OcrProvider::LocalModel => show_local_model_body(ui, form, download, t, &mut body_out),
                    _ => show_cloud_body(ui, form, t),
                }
                if let Some(err) = &form.error {
                    ui.add_space(8.0);
                    ui.label(RichText::new(err).color(t.warn));
                }
                body_out
            },
            |ui| {
                let mut foot_out = SetupOutcome::Keep;
                ui.horizontal_wrapped(|ui| {
                    if modal::secondary_button(ui, crate::tl_id!("ui.close"), t).clicked() {
                        foot_out = SetupOutcome::Close;
                    }
                    if !is_on_device && modal::secondary_button_enabled(ui, crate::tl_id!("ui.forget_key"), has_secret, t).clicked() {
                        foot_out = SetupOutcome::ForgetKey;
                    }
                    if modal::primary_button_enabled(ui, crate::tl_id!("ui.save_service"), can_save, t).clicked() {
                        foot_out = SetupOutcome::Save;
                    }
                });
                foot_out
            },
        );
    let mut outcome = if res.inner.1 != SetupOutcome::Keep { res.inner.1 } else { res.inner.0 };
    if outcome == SetupOutcome::Keep && res.closed {
        outcome = SetupOutcome::Close;
    }
    outcome
}

#[path = "math_ocr_setup.rs"]
mod setup_body;

use setup_body::{show_cloud_body, show_local_model_body};
