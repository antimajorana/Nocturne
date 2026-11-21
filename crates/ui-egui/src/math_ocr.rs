//! Handwritten equations to LaTeX, as the window shows it.
//!
//! Three small pieces, none of which is drawn while recognition is off:
//! - a "Convert to LaTeX" button over ink that has been picked up with the lasso;
//! - the conversion itself: a wait while a worker thread asks the service, then a preview
//!   where the LaTeX can be corrected before it replaces the ink;
//! - the screen where the service, its address, its model and its key are given.
//!
//! The request runs on its own thread ([`OcrRun`]) so the window never waits on the network.
//! The strokes being read are kept by value, not by position: the page can change while the
//! answer is on its way, and the ink is only replaced if those very strokes are still there.

use crate::theme::Tokens;
use egui::{Align2, Context, Id, Key, Order, Rect, RichText, Ui, vec2};
use nocturne_engine::SyncHttpClient;
use nocturne_engine::ocr::{self, OcrError, OcrJob, OcrProvider};
use nocturne_ink::Stroke;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// The id of the preview's single-line TeX box, so tests and the platform keyboard can find it.
pub const TEX_FIELD: &str = "math-ocr-tex";

const MATHPIX_APP_ID_SECRET: &str = "ocr_app_id_mathpix";

/// One request to the service, on a worker thread.
pub struct OcrRun {
    rx: Receiver<Result<String, OcrError>>,
}

impl OcrRun {
    pub fn start(ctx: Context, http: Arc<dyn SyncHttpClient>, job: OcrJob) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("nocturne-math-ocr".into())
            .spawn(move || {
                let _ = tx.send(ocr::recognize(&*http, &job));
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { rx })
    }

    /// The answer, once there is one. A worker that died without answering is a failure.
    #[must_use]
    pub fn try_recv(&self) -> Option<Result<String, OcrError>> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(OcrError::Transport(String::new()))),
        }
    }
}

/// Where a conversion has got to.
pub enum Stage {
    Waiting(OcrRun),
    /// The LaTeX, to be looked at and corrected. `opened` is false until its text box has
    /// asked for the keyboard once.
    Preview {
        tex: String,
        opened: bool,
    },
}

/// One conversion in progress: the ink it is about, and where it stands.
pub struct Conversion {
    /// The page the ink is on. A conversion does not follow the writer to another page.
    pub page: PathBuf,
    pub strokes: Vec<Stroke>,
    /// The box round those strokes, in page points: x0, y0, x1, y1.
    pub bounds: [f32; 4],
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

/// The button over held ink. `held` is where the held ink is on screen. True when pressed.
pub fn convert_button(ctx: &Context, held: Rect, view: Rect, t: &Tokens) -> bool {
    let height = crate::touch_ux::TOUCH_PILL_HEIGHT;
    // Above the ink when there is room, below it when there is not.
    let above = held.top() - height - 8.0 >= view.top();
    let at = if above { egui::pos2(held.center().x, held.top() - 8.0) } else { egui::pos2(held.center().x, held.bottom() + 8.0) };
    let mut pressed = false;
    egui::Area::new(Id::unique("math-ocr-convert"))
        .order(Order::Foreground)
        .pivot(if above { Align2::CENTER_BOTTOM } else { Align2::CENTER_TOP })
        .fixed_pos(at)
        .constrain_to(view)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(t.panel)
                .stroke(egui::Stroke::new(1.0, t.container_border))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .corner_radius(height / 2.0)
                .show(ui, |ui| {
                    let label = RichText::new(crate::tl_id!("ui.convert_to_latex")).color(t.text);
                    pressed = ui.add(egui::Button::new(label).frame(false).min_size(vec2(120.0, height - 8.0))).clicked();
                });
        });
    pressed
}

/// True when the TeX is something the typesetter reads in full, not its plain-text fallback.
#[must_use]
pub fn typesets(ui: &Ui, tex: &str) -> bool {
    !tex.trim().is_empty() && crate::mathview::measure(ui, tex, 18.0, true).engine == nocturne_math::Engine::Ratex
}

fn modal_frame(t: &Tokens) -> egui::Frame {
    egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.5, t.accent)).inner_margin(egui::Margin::same(16))
}

// i18n-ui-text: title
fn modal_title(ui: &mut Ui, t: &Tokens, title: &str) {
    egui::Frame::NONE.fill(t.band).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).strong().size(17.0).color(t.band_text));
    });
    ui.add_space(10.0);
}

fn modal_backdrop(ctx: &Context, id: &str) -> egui::LayerId {
    let screen = ctx.viewport_rect();
    egui::Area::new(Id::unique(id))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let response = ui.allocate_rect(Rect::from_min_size(egui::Pos2::ZERO, screen.size()), egui::Sense::click());
            ui.painter().rect_filled(response.rect, 0.0, egui::Color32::from_black_alpha(150));
        })
        .response
        .layer_id
}

