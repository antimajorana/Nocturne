//! Shared responsive modal dialog kit.
//!
//! Provides a single, theme-aware modal surface (`Order::Foreground`) with:
//! - Full-screen dimmed `Tokens::scrim` backdrop blocking clicks and drags to underlying chrome
//! - Sublayer ordering (`stay_above`) so clicking the backdrop can never raise it over the modal
//! - Rounded `RADIUS_MODAL` elevated surface (`Tokens::surface_elevated`), visible `Tokens::border`,
//!   and `Tokens::shadow_floating()`
//! - Single title treatment (no duplicate native `egui::Window` title bar) with optional close button
//! - Responsive width clamping for narrow iPad Split View / Slide Over viewports
//! - Vertical `ScrollArea` for overflowing body content with optional pinned footer
//! - Styled primary, danger, and secondary button helpers with normal, hover, pressed, disabled,
//!   and keyboard-focus ring states using contrast-guaranteed `Tokens::btn_*` tokens

use crate::theme::{self, RADIUS_CONTROL, RADIUS_MODAL, Tokens};
use egui::{
    Align2, Color32, Context, CornerRadius, FontId, Id, Key, LayerId, Margin, Modifiers, Order, Rect, Response, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, Ui, Vec2, pos2, vec2,
};

/// Default desired modal width in points before viewport clamping.
pub const DEFAULT_WIDTH: f32 = 460.0;
/// Default maximum fraction of viewport height a modal may occupy.
pub const DEFAULT_MAX_HEIGHT_FRACTION: f32 = 0.86;
/// Minimum interactive height for modal action buttons (comfortable for desktop and touch).
pub const BUTTON_HEIGHT: f32 = 34.0;

/// Visual emphasis of a modal action button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// High-emphasis confirmation / call-to-action button (`Tokens::btn_primary_*`).
    Primary,
    /// Neutral / cancel / alternate action button (`Tokens::panel` + `Tokens::border`).
    Secondary,
    /// Destructive action button (`Tokens::btn_danger_*`).
    Danger,
}

/// Result returned by [`Modal::show`] and [`Modal::show_with_footer`].
#[derive(Clone, Debug)]
pub struct ModalResponse<R> {
    /// The value returned by the body (or `(body, footer)`) closure.
    pub inner: R,
    /// True if the modal should close (close button clicked or Escape pressed when enabled).
    pub closed: bool,
    /// True if Escape was pressed and consumed this frame.
    pub escaped: bool,
    /// True if the user clicked on the dimmed scrim outside the modal panel.
    pub backdrop_clicked: bool,
    /// Outer screen rectangle occupied by the modal card.
    pub modal_rect: Rect,
    /// Layer ID of the dimmed scrim backdrop.
    pub backdrop_layer: LayerId,
    /// Layer ID of the modal dialog surface.
    pub modal_layer: LayerId,
}

/// Computes a responsive modal width clamped cleanly inside `viewport_width`,
/// keeping comfortable horizontal margins even on narrow iPad Split View / Slide Over screens.
#[must_use]
pub fn responsive_width(viewport_width: f32, desired: f32) -> f32 {
    let target = if desired.is_finite() && desired > 0.0 { desired } else { DEFAULT_WIDTH };
    if !viewport_width.is_finite() || viewport_width <= 0.0 {
        return target.max(200.0);
    }
    let margin = if viewport_width < 480.0 { 16.0 } else { 32.0 };
    let max_w = (viewport_width - margin * 2.0).max(160.0).min(viewport_width);
    target.min(max_w)
}

/// Computes the maximum height available for the scrollable modal body given `viewport_height`.
#[must_use]
pub fn responsive_body_max_height(viewport_height: f32, max_height_fraction: f32, has_footer: bool) -> f32 {
    if !viewport_height.is_finite() || viewport_height <= 0.0 {
        return 420.0;
    }
    let fraction = max_height_fraction.clamp(0.4, 0.95);
    let chrome = if has_footer { 148.0 } else { 96.0 };
    (viewport_height * fraction - chrome).max(120.0)
}

