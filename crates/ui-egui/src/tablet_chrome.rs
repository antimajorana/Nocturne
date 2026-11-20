//! Dedicated iPadOS / Tablet Studio chrome: single ~44pt top bar, floating Apple Pencil tool
//! pill (`Order::Foreground`), and the one-line banner that shows a failure until it is tapped.

use crate::icons::{self, Icon};
use crate::ribbon::{Act, Pointer, RackEdit, RuleLines, View};
use crate::theme::Tokens;
use egui::os::OperatingSystem;
use egui::{Align2, Color32, Context, Id, Order, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use nocturne_ink::{Rgb, Tool, rack, shapes::Shape};

pub const UI_LAYOUT_KEY: &str = "ui_layout_mode";

const NIB_COLOURS: [Rgb; 8] = [
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

fn shown_nib_color(colour: Rgb, panel: Color32) -> Color32 {
    let adj = crate::inkpad::shown_colour(colour, panel_is_dark(panel));
    Color32::from_rgb(adj.0, adj.1, adj.2)
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
                    // Here until the Tool Settings screen exists. It is a labelled switch and not
                    // an icon on the tool pill: the hand that stood for it there means "pan" on the
                    // desktop, and said neither.
                    let mut finger_on = input.view.finger_drawing;
                    if ui.checkbox(&mut finger_on, crate::tl_id!("ui.draw_with_finger")).changed() {
                        out.acts.push(Act::ToggleFingerDrawing);
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
                    let edit = ui.add(
                        egui::TextEdit::singleline(search)
                            .id(Id::new(crate::app::SEARCH_FIELD))
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
    let shapes_open_id = Id::new("studio-shapes-palette-open");
    let mut palette_for = ctx.data(|d| d.get_temp::<Option<usize>>(palette_id)).flatten();
    let mut shapes_open = ctx.data(|d| d.get_temp::<bool>(shapes_open_id)).unwrap_or(false);
    let eraser_open_id = Id::new("studio-eraser-options-open");
    let mut eraser_open = ctx.data(|d| d.get_temp::<bool>(eraser_open_id)).unwrap_or(false) && view.pointer == Pointer::Erase;
    if !matches!(view.pointer, Pointer::Shape(_)) {
        shapes_open = false;
    }

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
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for tool in Tool::ALL {
                            let chosen_tool = nib.tool == tool;
                            let tool_label = crate::tl!(tool.label());
                            let btn = ui.add(
                                egui::Button::new(RichText::new(tool_label).small().color(if chosen_tool { t.accent } else { t.text }))
                                    .selected(chosen_tool)
                                    .min_size(vec2(48.0, 26.0)),
                            );
                            if btn.clicked() && !chosen_tool {
                                let mut updated = nib;
                                updated.tool = tool;
                                if tool == Tool::Highlighter && updated.tenths < 80 {
                                    updated.tenths = 140;
                                } else if tool != Tool::Highlighter && updated.tenths > 100 {
                                    updated.tenths = 30;
                                }
                                acts.push(Act::InkRack(RackEdit::Set(nib_idx, updated)));
                            }
                        }
                    });
                    ui.add_space(3.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let ring_edge = crate::theme::mix(t.panel, t.text, 0.28);
                        for rgb in NIB_COLOURS {
                            let c = shown_nib_color(rgb, t.panel);
                            let (sw, sr) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                            sr.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.pen_colour")));
                            ui.painter().circle_filled(sw.center(), 9.5, c);
                            ui.painter().circle_stroke(sw.center(), 9.5, Stroke::new(0.8, ring_edge));
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
                        for tenths in rack::THICKNESSES {
                            let w = f32::from(tenths) / 10.0;
                            let chosen = nib.tenths.abs_diff(tenths) <= 3;
                            let (wr, ws) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                            ws.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.pen_thickness")));
                            if chosen {
                                ui.painter().rect_filled(wr, 5.0, t.accent.gamma_multiply(0.18));
                            }
                            ui.painter().circle_filled(wr.center(), (w * 0.5).clamp(1.2, 7.5), t.text);
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

                if eraser_open {
                    crate::ribbon::mark_popup_open(ui.ctx());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (pixel, label) in [(false, crate::tl_id!("ui.object_eraser")), (true, crate::tl_id!("ui.pixel_eraser"))] {
                            let chosen = view.eraser_pixel == pixel;
                            let button = egui::Button::new(RichText::new(label).small().color(if chosen { t.accent } else { t.text }))
                                .selected(chosen)
                                .min_size(vec2(86.0, 28.0));
                            if ui.add(button).clicked() && !chosen {
                                acts.push(Act::EraserPixel(pixel));
                            }
                        }
                        ui.separator();
                        for (name, width) in nocturne_engine::ERASER_SIZES {
                            let chosen = (view.eraser_width - width).abs() < 0.05;
                            let size = name.to_ascii_uppercase();
                            let label = crate::i18n::format_message_id("ui.eraser_size_named", &[("size", &size)]);
                            let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 28.0), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, chosen, &label));
                            if chosen {
                                ui.painter().rect_filled(rect, 6.0, t.accent.gamma_multiply(0.18));
                                ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.2, t.accent), StrokeKind::Inside);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, 6.0, t.row_hover);
                            }
                            // A dot as wide as the eraser, as far as the button has room for it.
                            ui.painter().circle_filled(rect.center(), (width / 4.0).clamp(1.5, 11.0), if chosen { t.accent } else { t.text });
                            if resp.clicked() {
                                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                                acts.push(Act::EraserWidthTenths((width * 10.0).round() as u16));
                            }
                        }
                    });
                    ui.separator();
                }

                if shapes_open && let Pointer::Shape(current_shape) = view.pointer {
                    crate::ribbon::mark_popup_open(ui.ctx());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for shape in Shape::ALL {
                            let here = current_shape == shape;
                            let label = crate::tl!(shape.label());
                            let (sr, resp) = ui.allocate_exact_size(vec2(30.0, 28.0), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, here, label));
                            if here {
                                ui.painter().rect_filled(sr, 6.0, t.accent.gamma_multiply(0.18));
                                ui.painter().rect_stroke(sr, 6.0, Stroke::new(1.2, t.accent), StrokeKind::Inside);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(sr, 6.0, t.row_hover);
                            }
                            let box_ = Rect::from_center_size(sr.center(), vec2(15.0, 15.0));
                            let dabs = nocturne_ink::shapes::trace(shape, (box_.min.x, box_.min.y), (box_.max.x, box_.max.y), 1.0);
                            let pts: Vec<egui::Pos2> = dabs.iter().map(|d| pos2(d.x, d.y)).collect();
                            if pts.len() >= 2 {
                                ui.painter().add(egui::Shape::line(pts, Stroke::new(1.4, if here { t.accent } else { t.text })));
                            }
                            if resp.on_hover_text(label).clicked() {
                                acts.push(Act::InkPointer(Pointer::Shape(shape)));
                            }
                        }
                    });
                    ui.separator();
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
                        shapes_open = false;
                        acts.push(Act::InkPointer(Pointer::Write));
                    }

                    // Scale displayed rack slots to the available canvas width so portrait / Split View
                    // never overflows, while showing up to 8 slots (all 7 default tools + 1) on full iPad width.
                    let slot_cap = if view_rect.width() < 560.0 {
                        3
                    } else if view_rect.width() < 740.0 {
                        5
                    } else {
                        8
                    };
                    let max_nibs = view.rack.len().min(slot_cap);
                    let border_mix = crate::theme::mix(t.panel, t.text, 0.30);
                    for i in 0..max_nibs {
                        let nib = view.rack[i];
                        let active = view.pointer == Pointer::Draw && view.nib == i;
                        let tool_label = crate::tl!(nib.tool.label());
                        let (nr, ns) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
                        ns.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, tool_label));
                        if active {
                            ui.painter().rect_filled(nr, 7.0, t.accent.gamma_multiply(0.18));
                            ui.painter().rect_stroke(nr, 7.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
                        } else if ns.hovered() {
                            ui.painter().rect_filled(nr, 7.0, t.row_hover);
                        }
                        let nib_col = shown_nib_color(nib.colour, t.panel);
                        match nib.tool {
                            Tool::Highlighter => {
                                let band = Rect::from_center_size(pos2(nr.center().x, nr.center().y - 2.0), vec2(18.0, 12.0));
                                ui.painter().rect_filled(band, 3.0, nib_col.gamma_multiply(0.65));
                                ui.painter().rect_stroke(band, 3.0, Stroke::new(0.9, border_mix), StrokeKind::Inside);
                                let tip = Rect::from_min_max(pos2(nr.min.x + 6.0, nr.max.y - 7.0), pos2(nr.max.x - 6.0, nr.max.y - 3.0));
                                ui.painter().rect_filled(tip, 1.5, nib_col);
                            }
                            Tool::Pencil => {
                                icons::paint(
                                    ui.painter(),
                                    Rect::from_center_size(pos2(nr.center().x, nr.center().y - 3.0), vec2(15.0, 15.0)),
                                    Icon::Pen,
                                    nib_col,
                                );
                                for dx in [-7.0_f32, -2.0, 3.0] {
                                    ui.painter().line_segment(
                                        [pos2(nr.center().x + dx, nr.max.y - 5.0), pos2(nr.center().x + dx + 3.0, nr.max.y - 5.0)],
                                        Stroke::new(1.8, nib_col),
                                    );
                                }
                            }
                            _ => {
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
                            }
                        }
                        if ns.on_hover_text(tool_label).clicked() {
                            shapes_open = false;
                            if active {
                                palette_for = if palette_for == Some(i) { None } else { Some(i) };
                            } else {
                                palette_for = None;
                                acts.push(Act::InkPen(i));
                            }
                        }
                    }

                    // The first tap picks the eraser up; a tap on the eraser already in hand opens
                    // its choices (object or pixel, and its size) above the pill.
                    if studio_icon_btn(ui, Icon::Eraser, view.pointer == Pointer::Erase, true, crate::tl_id!("ui.toggle_eraser_tip"), t).clicked() {
                        palette_for = None;
                        shapes_open = false;
                        if view.pointer == Pointer::Erase {
                            eraser_open = !eraser_open;
                        } else {
                            eraser_open = false;
                            acts.push(Act::InkPointer(Pointer::Erase));
                        }
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
                        shapes_open = false;
                        acts.push(Act::InkPointer(Pointer::Lasso));
                    }
                    if studio_icon_btn(ui, Icon::Pointer, view.pointer == Pointer::Pick, true, crate::tl_id!("ui.select"), t).clicked() {
                        palette_for = None;
                        shapes_open = false;
                        acts.push(Act::InkPointer(Pointer::Pick));
                    }
                    let shape_active = matches!(view.pointer, Pointer::Shape(_));
                    if studio_icon_btn(ui, Icon::Shapes, shape_active, true, crate::tl_id!("ui.shapes"), t).clicked() {
                        palette_for = None;
                        if shape_active {
                            shapes_open = !shapes_open;
                        } else {
                            shapes_open = true;
                            acts.push(Act::InkPointer(Pointer::Shape(Shape::Rectangle)));
                        }
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

    ctx.data_mut(|d| {
        d.insert_temp(palette_id, palette_for);
        d.insert_temp(shapes_open_id, shapes_open);
        d.insert_temp(eraser_open_id, eraser_open);
    });
    (acts, Some(area_resp.response.rect))
}

