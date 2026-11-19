//! Dedicated iPadOS / Tablet Studio chrome: single ~44pt top bar, floating Apple Pencil tool
//! pill (`Order::Foreground`), and transient non-blocking status toast banners.

use crate::icons::{self, Icon};
use crate::ribbon::{Act, Pointer, RackEdit, RuleLines, View};
use crate::theme::Tokens;
use egui::os::OperatingSystem;
use egui::{Align2, Color32, Context, Id, Order, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use nocturne_ink::{Rgb, shapes::Shape};

pub const UI_LAYOUT_KEY: &str = "ui_layout_mode";

const NIB_COLOURS: [Rgb; 8] = [
    Rgb(25, 28, 34),
    Rgb(31, 98, 214),
    Rgb(210, 47, 57),
    Rgb(24, 142, 78),
    Rgb(219, 124, 22),
    Rgb(132, 58, 196),
    Rgb(18, 148, 168),
    Rgb(245, 208, 42),
];

const NIB_TENTHS: [u16; 4] = [15, 28, 50, 100];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiLayoutMode {
    #[default]
    Auto,
    TabletStudio,
    ClassicRibbon,
}

impl UiLayoutMode {
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tablet" | "studio" | "tablet-studio" => Self::TabletStudio,
            "ribbon" | "classic" | "classic-ribbon" => Self::ClassicRibbon,
            _ => Self::Auto,
        }
    }

    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::TabletStudio => "tablet",
            Self::ClassicRibbon => "ribbon",
        }
    }

    #[must_use]
    pub fn is_tablet_studio(self, os: OperatingSystem, compact: bool) -> bool {
        if compact {
            return false;
        }
        match self {
            Self::TabletStudio => true,
            Self::ClassicRibbon => false,
            Self::Auto => os == OperatingSystem::IOS,
        }
    }
}

pub struct StudioTopBarIn<'a> {
    pub drawer_open: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub breadcrumb: &'a str,
    pub sync_busy: bool,
    pub sync_configured: bool,
    pub show_source: bool,
    pub view: View<'a>,
}

#[derive(Default)]
pub struct StudioTopBarOut {
    pub toggle_drawer: bool,
    pub nav_back: bool,
    pub nav_forward: bool,
    pub add_page: bool,
    pub clear_page_caret: bool,
    pub acts: Vec<Act>,
}

// i18n-ui-text: tip
fn studio_icon_btn(ui: &mut Ui, icon: Icon, active: bool, enabled: bool, tip: &str, t: &Tokens) -> egui::Response {
    let size = vec2(36.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let tip_owned = tip.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &tip_owned));
    if active {
        ui.painter().rect_filled(rect, 7.0, t.accent.gamma_multiply(0.18));
        ui.painter().rect_stroke(rect, 7.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 7.0, t.row_hover);
    }
    let fg = if !enabled {
        t.faint
    } else if active {
        t.accent
    } else {
        t.text
    };
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), icon, fg);
    resp.on_hover_text(tip)
}

