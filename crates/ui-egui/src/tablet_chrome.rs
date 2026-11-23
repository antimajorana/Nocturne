//! Dedicated iPadOS / Tablet Studio chrome: floating top-left header pill, classic-tree top
//! bar, title-clearance calculation, and one-line failure banner.

use crate::icons::{self, Icon};
use crate::ribbon::{Act, RuleLines, View};
use crate::theme::{RADIUS_PILL, RADIUS_POPUP, Tokens};
use egui::os::OperatingSystem;
use egui::{Align2, Color32, Context, CornerRadius, Id, Order, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use nocturne_engine::{LibraryView, ToolbarDock};
use nocturne_ink::Rgb;

pub use crate::tablet_dock::{nearest_dock, show_pencil_pill};
pub use crate::tablet_nibs::{
    ACTIVE_NIB_RAISE, DOCK_NIB_ALONG, DOCK_NIB_CROSS, NibColors, nib_colors_for_theme, nib_slot_size, nib_visual_rect, paint_upright_nib,
};

pub const UI_LAYOUT_KEY: &str = "ui_layout_mode";

/// How far floating Tablet Studio chrome keeps from the edge of the page view.
pub(crate) const DOCK_MARGIN: f32 = 12.0;

pub const NIB_COLOURS: [Rgb; 8] = [
    Rgb(0x20, 0x20, 0x20),
    Rgb(0x18, 0x55, 0xc4),
    Rgb(0xd0, 0x21, 0x21),
    Rgb(0x16, 0x7f, 0x3d),
    Rgb(0xdb, 0x7c, 0x16),
    Rgb(0x84, 0x3a, 0xc4),
    Rgb(0xff, 0xdd, 0x33),
    Rgb(0x6f, 0xe3, 0x9a),
];

fn panel_is_dark(panel: Color32) -> bool {
    let luma = 0.2126 * f32::from(panel.r()) + 0.7152 * f32::from(panel.g()) + 0.0722 * f32::from(panel.b());
    luma < 128.0
}

#[must_use]
pub fn shown_nib_color(colour: Rgb, panel: Color32) -> Color32 {
    let adj = crate::inkpad::shown_colour(colour, panel_is_dark(panel));
    Color32::from_rgb(adj.0, adj.1, adj.2)
}

/// Vertical room reserved at the top of the scrollable page sheet in Tablet Studio so the
/// floating top-left header pill (and a top-docked tool bar, if chosen) never covers the page
/// title at rest, while keeping `page_view_rect` borderless to the top of the window.
#[must_use]
pub fn canvas_top_clearance(view_rect: Rect, header_pill: Option<Rect>, pencil_pill: Option<Rect>, dock: ToolbarDock) -> f32 {
    let mut clear_to = view_rect.min.y;
    if let Some(h) = header_pill {
        clear_to = clear_to.max(h.max.y + 8.0);
    }
    if dock == ToolbarDock::Top {
        let fallback = header_pill.map_or(view_rect.min.y + 126.0, |h| h.max.y + 70.0);
        let bar_bottom = pencil_pill.filter(|p| p.center().y < view_rect.center().y).map_or(fallback, |p| p.max.y + 8.0);
        clear_to = clear_to.max(bar_bottom);
    }
    (clear_to - view_rect.min.y).max(0.0)
}

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
    pub library_view: LibraryView,
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
    pub back_to_library: bool,
    pub nav_back: bool,
    pub nav_forward: bool,
    pub add_page: bool,
    pub clear_page_caret: bool,
    pub acts: Vec<Act>,
}

/// Paints hover, active, pressed, and keyboard-focus background/outline chrome for a 40×40
/// Tablet Studio icon button.
pub(crate) fn paint_studio_btn_chrome(ui: &Ui, rect: Rect, resp: &egui::Response, active: bool, enabled: bool, t: &Tokens) {
    let pressed = enabled && resp.is_pointer_button_down_on();
    let hovered = enabled && resp.hovered();
    let r = CornerRadius::same(RADIUS_POPUP);
    if pressed {
        ui.painter().rect_filled(rect, r, t.row_selected);
        ui.painter().rect_stroke(rect, r, Stroke::new(1.5, t.accent), StrokeKind::Inside);
    } else if active {
        ui.painter().rect_filled(rect, r, t.row_selected);
        ui.painter().rect_stroke(rect, r, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else if hovered {
        ui.painter().rect_filled(rect, r, t.row_hover);
        ui.painter().rect_stroke(rect, r, Stroke::new(1.0, t.border_subtle), StrokeKind::Inside);
    }
    if enabled && resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(1.5), CornerRadius::same(RADIUS_POPUP + 2), Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
}

// i18n-ui-text: tip
pub(crate) fn studio_icon_btn(ui: &mut Ui, icon: Icon, active: bool, enabled: bool, tip: &str, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), if enabled { Sense::click() } else { Sense::hover() });
    let tip_owned = tip.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &tip_owned));
    paint_studio_btn_chrome(ui, rect, &resp, active, enabled, t);
    let fg = if !enabled {
        t.disabled
    } else if active || resp.is_pointer_button_down_on() {
        t.accent
    } else {
        t.text
    };
    icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), icon, fg);
    resp.on_hover_text(tip)
}