/// Paints a full-viewport dimmed `Tokens::scrim` backdrop at `Order::Foreground` that blocks
/// pointer clicks and drags from reaching underlying controls or canvas.
///
/// Returns `(backdrop_layer, clicked)`. Pair with [`stay_above`] on the modal surface so clicking
/// the backdrop never raises it over the dialog.
pub fn backdrop(ctx: &Context, id: Id, t: &Tokens) -> (LayerId, bool) {
    let (layer_id, clicked, _) = backdrop_with_sizing(ctx, id, t);
    (layer_id, clicked)
}

fn backdrop_with_sizing(ctx: &Context, id: Id, t: &Tokens) -> (LayerId, bool, bool) {
    let screen = ctx.viewport_rect();
    let mut clicked = false;
    let mut sizing = false;
    let layer_id = egui::Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .constrain(false)
        .movable(false)
        .show(ctx, |ui| {
            let response = ui.allocate_rect(Rect::from_min_size(screen.min, screen.size()), Sense::click_and_drag());
            ui.painter().rect_filled(response.rect, 0.0, t.scrim);
            clicked = response.clicked();
            sizing = ui.is_sizing_pass();
        })
        .response
        .layer_id;
    (layer_id, clicked, sizing)
}

/// Keeps `modal_layer` stacked directly above `backdrop_layer` inside `Order::Foreground`.
///
/// egui raises whichever area was clicked last within the same `Order`, so without this
/// sublayer pin, clicking the dimmed backdrop would lift it above the dialog (issue #98).
pub fn stay_above(ctx: &Context, backdrop_layer: LayerId, modal_layer: LayerId) {
    ctx.set_sublayer(backdrop_layer, modal_layer);
}

/// Standard elevated modal card frame: `Tokens::surface_elevated` fill, `Tokens::border` stroke,
/// `RADIUS_MODAL` corner radius, and `Tokens::shadow_floating()`.
pub fn frame(t: &Tokens) -> egui::Frame {
    egui::Frame::NONE
        .fill(t.surface_elevated)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(RADIUS_MODAL))
        .shadow(t.shadow_floating())
        .inner_margin(Margin::same(16))
}

/// Draws the single modal title row with an optional close (`×`) button.
///
/// Returns `true` when the close button was clicked.
pub fn header(ui: &mut Ui, title: &str, closable: bool, t: &Tokens) -> bool {
    let mut close_clicked = false;
    ui.horizontal(|ui| {
        let close_reserve = if closable { 36.0 } else { 0.0 };
        let title_w = (ui.available_width() - close_reserve).max(80.0);
        ui.allocate_ui_with_layout(vec2(title_w, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(title).strong().size(17.0).color(t.strong)).wrap());
        });
        if closable {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if close_button(ui, t).clicked() {
                    close_clicked = true;
                }
            });
        }
    });
    ui.add_space(8.0);
    hairline(ui, t);
    ui.add_space(10.0);
    close_clicked
}

/// Draws a crisp 1px horizontal divider in `Tokens::border_subtle`.
pub fn hairline(ui: &mut Ui, t: &Tokens) {
    let width = ui.available_width().max(1.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    let y = rect.center().y.round() + 0.5;
    ui.painter().hline(rect.min.x..=rect.max.x, y, Stroke::new(1.0, t.border_subtle));
}

fn close_button(ui: &mut Ui, t: &Tokens) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let hovered = resp.hovered() || resp.is_pointer_button_down_on();
    let painter = ui.painter();
    if hovered {
        let fill = if resp.is_pointer_button_down_on() { theme::mix(t.close_hover, Color32::BLACK, 0.14) } else { t.close_hover };
        painter.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), fill);
    }
    if resp.has_focus() {
        painter.rect_stroke(rect.expand(2.0), CornerRadius::same(RADIUS_CONTROL + 2), Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
    let glyph_color = if hovered { Color32::WHITE } else { t.muted };
    let c = rect.center();
    let r = 5.0;
    let stroke = Stroke::new(1.6, glyph_color);
    painter.line_segment([pos2(c.x - r, c.y - r), pos2(c.x + r, c.y + r)], stroke);
    painter.line_segment([pos2(c.x + r, c.y - r), pos2(c.x - r, c.y + r)], stroke);
    let close_label = crate::tl_id!("ui.close").to_string();
    let label_for_info = close_label.clone();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label_for_info.clone()));
    resp.on_hover_text(close_label)
}

