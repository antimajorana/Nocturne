//! Floating Apple Pencil dock pill, four-edge drag grip, and tool option popovers for
//! Tablet Studio.

use crate::icons::Icon;
use crate::ribbon::{Act, Pointer, RackEdit, View};
use crate::tablet_chrome::{DOCK_MARGIN, NIB_COLOURS, shown_nib_color, studio_icon_btn};
use crate::tablet_nibs::{nib_colors_for_theme, nib_slot_size, nib_visual_rect, paint_upright_nib};
use crate::theme::Tokens;
use egui::{Align2, Context, Id, Order, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use nocturne_engine::{DockTool, ToolbarDock};
use nocturne_ink::{Tool, rack, shapes::Shape};

/// Renders the floating Apple Pencil tool pill at `Order::Foreground` along the chosen edge of
/// `view_rect`. Returns `(acts, occupied_rect)`.
pub fn show_pencil_pill(
    ctx: &Context,
    view_rect: Rect,
    edge_clearance: f32,
    studio: nocturne_engine::StudioOptions,
    settings_open: bool,
    view: &View<'_>,
    t: &Tokens,
) -> (Vec<Act>, Option<Rect>) {
    let mut acts = Vec::new();
    if view_rect.width() < 320.0 || view_rect.height() < 160.0 {
        return (acts, None);
    }
    let palette_id = Id::unique("studio-nib-palette-open");
    let shapes_open_id = Id::unique("studio-shapes-palette-open");
    let mut palette_for = ctx.data(|d| d.get_temp::<Option<usize>>(palette_id)).flatten();
    let mut shapes_open = ctx.data(|d| d.get_temp::<bool>(shapes_open_id)).unwrap_or(false);
    let eraser_open_id = Id::unique("studio-eraser-options-open");
    let mut eraser_open = ctx.data(|d| d.get_temp::<bool>(eraser_open_id)).unwrap_or(false) && view.pointer == Pointer::Erase;
    if !matches!(view.pointer, Pointer::Shape(_)) {
        shapes_open = false;
    }

    let dock = studio.toolbar_dock;
    let vertical = dock.is_vertical();
    let (pivot, anchor) = dock_anchor(view_rect, dock, edge_clearance);
    // How much room the bar has along its own length before it has to scroll.
    let extent = if vertical { view_rect.height() } else { view_rect.width() }
        // A bar down the left edge is centred, so it gives the floating header its height twice.
        - if dock == ToolbarDock::Left { 2.0 * (edge_clearance.max(0.0) + DOCK_MARGIN) } else { 0.0 };

    let last_dock_id = Id::unique("studio-pencil-last-dock");
    let dock_changed = ctx.data_mut(|d| {
        let prev = d.get_temp::<ToolbarDock>(last_dock_id);
        d.insert_temp(last_dock_id, dock);
        prev.is_some_and(|p| p != dock)
    });

    let area_resp = egui::Area::new(Id::unique("studio-apple-pencil-pill"))
        .order(Order::Foreground)
        .pivot(pivot)
        .fixed_pos(anchor)
        .constrain(true)
        .movable(false)
        .sizing_pass(dock_changed)
        .show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(t.panel.gamma_multiply(0.97))
                .stroke(Stroke::new(1.0, t.container_border))
                .corner_radius(14.0)
                .inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 6 });
            frame.show(ui, |ui| {
                let mut draw = |ui: &mut Ui, options: bool| {
                    if options {
                        draw_tool_popovers(ui, view, t, &mut palette_for, eraser_open, shapes_open, &mut acts);
                    } else {
                        let (moved_to, settings_tapped) = dock_row(ui, vertical, extent - 24.0, view_rect, settings_open, t, |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for tool in studio.roster.shown() {
                                match tool {
                                    DockTool::Write => {
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
                                    }
                                    DockTool::Pens => {
                                        // Every pen on the rack is on the bar as an upright colour-banded nib;
                                        // the active pen rises inward toward the canvas.
                                        for (i, &nib) in view.rack.iter().enumerate() {
                                            let active = view.pointer == Pointer::Draw && view.nib == i;
                                            let tool_label = crate::tl!(nib.tool.label());
                                            let (slot_rect, _) = ui.allocate_exact_size(nib_slot_size(dock), Sense::hover());
                                            let visual_rect = nib_visual_rect(slot_rect, dock, active);
                                            let ns = ui.interact(visual_rect, ui.id().with(("studio-nib", i)), Sense::click());
                                            ns.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, tool_label));
                                            if !active && ns.hovered() {
                                                ui.painter().rect_filled(visual_rect.expand(1.5), 6.0, t.row_hover);
                                            }
                                            let colors = nib_colors_for_theme(t, nib.colour, active);
                                            paint_upright_nib(ui.painter(), visual_rect, dock, nib, active, colors);
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
                                    }
                                    DockTool::Eraser => {
                                        if studio_icon_btn(
                                            ui,
                                            Icon::Eraser,
                                            view.pointer == Pointer::Erase,
                                            true,
                                            crate::tl_id!("ui.toggle_eraser_tip"),
                                            t,
                                        )
                                        .clicked()
                                        {
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
                                                egui::Button::new(
                                                    RichText::new(crate::tl_id!("ui.eraser_auto_short")).small().color(if view.eraser_auto_return {
                                                        t.accent
                                                    } else {
                                                        t.muted
                                                    }),
                                                )
                                                .selected(view.eraser_auto_return)
                                                .min_size(vec2(38.0, 34.0)),
                                            )
                                            .on_hover_text(crate::tl_id!("ui.eraser_auto_return_tip"));
                                        if auto_btn.clicked() {
                                            acts.push(Act::ToggleEraserAutoReturn);
                                        }
                                    }
                                    DockTool::Lasso => {
                                        if studio_icon_btn(ui, Icon::Lasso, view.pointer == Pointer::Lasso, true, crate::tl_id!("ui.lasso_select"), t)
                                            .clicked()
                                        {
                                            palette_for = None;
                                            shapes_open = false;
                                            acts.push(Act::InkPointer(Pointer::Lasso));
                                        }
                                    }
                                    DockTool::Select => {
                                        if studio_icon_btn(ui, Icon::Pointer, view.pointer == Pointer::Pick, true, crate::tl_id!("ui.select"), t)
                                            .clicked()
                                        {
                                            palette_for = None;
                                            shapes_open = false;
                                            acts.push(Act::InkPointer(Pointer::Pick));
                                        }
                                    }
                                    DockTool::Shapes => {
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
                                    }
                                    DockTool::Picture => {
                                        if studio_icon_btn(ui, Icon::Picture, false, true, crate::tl_id!("ui.dock_insert_picture"), t).clicked() {
                                            palette_for = None;
                                            shapes_open = false;
                                            acts.push(Act::AddPicture);
                                        }
                                    }
                                    DockTool::MathOcr => {
                                        let ocr_ready = view.math_ocr_enabled && view.holding_ink;
                                        if studio_icon_btn(ui, Icon::Equation, ocr_ready, true, crate::tl_id!("ui.dock_math_ocr"), t).clicked() {
                                            palette_for = None;
                                            shapes_open = false;
                                            acts.push(Act::DockMathOcr);
                                        }
                                    }
                                    DockTool::History => {
                                        if studio_icon_btn(ui, Icon::Undo, false, view.can_undo, crate::tl_id!("ui.undo"), t).clicked() {
                                            acts.push(Act::Undo);
                                        }
                                        if studio_icon_btn(ui, Icon::Redo, false, view.can_redo || view.can_ink_redo, crate::tl_id!("ui.redo"), t)
                                            .clicked()
                                        {
                                            acts.push(Act::Redo);
                                        }
                                        ui.separator();
                                    }
                                }
                            }
                        });
                        if let Some(edge) = moved_to {
                            acts.push(Act::ToolbarDock(edge));
                        }
                        if settings_tapped {
                            acts.push(Act::ToggleToolSettings);
                        }
                    }
                };
                // What a tool opens (its colours, the eraser's kinds, the shapes) goes on the side of the
                // bar that faces the page, whichever edge the bar is on.
                match dock {
                    ToolbarDock::Bottom => {
                        draw(ui, true);
                        draw(ui, false);
                    }
                    ToolbarDock::Top => {
                        draw(ui, false);
                        draw(ui, true);
                    }
                    ToolbarDock::Left => {
                        ui.horizontal_top(|ui| {
                            draw(ui, false);
                            ui.vertical(|ui| draw(ui, true));
                        });
                    }
                    ToolbarDock::Right => {
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| draw(ui, true));
                            draw(ui, false);
                        });
                    }
                }
            });
        });

    ctx.data_mut(|d| {
        d.insert_temp(palette_id, palette_for);
        d.insert_temp(shapes_open_id, shapes_open);
        d.insert_temp(eraser_open_id, eraser_open);
    });
    (acts, Some(area_resp.response.rect))
}