fn stay_above(ui: &Ui, backdrop: egui::LayerId) {
    ui.ctx().set_sublayer(backdrop, ui.layer_id());
}

// i18n-ui-text: label
fn primary_button(ui: &mut Ui, t: &Tokens, label: &str, min_width: f32, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(RichText::new(label).color(t.band_text))
        .fill(t.band)
        .stroke(egui::Stroke::new(1.0, t.accent))
        .min_size(vec2(min_width, 34.0));
    ui.add_enabled(enabled, btn)
}

// i18n-ui-text: label
fn secondary_button(ui: &mut Ui, t: &Tokens, label: &str, min_width: f32, enabled: bool) -> egui::Response {
    let btn = egui::Button::new(RichText::new(label).color(t.text))
        .fill(t.panel)
        .stroke(egui::Stroke::new(1.0, t.container_border))
        .min_size(vec2(min_width, 34.0));
    ui.add_enabled(enabled, btn)
}

/// Draws the conversion: the wait, or the preview. Says what the writer chose.
pub fn show(ctx: &Context, conv: &mut Conversion, t: &Tokens) -> Outcome {
    let mut outcome = Outcome::Keep;
    let backdrop = matches!(conv.stage, Stage::Preview { .. }).then(|| modal_backdrop(ctx, "math-ocr-window-backdrop"));
    egui::Window::new(crate::tl_id!("ui.convert_to_latex"))
        .id(Id::unique("math-ocr-window"))
        .title_bar(false)
        .frame(modal_frame(t))
        .collapsible(false)
        .resizable(false)
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            if let Some(layer) = backdrop {
                stay_above(ui, layer);
                crate::ribbon::mark_popup_open(ui.ctx());
            }
            let width = 360.0f32.min((ui.ctx().content_rect().width() - 48.0).max(220.0));
            ui.set_width(width);
            modal_title(ui, t, crate::tl_id!("ui.convert_to_latex"));
            match &mut conv.stage {
                Stage::Waiting(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(crate::tl_id!("ui.reading_the_handwriting")).color(t.text));
                    });
                    ui.add_space(10.0);
                    if secondary_button(ui, t, crate::tl_id!("ui.cancel"), 92.0, true).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        outcome = Outcome::Cancel;
                    }
                }
                Stage::Preview { tex, opened } => {
                    if tex.contains(['\r', '\n']) {
                        *tex = tex.replace("\r\n", " ").replace(['\r', '\n'], " ");
                    }
                    egui::Frame::NONE.fill(t.code_bg).inner_margin(egui::Margin::same(10)).corner_radius(6.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        crate::mathview::display(ui, tex, 20.0, t.strong);
                    });
                    ui.add_space(8.0);
                    ui.label(RichText::new(crate::tl_id!("ui.latex_source")).color(t.text));
                    let tex_id = Id::unique(TEX_FIELD);
                    // A window's first frame only measures it and its widgets are disabled, so
                    // the keyboard is asked for on the first real frame the box can take it.
                    if !*opened && ui.is_enabled() && !ui.is_sizing_pass() {
                        ui.memory_mut(|m| m.request_focus(tex_id));
                        *opened = true;
                    }
                    ui.add(egui::TextEdit::singleline(tex).id(tex_id).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                    if tex.contains(['\r', '\n']) {
                        *tex = tex.replace("\r\n", " ").replace(['\r', '\n'], " ");
                    }
                    let usable = !tex.trim().is_empty();
                    if usable && !typesets(ui, tex) {
                        ui.colored_label(t.warn, crate::tl_id!("ui.latex_not_typeset"));
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if secondary_button(ui, t, crate::tl_id!("ui.cancel"), 92.0, true).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            outcome = Outcome::Cancel;
                        }
                        if primary_button(ui, t, crate::tl_id!("ui.insert_equation"), 136.0, usable).clicked() {
                            outcome = Outcome::Insert;
                        }
                    });
                }
            }
        });
    outcome
}

/// What the service screen is showing and what has been typed into it.
#[derive(Clone, Default)]
pub struct Setup {
    pub provider: OcrProvider,
    /// Empty for the service's own address.
    pub endpoint: String,
    /// Empty for the service's own model.
    pub model: String,
    /// A new key, if one has been typed. The kept key is never read back into the screen.
    pub key: String,
    pub app_id: String,
    /// Which services have a key kept, in the order of `OcrProvider::ALL`.
    pub has_key: [bool; 5],
    /// Whether Mathpix has an `ocr_app_id_mathpix` secret kept.
    pub has_app_id: bool,
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
            .finish()
    }
}

impl Setup {
    #[must_use]
    pub fn from_session(session: &nocturne_engine::Session) -> Self {
        let cfg = session.ocr_config();
        let secrets = session.sync_secrets();
        let has = |name: &str| secrets.get_secret(name).ok().flatten().is_some();
        Self {
            provider: cfg.provider,
            endpoint: cfg.endpoint,
            model: cfg.model,
            key: String::new(),
            app_id: String::new(),
            has_key: OcrProvider::ALL.map(|p| has(&p.secret_key())),
            has_app_id: has(MATHPIX_APP_ID_SECRET),
        }
    }