/// Styled modal button with normal, hover, pressed, disabled, and keyboard-focus ring states.
pub fn button(ui: &mut Ui, label: &str, kind: ButtonKind, enabled: bool, t: &Tokens) -> Response {
    let font = FontId::proportional(13.5);
    let text_w = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x;
    let width = (text_w + 32.0).max(84.0).min(ui.available_width().max(84.0));
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, BUTTON_HEIGHT), sense);

    let dark = t.is_dark();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let hovered = enabled && resp.hovered();

    let (fill, border_stroke, text_color, focus_color) = if !enabled {
        (t.faint, Stroke::new(1.0, t.border_subtle), t.disabled, t.accent)
    } else {
        match kind {
            ButtonKind::Primary => {
                let bg = if pressed {
                    theme::mix(t.btn_primary_hover, if dark { Color32::WHITE } else { Color32::BLACK }, 0.12)
                } else if hovered {
                    t.btn_primary_hover
                } else {
                    t.btn_primary_bg
                };
                (bg, Stroke::NONE, t.btn_primary_text, t.strong)
            }
            ButtonKind::Danger => {
                let bg = if pressed {
                    theme::mix(t.btn_danger_hover, if dark { Color32::WHITE } else { Color32::BLACK }, 0.12)
                } else if hovered {
                    t.btn_danger_hover
                } else {
                    t.btn_danger_bg
                };
                (bg, Stroke::NONE, t.btn_danger_text, t.strong)
            }
            ButtonKind::Secondary => {
                let (bg, edge, fg) = if pressed {
                    (t.row_selected, Stroke::new(1.5, t.accent), t.strong)
                } else if hovered {
                    (t.row_hover, Stroke::new(1.0, t.accent), t.strong)
                } else {
                    (t.panel, Stroke::new(1.0, t.border), t.text)
                };
                (bg, edge, fg, t.accent)
            }
        }
    };

    let painter = ui.painter();
    let radius = CornerRadius::same(RADIUS_CONTROL);
    painter.rect_filled(rect, radius, fill);
    if border_stroke.width > 0.0 {
        painter.rect_stroke(rect, radius, border_stroke, StrokeKind::Inside);
    }
    if enabled && resp.has_focus() {
        painter.rect_stroke(rect.expand(2.0), CornerRadius::same(RADIUS_CONTROL + 2), Stroke::new(2.0, focus_color), StrokeKind::Outside);
    }
    let text_pos = rect.center();
    painter.with_clip_rect(rect.shrink2(vec2(6.0, 2.0))).text(text_pos, Align2::CENTER_CENTER, label, font.clone(), text_color);
    if matches!(kind, ButtonKind::Primary | ButtonKind::Danger) && enabled {
        painter.with_clip_rect(rect.shrink2(vec2(6.0, 2.0))).text(text_pos + vec2(0.35, 0.0), Align2::CENTER_CENTER, label, font, text_color);
    }

    let name = label.to_owned();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name.clone()));
    resp
}

/// High-emphasis primary button (`Tokens::btn_primary_*`).
pub fn primary_button(ui: &mut Ui, label: &str, t: &Tokens) -> Response {
    button(ui, label, ButtonKind::Primary, true, t)
}

/// High-emphasis primary button with explicit enabled state.
pub fn primary_button_enabled(ui: &mut Ui, label: &str, enabled: bool, t: &Tokens) -> Response {
    button(ui, label, ButtonKind::Primary, enabled, t)
}

/// Destructive action button (`Tokens::btn_danger_*`).
pub fn danger_button(ui: &mut Ui, label: &str, t: &Tokens) -> Response {
    button(ui, label, ButtonKind::Danger, true, t)
}