fn draw_tool_popovers(
    ui: &mut Ui,
    view: &View<'_>,
    t: &Tokens,
    palette_for: &mut Option<usize>,
    eraser_open: bool,
    shapes_open: bool,
    acts: &mut Vec<Act>,
) {
    if let Some(nib_idx) = *palette_for
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
        *palette_for = None;
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
                let pts: Vec<Pos2> = dabs.iter().map(|d| pos2(d.x, d.y)).collect();
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
}

/// Where the tool bar is pinned for an edge: the point, and which point of the bar goes there.
fn dock_anchor(view: Rect, dock: ToolbarDock, clearance: f32) -> (Align2, Pos2) {
    let clear = clearance.max(0.0) + DOCK_MARGIN;
    match dock {
        ToolbarDock::Bottom => (Align2::CENTER_BOTTOM, pos2(view.center().x, (view.max.y - clear).max(view.min.y + 52.0))),
        ToolbarDock::Top => (Align2::CENTER_TOP, pos2(view.center().x, (view.min.y + clear).min(view.max.y - 52.0))),
        ToolbarDock::Left => (Align2::LEFT_CENTER, pos2(view.min.x + DOCK_MARGIN, view.center().y)),
        ToolbarDock::Right => (Align2::RIGHT_CENTER, pos2((view.max.x - clear).max(view.min.x + 52.0), view.center().y)),
    }
}