/// Renders the single ~44pt Tablet Studio top bar.
pub fn show_top_bar(ui: &mut Ui, input: &StudioTopBarIn<'_>, search: &mut String, focus_search: &mut bool, t: &Tokens) -> StudioTopBarOut {
    let mut out = StudioTopBarOut::default();
    let search_open_id = Id::new("studio-search-open");
    let mut search_open = ui.ctx().data(|d| d.get_temp::<bool>(search_open_id)).unwrap_or(false) || *focus_search || !search.trim().is_empty();
    let frame = egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin { left: 10, right: 10, top: 5, bottom: 5 });
    frame.show(ui, |ui| {
        ui.set_min_height(36.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if studio_icon_btn(ui, Icon::Bars, input.drawer_open, true, crate::tl_id!("ui.studio_toggle_drawer"), t).clicked() {
                out.toggle_drawer = true;
            }
            if studio_icon_btn(ui, Icon::MoveUp, false, input.can_back, crate::tl_id!("ui.back"), t).clicked() {
                out.nav_back = true;
            }
            if studio_icon_btn(ui, Icon::MoveDown, false, input.can_forward, crate::tl_id!("ui.forward"), t).clicked() {
                out.nav_forward = true;
            }
            if studio_icon_btn(ui, Icon::Plus, false, true, crate::tl_id!("ui.add_page"), t).clicked() {
                out.add_page = true;
            }
            ui.separator();
            let crumb = if input.breadcrumb.trim().is_empty() { crate::tl_id!("ui.notebooks") } else { input.breadcrumb };
            ui.add(egui::Label::new(RichText::new(crumb).strong().color(t.text)).truncate());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let file_btn = ui
                    .add(egui::Button::new(RichText::new(crate::tl_id!("ui.file")).color(t.text)).min_size(vec2(46.0, 34.0)))
                    .on_hover_text(crate::tl_id!("ui.open_file_and_options"));
                if file_btn.clicked() {
                    out.acts.push(Act::File);
                }

                ui.menu_button(RichText::new(format!("{}%", input.view.zoom_percent)).color(t.text), |ui| {
                    crate::ribbon::mark_popup_open(ui.ctx());
                    ui.set_min_width(210.0);
                    ui.horizontal(|ui| {
                        if ui.button(crate::tl_id!("ui.zoom_out")).clicked() {
                            out.acts.push(Act::Zoom(-1));
                        }
                        // i18n-exempt: 100% is a numeric zoom reset value.
                        if ui.button("100%").clicked() {
                            out.acts.push(Act::Zoom(0));
                            ui.close();
                        }
                        if ui.button(crate::tl_id!("ui.zoom_in")).clicked() {
                            out.acts.push(Act::Zoom(1));
                        }
                    });
                    if ui.button(crate::tl_id!("ui.fit_all_content")).clicked() {
                        out.acts.push(Act::FitAllContent);
                        ui.close();
                    }
                    if ui.button(crate::tl_id!("ui.page_width")).clicked() {
                        out.acts.push(Act::PageWidth);
                        ui.close();
                    }
                    let mut minimap_on = input.view.show_minimap;
                    if ui.checkbox(&mut minimap_on, crate::tl_id!("ui.show_minimap")).changed() {
                        out.acts.push(Act::ToggleMinimap);
                    }
                    ui.separator();
                    let next_rules = match input.view.rules {
                        RuleLines::None => RuleLines::College,
                        RuleLines::College => RuleLines::GridMedium,
                        _ => RuleLines::None,
                    };
                    if ui.button(crate::tl_id!("ui.studio_cycle_paper_lines")).clicked() {
                        out.acts.push(Act::RuleLines(next_rules));
                    }
                    if ui.button(crate::tl_id!("ui.theme")).clicked() {
                        out.acts.push(Act::OpenThemes);
                        ui.close();
                    }
                    if ui
                        .button(if input.show_source { crate::tl_id!("ui.hide_the_page_source") } else { crate::tl_id!("ui.show_the_page_source") })
                        .clicked()
                    {
                        out.acts.push(Act::ToggleSource);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(crate::tl_id!("ui.switch_to_classic_ribbon")).clicked() {
                        out.acts.push(Act::ToggleStudioLayout);
                        ui.close();
                    }
                });

                let sync_label = if input.sync_busy {
                    crate::tl_id!("ui.syncing_short")
                } else if input.sync_configured {
                    crate::tl_id!("ui.sync_short")
                } else {
                    crate::tl_id!("ui.sync_setup_short")
                };
                let sync_btn = ui
                    .add(
                        egui::Button::new(RichText::new(sync_label).color(if input.sync_busy { t.accent } else { t.text }))
                            .min_size(vec2(58.0, 34.0)),
                    )
                    .on_hover_text(crate::tl_id!("ui.kdrive_sync_tip"));
                if sync_btn.clicked() {
                    out.acts.push(Act::OpenCloudSync);
                }

                if studio_icon_btn(ui, Icon::Search, search_open, true, crate::tl_id!("ui.search_all_pages"), t).clicked() {
                    if search_open {
                        search.clear();
                        search_open = false;
                    } else {
                        search_open = true;
                        *focus_search = true;
                        out.clear_page_caret = true;
                    }
                }
                if search_open {
                    let edit = ui.add(egui::TextEdit::singleline(search).hint_text(crate::tl_id!("ui.search_all_pages")).desired_width(155.0));
                    if std::mem::take(focus_search) {
                        edit.request_focus();
                        out.clear_page_caret = true;
                    }
                }
            });
        });
    });
    ui.ctx().data_mut(|d| d.insert_temp(search_open_id, search_open));
    let r = ui.min_rect();
    ui.painter().hline(r.x_range(), r.max.y, Stroke::new(1.0, t.container_border));
    out
}