// i18n-ui-text: tip
fn studio_chevron_btn(ui: &mut Ui, right: bool, enabled: bool, tip: &str, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), if enabled { Sense::click() } else { Sense::hover() });
    let tip_owned = tip.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &tip_owned));
    paint_studio_btn_chrome(ui, rect, &resp, false, enabled, t);
    let fg = if !enabled {
        t.disabled
    } else if resp.is_pointer_button_down_on() {
        t.accent
    } else {
        t.text
    };
    let c = rect.center();
    let dir = if right { 1.0 } else { -1.0 };
    let stroke = Stroke::new(2.0, fg);
    ui.painter().line_segment([pos2(c.x - 3.0 * dir, c.y - 6.0), pos2(c.x + 3.0 * dir, c.y)], stroke);
    ui.painter().line_segment([pos2(c.x + 3.0 * dir, c.y), pos2(c.x - 3.0 * dir, c.y + 6.0)], stroke);
    resp.on_hover_text(tip)
}

/// Renders the single ~50pt Tablet Studio top bar (used in ClassicTree mode when no page is open).
pub fn show_top_bar(ui: &mut Ui, input: &StudioTopBarIn<'_>, search: &mut String, focus_search: &mut bool, t: &Tokens) -> StudioTopBarOut {
    let mut out = StudioTopBarOut::default();
    let search_open_id = Id::unique("studio-search-open");
    let mut search_open = ui.ctx().data(|d| d.get_temp::<bool>(search_open_id)).unwrap_or(false) || *focus_search || !search.trim().is_empty();
    let frame = egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin { left: 10, right: 10, top: 5, bottom: 5 });
    frame.show(ui, |ui| {
        ui.set_min_height(40.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.spacing_mut().interact_size = vec2(40.0, 40.0);
            if studio_icon_btn(ui, Icon::Bars, input.drawer_open, true, crate::tl_id!("ui.studio_toggle_drawer"), t).clicked() {
                out.toggle_drawer = true;
            }
            if studio_chevron_btn(ui, false, input.can_back, crate::tl_id!("ui.back"), t).clicked() {
                out.nav_back = true;
            }
            if studio_chevron_btn(ui, true, input.can_forward, crate::tl_id!("ui.forward"), t).clicked() {
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
                ui.spacing_mut().interact_size = vec2(40.0, 40.0);
                let file_btn = ui
                    .add(egui::Button::new(RichText::new(crate::tl_id!("ui.file")).color(t.text)).min_size(vec2(48.0, 40.0)))
                    .on_hover_text(crate::tl_id!("ui.open_file_and_options"));
                if file_btn.clicked() {
                    out.acts.push(Act::File);
                }

                ui.menu_button(RichText::new(format!("{}%", input.view.zoom_percent)).color(t.text), |ui| {
                    view_menu(ui, input, &mut out);
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
                        egui::Button::new(RichText::new(sync_label).color(if input.sync_busy { t.accent_text } else { t.text }))
                            .min_size(vec2(60.0, 40.0)),
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
                    let edit = ui.add(
                        egui::TextEdit::singleline(search)
                            .id(Id::unique(crate::app::SEARCH_FIELD))
                            .hint_text(crate::tl_id!("ui.search_all_pages"))
                            .desired_width(155.0),
                    );
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
    ui.painter().hline(r.x_range(), r.max.y, Stroke::new(1.0, t.border));
    out
}

/// The narrowest the failure banner is drawn.
pub const ERROR_BANNER_MIN_WIDTH: f32 = 240.0;

/// Shows a failure on one line at the top of the view, clearing any visible floating header pill,
/// until tapped away or superseded.
pub fn show_error_banner(ctx: &Context, view_rect: Rect, message: &str, is_error: bool, tokens: &Tokens) -> bool {
    let message = message.trim();
    if message.is_empty() {
        return false;
    }
    let margin = egui::Margin { left: 12, right: 12, top: 7, bottom: 7 };
    let widest = (view_rect.width() - 32.0).max(ERROR_BANNER_MIN_WIDTH);
    let header_clear = ctx
        .memory(|m| {
            let id = Id::unique("studio-header-pill");
            m.areas().visible_layer_ids().iter().any(|l| l.id == id).then(|| m.area_rect(id)).flatten()
        })
        .map_or(view_rect.min.y + 56.0, |h| h.max.y + 8.0);
    let y = (view_rect.min.y + 12.0).max(header_clear).min((view_rect.max.y - 52.0).max(view_rect.min.y + 12.0));
    let pos = pos2(view_rect.center().x, y);
    let mut dismissed = false;
    egui::Area::new(Id::unique("studio-error-banner")).order(Order::Foreground).pivot(Align2::CENTER_TOP).fixed_pos(pos).constrain(true).show(
        ctx,
        |ui| {
            let frame = egui::Frame::NONE
                .fill(tokens.surface_elevated)
                .stroke(Stroke::new(1.0, if is_error { tokens.warn } else { tokens.border }))
                .corner_radius(CornerRadius::same(RADIUS_POPUP))
                .shadow(tokens.shadow_popup())
                .inner_margin(margin)
                .show(ui, |ui| {
                    ui.set_min_width(ERROR_BANNER_MIN_WIDTH - f32::from(margin.left + margin.right));
                    ui.set_max_width(widest - f32::from(margin.left + margin.right));
                    ui.add(
                        egui::Label::new(RichText::new(message).color(if is_error { tokens.warn } else { tokens.text })).truncate().selectable(false),
                    );
                });
            let tap = ui.interact(frame.response.rect, Id::unique("studio-error-banner-tap"), Sense::click());
            if tap.is_pointer_button_down_on() {
                ui.painter().rect_stroke(frame.response.rect, CornerRadius::same(RADIUS_POPUP), Stroke::new(1.5, tokens.accent), StrokeKind::Inside);
            }
            if tap.has_focus() {
                ui.painter().rect_stroke(
                    frame.response.rect.expand(1.5),
                    CornerRadius::same(RADIUS_POPUP + 2),
                    Stroke::new(2.0, tokens.accent),
                    StrokeKind::Outside,
                );
            }
            tap.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, message));
            dismissed = tap.clicked();
        },
    );
    dismissed
}

/// Shared view menu for the top bar and floating header pill.
fn view_menu(ui: &mut Ui, input: &StudioTopBarIn<'_>, out: &mut StudioTopBarOut) {
    crate::ribbon::mark_popup_open(ui.ctx());
    ui.set_min_width(210.0);
    ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(36.0);
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
    if ui.button(if input.show_source { crate::tl_id!("ui.hide_the_page_source") } else { crate::tl_id!("ui.show_the_page_source") }).clicked() {
        out.acts.push(Act::ToggleSource);
        ui.close();
    }
    ui.separator();
    let (next_library_view, view_label) = match input.library_view {
        LibraryView::StudioGrid => (LibraryView::ClassicTree, crate::tl_id!("ui.use_classic_tree")),
        LibraryView::ClassicTree => (LibraryView::StudioGrid, crate::tl_id!("ui.use_library_grid")),
    };
    if ui.button(view_label).clicked() {
        out.acts.push(Act::LibraryView(next_library_view));
        ui.close();
    }
    if ui.button(crate::tl_id!("ui.switch_to_classic_ribbon")).clicked() {
        out.acts.push(Act::ToggleStudioLayout);
        ui.close();
    }
}

/// The floating header of an open page in Tablet Studio at the top left of `view_rect`.
pub fn show_header_pill(
    ctx: &Context,
    view_rect: Rect,
    input: &StudioTopBarIn<'_>,
    search: &mut String,
    focus_search: &mut bool,
    t: &Tokens,
) -> (StudioTopBarOut, Rect) {
    let mut out = StudioTopBarOut::default();
    let search_open_id = Id::unique("studio-search-open");
    let mut search_open = ctx.data(|d| d.get_temp::<bool>(search_open_id)).unwrap_or(false) || *focus_search || !search.trim().is_empty();
    let area = egui::Area::new(Id::unique("studio-header-pill"))
        .order(Order::Foreground)
        .fixed_pos(pos2(view_rect.min.x + DOCK_MARGIN, view_rect.min.y + DOCK_MARGIN))
        .constrain(true)
        .show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(t.surface_elevated)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(RADIUS_PILL))
                .shadow(t.shadow_floating())
                .inner_margin(egui::Margin { left: 10, right: 10, top: 5, bottom: 5 });
            frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.spacing_mut().interact_size = vec2(40.0, 40.0);
                    if input.library_view == LibraryView::StudioGrid {
                        if studio_chevron_btn(ui, false, true, crate::tl_id!("ui.back_to_library"), t).clicked() {
                            out.back_to_library = true;
                        }
                    } else if studio_icon_btn(ui, Icon::Bars, input.drawer_open, true, crate::tl_id!("ui.studio_toggle_drawer"), t).clicked() {
                        out.toggle_drawer = true;
                    }
                    if studio_icon_btn(ui, Icon::Plus, false, true, crate::tl_id!("ui.add_page"), t).clicked() {
                        out.add_page = true;
                    }
                    let sync_label = if input.sync_busy {
                        crate::tl_id!("ui.syncing_short")
                    } else if input.sync_configured {
                        crate::tl_id!("ui.sync_short")
                    } else {
                        crate::tl_id!("ui.sync_setup_short")
                    };
                    let sync = egui::Button::new(RichText::new(sync_label).color(if input.sync_busy { t.accent_text } else { t.text }))
                        .min_size(vec2(56.0, 40.0));
                    if ui.add(sync).clicked() {
                        out.acts.push(Act::OpenCloudSync);
                    }
                    if search_open {
                        let edit = ui.add(
                            egui::TextEdit::singleline(search)
                                .id(Id::unique(crate::app::SEARCH_FIELD))
                                .hint_text(crate::tl_id!("ui.search_all_pages"))
                                .desired_width(170.0),
                        );
                        if std::mem::take(focus_search) {
                            edit.request_focus();
                            out.clear_page_caret = true;
                        }
                    }
                    ui.menu_button(RichText::new(crate::tl_id!("ui.more")).color(t.text), |ui| {
                        crate::ribbon::mark_popup_open(ui.ctx());
                        ui.set_min_width(210.0);
                        ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(36.0);
                        if ui.add_enabled(input.can_back, egui::Button::new(crate::tl_id!("ui.back"))).clicked() {
                            out.nav_back = true;
                            ui.close();
                        }
                        if ui.add_enabled(input.can_forward, egui::Button::new(crate::tl_id!("ui.forward"))).clicked() {
                            out.nav_forward = true;
                            ui.close();
                        }
                        if ui.button(crate::tl_id!("ui.search_all_pages")).clicked() {
                            if search_open {
                                search.clear();
                                search_open = false;
                            } else {
                                search_open = true;
                                *focus_search = true;
                                out.clear_page_caret = true;
                            }
                            ui.close();
                        }
                        if ui.button(crate::tl_id!("ui.file")).clicked() {
                            out.acts.push(Act::File);
                            ui.close();
                        }
                        if ui.button(crate::tl_id!("ui.settings")).clicked() {
                            out.acts.push(Act::Options);
                            ui.close();
                        }
                        if ui.button(crate::tl_id!("ui.tool_settings")).clicked() {
                            out.acts.push(Act::ToggleToolSettings);
                            ui.close();
                        }
                        ui.separator();
                        view_menu(ui, input, &mut out);
                    });
                });
            });
        });
    ctx.data_mut(|d| d.insert_temp(search_open_id, search_open));
    (out, area.response.rect)
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

    #[test]
    fn canvas_top_clearance_reserves_room_below_header_and_top_docked_bar() {
        let view = Rect::from_min_max(pos2(0.0, 0.0), pos2(1180.0, 820.0));
        let header = Rect::from_min_max(pos2(12.0, 12.0), pos2(228.0, 56.0));
        let bottom_bar = Rect::from_min_max(pos2(300.0, 754.0), pos2(880.0, 808.0));
        let top_bar = Rect::from_min_max(pos2(300.0, 62.0), pos2(880.0, 116.0));

        assert!((canvas_top_clearance(view, Some(header), Some(bottom_bar), ToolbarDock::Bottom) - 64.0).abs() < 0.01);
        assert!((canvas_top_clearance(view, Some(header), Some(top_bar), ToolbarDock::Top) - 124.0).abs() < 0.01);
        assert!(canvas_top_clearance(view, Some(header), Some(bottom_bar), ToolbarDock::Top) >= top_bar.max.y + 8.0);
    }
}