/// The edge of `view` nearest to `at`: where a tool bar let go there comes to rest.
#[must_use]
pub fn nearest_dock(view: Rect, at: Pos2) -> ToolbarDock {
    let distances = [
        (ToolbarDock::Bottom, view.max.y - at.y),
        (ToolbarDock::Top, at.y - view.min.y),
        (ToolbarDock::Left, at.x - view.min.x),
        (ToolbarDock::Right, view.max.x - at.x),
    ];
    distances.into_iter().min_by(|a, b| a.1.total_cmp(&b.1)).map_or(ToolbarDock::Bottom, |(dock, _)| dock)
}

/// The strip along an edge that is outlined while the tool bar is dragged towards it.
fn dock_target_rect(view: Rect, dock: ToolbarDock) -> Rect {
    let inner = view.shrink(DOCK_MARGIN);
    let (long, thick) = (0.6, 46.0);
    match dock {
        ToolbarDock::Bottom => Rect::from_center_size(pos2(inner.center().x, inner.max.y - thick / 2.0), vec2(inner.width() * long, thick)),
        ToolbarDock::Top => Rect::from_center_size(pos2(inner.center().x, inner.min.y + thick / 2.0), vec2(inner.width() * long, thick)),
        ToolbarDock::Left => Rect::from_center_size(pos2(inner.min.x + thick / 2.0, inner.center().y), vec2(thick, inner.height() * long)),
        ToolbarDock::Right => Rect::from_center_size(pos2(inner.max.x - thick / 2.0, inner.center().y), vec2(thick, inner.height() * long)),
    }
}