/// Renders the floating Apple Pencil tool pill at `Order::Foreground` bottom-center of `view_rect`.
/// Returns `(acts, occupied_rect)`.
pub fn show_pencil_pill(ctx: &Context, view_rect: Rect, bottom_clearance: f32, view: &View<'_>, t: &Tokens) -> (Vec<Act>, Option<Rect>) {
    let mut acts = Vec::new();
    if view_rect.width() < 320.0 || view_rect.height() < 160.0 {
        return (acts, None);
    }
    let palette_id = Id::new("studio-nib-palette-open");
    let mut palette_for = ctx.data(|d| d.get_temp::<Option<usize>>(palette_id)).flatten();

    let anchor_y = (view_rect.max.y - 12.0 - bottom_clearance.max(0.0)).max(view_rect.min.y + 52.0);
    let center_x = view_rect.center().x;

    let area_resp = egui::Area::new(Id::new("studio-apple-pencil-pill"))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(center_x, anchor_y))
        .constrain(true)
        .movable(false)
        .show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(t.panel.gamma_multiply(0.97))
                .stroke(Stroke::new(1.0, t.container_border))
                .corner_radius(14.0)
                .inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 6 });
            frame.show(ui, |ui| {
                if let Some(nib_idx) = palette_for
                    && let Some(&nib) = view.rack.get(nib_idx)
                    && view.pointer == Pointer::Draw
                    && view.nib == nib_idx
                {
                    crate::ribbon::mark_popup_open(ui.ctx());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        for rgb in NIB_COLOURS {
                            let c = Color32::from_rgb(rgb.0, rgb.1, rgb.2);
                            let (sw, sr) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                            sr.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.pen_colour")));
                            ui.painter().circle_filled(sw.center(), 10.0, c);
                            if nib.colour == rgb {
                                ui.painter().circle_stroke(sw.center(), 11.5, Stroke::new(2.0, t.accent));
                            }
                            if sr.clicked() {
                                let mut updated = nib;
                                updated.colour = rgb;
                                acts.push(Act::InkRack(RackEdit::Set(nib_idx, updated)));
                            }
                        }
                        ui.separator();
                        for tenths in NIB_TENTHS {
                            let w = f32::from(tenths) / 10.0;
                            let chosen = nib.tenths.abs_diff(tenths) <= 3;
                            let (wr, ws) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::click());
                            ws.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.pen_thickness")));
                            if chosen {
                                ui.painter().rect_filled(wr, 5.0, t.accent.gamma_multiply(0.18));
                            }
                            ui.painter().circle_filled(wr.center(), (w * 0.9).clamp(1.5, 8.0), t.text);
                            if ws.clicked() {
                                let mut updated = nib;
                                updated.tenths = tenths;
                                acts.push(Act::InkRack(RackEdit::Set(nib_idx, updated)));
                            }
                        }
                    });
                    ui.separator();
                } else if palette_for.is_some() {
                    palette_for = None;
                }

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if studio_icon_btn(
                        ui,
                        Icon::DirLtr,
                        view.pointer == Pointer::Write,
                        true,
                        crate::tl_id!("ui.put_the_pointer_back_to_the_words_and_the_caret"),
                        t,
                    )
                    .clicked()
                    {
                        palette_for = None;
                        acts.push(Act::InkPointer(Pointer::Write));
                    }

                    let max_nibs = view.rack.len().min(4);
                    for i in 0..max_nibs {
                        let nib = view.rack[i];
                        let active = view.pointer == Pointer::Draw && view.nib == i;
                        let (nr, ns) = ui.allocate_exact_size(vec2(36.0, 34.0), Sense::click());
                        ns.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.pen")));
                        if active {
                            ui.painter().rect_filled(nr, 7.0, t.accent.gamma_multiply(0.18));
                            ui.painter().rect_stroke(nr, 7.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
                        } else if ns.hovered() {
                            ui.painter().rect_filled(nr, 7.0, t.row_hover);
                        }
                        let nib_col = Color32::from_rgb(nib.colour.0, nib.colour.1, nib.colour.2);
                        icons::paint(
                            ui.painter(),
                            Rect::from_center_size(pos2(nr.center().x, nr.center().y - 3.0), vec2(16.0, 16.0)),
                            Icon::Pen,
                            nib_col,
                        );
                        ui.painter().line_segment(
                            [pos2(nr.min.x + 7.0, nr.max.y - 5.0), pos2(nr.max.x - 7.0, nr.max.y - 5.0)],
                            Stroke::new((nib.width() * 0.45).clamp(1.5, 4.0), nib_col),
                        );
                        if ns.clicked() {
                            if active {
                                palette_for = if palette_for == Some(i) { None } else { Some(i) };
                            } else {
                                palette_for = None;
                                acts.push(Act::InkPen(i));
                            }
                        }
                    }

                    if studio_icon_btn(ui, Icon::Eraser, view.pointer == Pointer::Erase, true, crate::tl_id!("ui.toggle_eraser_tip"), t).clicked() {
                        palette_for = None;
                        acts.push(Act::InkPointer(Pointer::Erase));
                    }
                    let auto_btn = ui
                        .add(
                            egui::Button::new(RichText::new(crate::tl_id!("ui.eraser_auto_short")).small().color(if view.eraser_auto_return {
                                t.accent
                            } else {
                                t.muted
                            }))
                            .selected(view.eraser_auto_return)
                            .min_size(vec2(38.0, 34.0)),
                        )
                        .on_hover_text(crate::tl_id!("ui.eraser_auto_return_tip"));
                    if auto_btn.clicked() {
                        acts.push(Act::ToggleEraserAutoReturn);
                    }

                    if studio_icon_btn(ui, Icon::Lasso, view.pointer == Pointer::Lasso, true, crate::tl_id!("ui.lasso_select"), t).clicked() {
                        palette_for = None;
                        acts.push(Act::InkPointer(Pointer::Lasso));
                    }
                    if studio_icon_btn(ui, Icon::Pointer, view.pointer == Pointer::Pick, true, crate::tl_id!("ui.select"), t).clicked() {
                        palette_for = None;
                        acts.push(Act::InkPointer(Pointer::Pick));
                    }
                    let shape_active = matches!(view.pointer, Pointer::Shape(_));
                    if studio_icon_btn(ui, Icon::Shapes, shape_active, true, crate::tl_id!("ui.shapes"), t).clicked() {
                        palette_for = None;
                        acts.push(Act::InkPointer(if shape_active { Pointer::Draw } else { Pointer::Shape(Shape::Rectangle) }));
                    }
                    if studio_icon_btn(ui, Icon::Hand, view.finger_drawing, true, crate::tl_id!("ui.finger_drawing_tip"), t).clicked() {
                        acts.push(Act::ToggleFingerDrawing);
                    }
                    ui.separator();
                    if studio_icon_btn(ui, Icon::Undo, false, view.can_undo, crate::tl_id!("ui.undo"), t).clicked() {
                        acts.push(Act::Undo);
                    }
                    if studio_icon_btn(ui, Icon::Redo, false, view.can_redo || view.can_ink_redo, crate::tl_id!("ui.redo"), t).clicked() {
                        acts.push(Act::Redo);
                    }
                });
            });
        });

    ctx.data_mut(|d| d.insert_temp(palette_id, palette_for));
    (acts, Some(area_resp.response.rect))
}