    #[must_use]
    pub fn kept(&self) -> bool {
        let key_kept = OcrProvider::ALL.iter().position(|p| *p == self.provider).and_then(|i| self.has_key.get(i).copied()).unwrap_or(false);
        if self.provider == OcrProvider::Mathpix { key_kept || self.has_app_id } else { key_kept }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupOutcome {
    Keep,
    Close,
    Save,
    ForgetKey,
}

fn provider_label(provider: OcrProvider) -> &'static str {
    match provider {
        OcrProvider::Anthropic => crate::tl_id!("ui.ocr_provider_anthropic"),
        OcrProvider::Gemini => crate::tl_id!("ui.ocr_provider_gemini"),
        OcrProvider::OpenAi => crate::tl_id!("ui.ocr_provider_openai"),
        OcrProvider::Mathpix => crate::tl_id!("ui.ocr_provider_mathpix"),
        OcrProvider::Custom => crate::tl_id!("ui.ocr_provider_custom"),
    }
}

/// Draws the service screen.
pub fn show_setup(ctx: &Context, form: &mut Setup, t: &Tokens) -> SetupOutcome {
    let mut outcome = SetupOutcome::Keep;
    let backdrop = modal_backdrop(ctx, "math-ocr-setup-backdrop");
    egui::Window::new(crate::tl_id!("ui.recognition_service"))
        .id(Id::unique("math-ocr-setup"))
        .title_bar(false)
        .frame(modal_frame(t))
        .collapsible(false)
        .resizable(false)
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            stay_above(ui, backdrop);
            crate::ribbon::mark_popup_open(ui.ctx());
            let width = 380.0f32.min((ui.ctx().content_rect().width() - 48.0).max(240.0));
            ui.set_width(width);
            modal_title(ui, t, crate::tl_id!("ui.recognition_service"));
            ui.label(RichText::new(crate::tl_id!("ui.recognition_sends_picture")).color(t.muted));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                for provider in OcrProvider::ALL {
                    let chosen = form.provider == provider;
                    let label = RichText::new(provider_label(provider)).color(if chosen { t.band_text } else { t.text });
                    let btn = egui::Button::new(label)
                        .selected(chosen)
                        .fill(if chosen { t.band } else { t.panel })
                        .stroke(egui::Stroke::new(1.0, if chosen { t.accent } else { t.container_border }))
                        .min_size(vec2(72.0, 34.0));
                    if ui.add(btn).clicked() && !chosen {
                        // Another service has its own address and model; the old ones are not carried over.
                        form.provider = provider;
                        form.endpoint.clear();
                        form.model.clear();
                        form.key.clear();
                        form.app_id.clear();
                    }
                }
            });
            ui.add_space(6.0);
            let field_width = (width - 104.0).max(120.0);
            egui::Grid::new(Id::unique("math-ocr-setup-grid")).num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
                ui.label(RichText::new(crate::tl_id!("ui.service_address")).color(t.text));
                ui.add(
                    egui::TextEdit::singleline(&mut form.endpoint)
                        .id(Id::unique("math-ocr-endpoint"))
                        .desired_width(field_width)
                        .hint_text(form.provider.default_endpoint()),
                );
                ui.end_row();
                if form.provider != OcrProvider::Mathpix {
                    ui.label(RichText::new(crate::tl_id!("ui.model_name")).color(t.text));
                    ui.add(
                        egui::TextEdit::singleline(&mut form.model)
                            .id(Id::unique("math-ocr-model"))
                            .desired_width(field_width)
                            .hint_text(form.provider.default_model()),
                    );
                    ui.end_row();
                }
                if form.provider == OcrProvider::Mathpix {
                    ui.label(RichText::new(crate::tl_id!("ui.app_id")).color(t.text));
                    ui.add(egui::TextEdit::singleline(&mut form.app_id).id(Id::unique("math-ocr-app-id")).desired_width(field_width));
                    ui.end_row();
                }
                ui.label(RichText::new(crate::tl_id!("ui.service_key")).color(t.text));
                ui.add(egui::TextEdit::singleline(&mut form.key).id(Id::unique("math-ocr-key")).password(true).desired_width(field_width));
                ui.end_row();
            });
            let kept = form.kept();
            ui.label(RichText::new(if kept { crate::tl_id!("ui.key_is_kept") } else { crate::tl_id!("ui.no_key_kept") }).color(t.muted));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if secondary_button(ui, t, crate::tl_id!("ui.close"), 84.0, true).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    outcome = SetupOutcome::Close;
                }
                if secondary_button(ui, t, crate::tl_id!("ui.forget_key"), 104.0, kept).clicked() {
                    outcome = SetupOutcome::ForgetKey;
                }
                if primary_button(ui, t, crate::tl_id!("ui.save_service"), 92.0, true).clicked() {
                    outcome = SetupOutcome::Save;
                }
            });
        });
    outcome
}