/// Destructive action button with explicit enabled state.
pub fn danger_button_enabled(ui: &mut Ui, label: &str, enabled: bool, t: &Tokens) -> Response {
    button(ui, label, ButtonKind::Danger, enabled, t)
}

/// Neutral secondary button (`Tokens::panel`, `Tokens::border`, `Tokens::text`).
pub fn secondary_button(ui: &mut Ui, label: &str, t: &Tokens) -> Response {
    button(ui, label, ButtonKind::Secondary, true, t)
}

/// Neutral secondary button with explicit enabled state.
pub fn secondary_button_enabled(ui: &mut Ui, label: &str, enabled: bool, t: &Tokens) -> Response {
    button(ui, label, ButtonKind::Secondary, enabled, t)
}

/// Builder for a responsive, theme-aware modal dialog.
pub struct Modal<'a> {
    id: Id,
    backdrop_id: Option<Id>,
    title: &'a str,
    desired_width: f32,
    max_height_fraction: f32,
    closable: bool,
    close_on_escape: bool,
    scrollable: bool,
}

impl<'a> Modal<'a> {
    /// Creates a new modal with the given stable `id_source` and single `title`.
    pub fn new(id_source: impl std::hash::Hash + std::fmt::Debug, title: &'a str) -> Self {
        Self {
            id: Id::new(id_source),
            backdrop_id: None,
            title,
            desired_width: DEFAULT_WIDTH,
            max_height_fraction: DEFAULT_MAX_HEIGHT_FRACTION,
            closable: true,
            close_on_escape: true,
            scrollable: true,
        }
    }

    /// Overrides the backdrop area's `egui::Id` (useful when existing tests query a specific
    /// backdrop ID such as `"rename-modal-backdrop"`).
    #[must_use]
    pub fn backdrop_id(mut self, id_source: impl std::hash::Hash + std::fmt::Debug) -> Self {
        self.backdrop_id = Some(Id::new(id_source));
        self
    }

    /// Sets the preferred outer modal width in points before clamping to the viewport.
    #[must_use]
    pub fn width(mut self, desired_width: f32) -> Self {
        self.desired_width = desired_width;
        self
    }

    /// Sets the maximum fraction of viewport height the modal may occupy (default `0.86`).
    #[must_use]
    pub fn max_height_fraction(mut self, fraction: f32) -> Self {
        self.max_height_fraction = fraction;
        self
    }