/// Lays the tool bar out along its length, across the page or down it: the grip it is dragged
/// by, then its tools, then the three dots for Tool Settings.
fn dock_row(
    ui: &mut Ui,
    vertical: bool,
    room: f32,
    view: Rect,
    settings_open: bool,
    t: &Tokens,
    add: impl FnOnce(&mut Ui),
) -> (Option<ToolbarDock>, bool) {
    let room = (room - 86.0).max(120.0);
    let mut ends = (None, false);
    let row = |ui: &mut Ui| {
        ends.0 = dock_grip(ui, view, vertical, t);
        let scroll = egui::containers::scroll_area::ScrollSource { scroll_bar: false, ..Default::default() };
        if vertical {
            egui::ScrollArea::vertical()
                .id_salt("studio-dock-scroll-down")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .scroll_source(scroll)
                .max_height(room)
                .min_scrolled_height(room)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_max_width(46.0);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        add(ui);
                    });
                });
        } else {
            egui::ScrollArea::horizontal()
                .id_salt("studio-dock-scroll")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .scroll_source(scroll)
                .max_width(room)
                .show(ui, |ui| ui.horizontal(add));
        }
        ends.1 = dots_btn(ui, settings_open, crate::tl_id!("ui.tool_settings"), t).clicked();
    };
    if vertical {
        ui.vertical(|ui| {
            ui.set_max_width(46.0);
            row(ui);
        });
    } else {
        ui.horizontal(row);
    }
    ends
}

/// The grip the tool bar is dragged by. While it is dragged the edge it would land on is
/// outlined; let go, it asks to be moved there.
fn dock_grip(ui: &mut Ui, view: Rect, vertical: bool, t: &Tokens) -> Option<ToolbarDock> {
    let size = if vertical { vec2(34.0, 16.0) } else { vec2(16.0, 34.0) };
    let (rect, grip) = ui.allocate_exact_size(size, Sense::drag());
    grip.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.move_tool_bar")));
    let step = if vertical { vec2(6.0, 0.0) } else { vec2(0.0, 6.0) };
    for k in [-1.0_f32, 0.0, 1.0] {
        ui.painter().circle_filled(rect.center() + step * k, 1.7, if grip.dragged() { t.accent } else { t.muted });
    }
    let last_id = Id::unique("studio-dock-grip-at");
    let at = ui.ctx().pointer_interact_pos().or_else(|| ui.data(|d| d.get_temp::<Pos2>(last_id)));
    let at = at?;
    if grip.dragged() {
        ui.data_mut(|d| d.insert_temp(last_id, at));
        let painter = ui.ctx().layer_painter(egui::LayerId::new(Order::Foreground, Id::unique("studio-dock-target")));
        let target = dock_target_rect(view, nearest_dock(view, at));
        painter.rect_filled(target, 14.0, t.accent.gamma_multiply(0.10));
        painter.rect_stroke(target, 14.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
    }
    if grip.drag_stopped() {
        ui.data_mut(|d| d.remove::<Pos2>(last_id));
        return Some(nearest_dock(view, at));
    }
    None
}

/// A button of three dots: more, here the Tool Settings screen.
// i18n-ui-text: label
fn dots_btn(ui: &mut Ui, active: bool, label: &str, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 34.0), Sense::click());
    let label_owned = label.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label_owned));
    if active {
        ui.painter().rect_filled(rect, 7.0, t.accent.gamma_multiply(0.18));
        ui.painter().rect_stroke(rect, 7.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 7.0, t.row_hover);
    }
    for k in [-1.0_f32, 0.0, 1.0] {
        ui.painter().circle_filled(rect.center() + vec2(6.0 * k, 0.0), 1.9, if active { t.accent } else { t.text });
    }
    resp
}