/// Shows a non-blocking transient toast banner when Tablet Studio hides the permanent status bar.
pub fn show_transient_notice(ctx: &Context, view_rect: Rect, message: &str, is_error: bool, tokens: &Tokens) {
    if message.trim().is_empty() {
        return;
    }
    let pos = pos2(view_rect.center().x, (view_rect.min.y + 12.0).max(52.0));
    egui::Area::new(Id::new("studio-transient-notice")).order(Order::Tooltip).pivot(Align2::CENTER_TOP).fixed_pos(pos).interactable(false).show(
        ctx,
        |ui| {
            let stroke_col = if is_error { tokens.warn } else { tokens.container_border };
            let text_col = if is_error { tokens.warn } else { tokens.text };
            egui::Frame::NONE
                .fill(tokens.panel.gamma_multiply(0.96))
                .stroke(Stroke::new(1.0, stroke_col))
                .corner_radius(8.0)
                .inner_margin(egui::Margin { left: 12, right: 12, top: 6, bottom: 6 })
                .show(ui, |ui| {
                    ui.label(RichText::new(message).small().color(text_col));
                });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_layout_uses_tablet_studio_on_ipad_and_classic_ribbon_on_desktop() {
        assert!(UiLayoutMode::Auto.is_tablet_studio(OperatingSystem::IOS, false));
        assert!(!UiLayoutMode::Auto.is_tablet_studio(OperatingSystem::IOS, true), "phone compact (<600pt) uses compact layout");
        assert!(!UiLayoutMode::Auto.is_tablet_studio(OperatingSystem::Windows, false));
        assert!(!UiLayoutMode::Auto.is_tablet_studio(OperatingSystem::Mac, false));
        assert!(UiLayoutMode::TabletStudio.is_tablet_studio(OperatingSystem::Windows, false));
        assert!(!UiLayoutMode::ClassicRibbon.is_tablet_studio(OperatingSystem::IOS, false));
    }
}