    /// Controls whether the header shows a close (`×`) button (default `true`).
    #[must_use]
    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }

    /// Controls whether pressing Escape consumes the key and marks `ModalResponse::closed`
    /// (default `true`).
    #[must_use]
    pub fn close_on_escape(mut self, close_on_escape: bool) -> Self {
        self.close_on_escape = close_on_escape;
        self
    }

    /// Controls whether the modal body is wrapped in a vertical `ScrollArea` (default `true`).
    #[must_use]
    pub fn scrollable(mut self, scrollable: bool) -> Self {
        self.scrollable = scrollable;
        self
    }

    /// Renders the modal with a single body region.
    pub fn show<R>(self, ctx: &Context, t: &Tokens, body: impl FnOnce(&mut Ui) -> R) -> ModalResponse<R> {
        let id = self.id;
        let scrollable = self.scrollable;
        self.show_shell(ctx, t, false, move |ui, body_max_h| {
            if scrollable {
                ScrollArea::vertical().id_salt(id.with("scroll")).max_height(body_max_h).auto_shrink([false, true]).show(ui, body).inner
            } else {
                body(ui)
            }
        })
    }

    /// Renders the modal with a scrollable body region and a pinned non-scrolling footer
    /// (recommended for dialogs with action buttons so actions stay reachable on short screens).
    pub fn show_with_footer<R, F>(
        self,
        ctx: &Context,
        t: &Tokens,
        body: impl FnOnce(&mut Ui) -> R,
        footer: impl FnOnce(&mut Ui) -> F,
    ) -> ModalResponse<(R, F)> {
        let id = self.id;
        let scrollable = self.scrollable;
        self.show_shell(ctx, t, true, move |ui, body_max_h| {
            let body_out = if scrollable {
                ScrollArea::vertical().id_salt(id.with("scroll")).max_height(body_max_h).auto_shrink([false, true]).show(ui, body).inner
            } else {
                body(ui)
            };
            ui.add_space(10.0);
            hairline(ui, t);
            ui.add_space(10.0);
            let footer_out = footer(ui);
            (body_out, footer_out)
        })
    }

    fn show_shell<R>(self, ctx: &Context, t: &Tokens, has_footer: bool, content: impl FnOnce(&mut Ui, f32) -> R) -> ModalResponse<R> {
        let screen = ctx.viewport_rect();
        let outer_w = responsive_width(screen.width(), self.desired_width);
        let inner_w = (outer_w - 32.0).max(128.0);
        let body_max_h = responsive_body_max_height(screen.height(), self.max_height_fraction, has_footer);

        let backdrop_id = self.backdrop_id.unwrap_or_else(|| self.id.with("backdrop"));
        let (backdrop_layer, backdrop_clicked, backdrop_sizing) = backdrop_with_sizing(ctx, backdrop_id, t);

        let mut close_clicked = false;
        let area_out =
            egui::Area::new(self.id).order(Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).movable(false).constrain(true).show(
                ctx,
                |ui| {
                    stay_above(ctx, backdrop_layer, ui.layer_id());
                    frame(t)
                        .show(ui, |ui| {
                            ui.set_width(inner_w);
                            ui.set_max_width(inner_w);
                            ui.visuals_mut().override_text_color = Some(t.text);

                            if header(ui, self.title, self.closable, t) {
                                close_clicked = true;
                            }

                            content(ui, body_max_h)
                        })
                        .inner
                },
            );

        let escaped = self.close_on_escape && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let closed = close_clicked || escaped;
        if backdrop_sizing && !closed {
            ctx.request_discard("modal backdrop sizing");
        }
        ModalResponse {
            inner: area_out.inner,
            closed,
            escaped,
            backdrop_clicked,
            modal_rect: area_out.response.rect,
            backdrop_layer,
            modal_layer: area_out.response.layer_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn responsive_width_clamps_for_narrow_ipad_split_view_and_slide_over() {
        // Desktop wide screen keeps desired width.
        assert_eq!(responsive_width(1440.0, 480.0), 480.0);
        // iPad Split View (420pt): 16pt margin each side -> 388pt max.
        assert_eq!(responsive_width(420.0, 480.0), 388.0);
        // iPad Slide Over (320pt): 16pt margin each side -> 288pt max.
        assert_eq!(responsive_width(320.0, 480.0), 288.0);
        // Narrow modal desired width smaller than viewport stays at desired width.
        assert_eq!(responsive_width(320.0, 260.0), 260.0);
        // Degenerate viewport widths degrade safely without panicking.
        assert!(responsive_width(0.0, 460.0) >= 200.0);
        assert!(responsive_width(f32::NAN, 460.0) >= 200.0);
    }

    #[test]
    fn modal_renders_foreground_backdrop_and_clamps_width_on_narrow_viewport() {
        for theme in [Theme::DEFAULT, Theme { mode: crate::theme::Mode::Dark, accent: crate::theme::Accent::Yellow }] {
            let t = theme.tokens();
            let ctx = Context::default();
            ctx.set_visuals(theme.visuals());
            let mut modal_rect = Rect::NOTHING;
            let mut backdrop_layer = None;
            let mut modal_layer = None;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(320.0, 568.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                let res = Modal::new("test-narrow-modal", "Test Modal").backdrop_id("test-narrow-backdrop").width(520.0).show_with_footer(
                    ui.ctx(),
                    &t,
                    |ui| {
                        for i in 0..20 {
                            ui.label(format!("Line {i} of scrollable content"));
                        }
                    },
                    |ui| {
                        ui.horizontal(|ui| {
                            let _ = primary_button(ui, "Apply", &t);
                            let _ = secondary_button(ui, "Cancel", &t);
                        });
                    },
                );
                modal_rect = res.modal_rect;
                backdrop_layer = Some(res.backdrop_layer);
                modal_layer = Some(res.modal_layer);
                assert!(!res.closed);
            });
            out.textures_delta.clear();
            assert!(modal_rect.width() <= 320.0, "{}: modal overflowed 320pt Slide Over: {modal_rect:?}", theme.label());
            assert!(modal_rect.height() <= 568.0, "{}: modal overflowed 568pt height: {modal_rect:?}", theme.label());
            let bl = backdrop_layer.expect("backdrop layer");
            let ml = modal_layer.expect("modal layer");
            assert_eq!(bl.order, Order::Foreground);
            assert_eq!(ml.order, Order::Foreground);
            assert_ne!(bl, ml);
        }
    }

    #[test]
    fn escape_closes_modal_only_when_close_on_escape_is_enabled() {
        let t = Theme::DEFAULT.tokens();
        for allow_escape in [true, false] {
            let ctx = Context::default();
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))), ..Default::default() };
            input.events.push(egui::Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
            let mut closed = false;
            let mut escaped = false;
            let mut out = ctx.run_ui(input, |ui| {
                let res = Modal::new("test-escape-modal", "Escape Test").close_on_escape(allow_escape).show(ui.ctx(), &t, |ui| {
                    ui.label("Body");
                });
                closed = res.closed;
                escaped = res.escaped;
            });
            out.textures_delta.clear();
            assert_eq!(closed, allow_escape);
            assert_eq!(escaped, allow_escape);
        }
    }

    #[test]
    fn backdrop_blocks_clicks_to_underlying_controls() {
        let t = Theme::DEFAULT.tokens();
        let ctx = Context::default();
        let mut bg_clicked = false;
        let mut backdrop_clicked = false;
        let mut bg_pos = pos2(20.0, 20.0);

        for frame in 0..3 {
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 700.0))), ..Default::default() };
            if frame == 1 {
                input.events.push(egui::Event::PointerMoved(bg_pos));
                input.events.push(egui::Event::PointerButton {
                    pos: bg_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                });
            } else if frame == 2 {
                input.events.push(egui::Event::PointerMoved(bg_pos));
                input.events.push(egui::Event::PointerButton {
                    pos: bg_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                });
            }
            let mut out = ctx.run_ui(input, |ui| {
                let r = ui.button("Behind modal");
                bg_pos = r.rect.center();
                if r.clicked() {
                    bg_clicked = true;
                }
                let res = Modal::new("blocking-modal", "Blocking").width(300.0).show(ui.ctx(), &t, |ui| {
                    ui.label("Centered dialog");
                });
                if res.backdrop_clicked {
                    backdrop_clicked = true;
                }
            });
            out.textures_delta.clear();
        }
        assert!(!bg_clicked, "clicks outside the modal must be blocked by the scrim backdrop");
        assert!(backdrop_clicked, "the backdrop should receive the click outside the modal");
    }

    #[test]
    fn styled_buttons_render_across_all_fourteen_themes() {
        for theme in Theme::all() {
            let t = theme.tokens();
            let ctx = Context::default();
            ctx.set_visuals(theme.visuals());
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let p = primary_button(ui, "Save", &t);
                let pd = primary_button_enabled(ui, "Save disabled", false, &t);
                let d = danger_button(ui, "Delete", &t);
                let dd = danger_button_enabled(ui, "Delete disabled", false, &t);
                let s = secondary_button(ui, "Cancel", &t);
                let sd = secondary_button_enabled(ui, "Cancel disabled", false, &t);
                assert!(p.rect.height() >= BUTTON_HEIGHT);
                assert!(pd.rect.height() >= BUTTON_HEIGHT);
                assert!(d.rect.height() >= BUTTON_HEIGHT);
                assert!(dd.rect.height() >= BUTTON_HEIGHT);
                assert!(s.rect.height() >= BUTTON_HEIGHT);
                assert!(sd.rect.height() >= BUTTON_HEIGHT);
            });
            out.textures_delta.clear();
        }
    }
}