/// The narrowest the failure banner is drawn.
pub const ERROR_BANNER_MIN_WIDTH: f32 = 240.0;

/// Shows a failure on one line at the top of the view. The caller draws it until it is tapped
/// or a message that is not a failure takes its place. Returns true when it was tapped away
/// this frame.
///
/// Tablet Studio has no status bar. It used to float every status message over the page in a box
/// with no width of its own, which wrapped the words a few letters to a line. Routine messages
/// ("Drawing saved, 2 strokes") are no longer shown at all; only a failure is, and it has a
/// width: at least [`ERROR_BANNER_MIN_WIDTH`], at most the view, cut short with an ellipsis.
pub fn show_error_banner(ctx: &Context, view_rect: Rect, message: &str, tokens: &Tokens) -> bool {
    let message = message.trim();
    if message.is_empty() {
        return false;
    }
    let margin = egui::Margin { left: 12, right: 12, top: 7, bottom: 7 };
    let widest = (view_rect.width() - 32.0).max(ERROR_BANNER_MIN_WIDTH);
    let pos = pos2(view_rect.center().x, (view_rect.min.y + 12.0).max(52.0));
    let mut dismissed = false;
    egui::Area::new(Id::new("studio-error-banner")).order(Order::Foreground).pivot(Align2::CENTER_TOP).fixed_pos(pos).constrain(true).show(
        ctx,
        |ui| {
            let frame = egui::Frame::NONE
                .fill(tokens.panel.gamma_multiply(0.97))
                .stroke(Stroke::new(1.0, tokens.warn))
                .corner_radius(8.0)
                .inner_margin(margin)
                .show(ui, |ui| {
                    ui.set_min_width(ERROR_BANNER_MIN_WIDTH - f32::from(margin.left + margin.right));
                    ui.set_max_width(widest - f32::from(margin.left + margin.right));
                    ui.add(egui::Label::new(RichText::new(message).color(tokens.warn)).truncate().selectable(false));
                });
            let tap = ui.interact(frame.response.rect, Id::new("studio-error-banner-tap"), Sense::click());
            tap.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, message));
            dismissed = tap.clicked();
        },
    );
    dismissed
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
