//! The Tool Settings screen of Tablet Studio: one place for the choices that used to be
//! scattered over the tool bar or had no control at all.
//!
//! It holds whether a finger draws, whether a held pen straightens its stroke, what the eraser
//! takes, which edge the tool bar sits on, which tools are on the bar and in what order, and
//! the pens: each kind of pen with the colours it is kept in.
//!
//! A pen's colours are places on the pen rack ([`nocturne_ink::rack`]): several places with the
//! same kind of pen are that pen in several colours. Nothing new is stored for them.

use crate::ribbon::{Act, RackEdit};
use crate::tablet_chrome::{NIB_COLOURS, paint_studio_btn_chrome, shown_nib_color};
use crate::theme::{RADIUS_CONTROL, RADIUS_POPUP, Tokens};
use egui::{Align2, Context, CornerRadius, Id, Order, Rect, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use nocturne_engine::{DockTool, LibraryView, StudioOptions, ToolbarDock};
use nocturne_ink::Tool;
use nocturne_ink::rack::{self, Nib};

/// The most colours one kind of pen is kept in.
pub const MAX_COLOURS_PER_TOOL: usize = 12;

/// What the screen shows: the choices as they stand.
pub struct ToolSettingsIn<'a> {
    pub finger_drawing: bool,
    pub hold_to_straighten: bool,
    pub eraser_pixel: bool,
    pub math_ocr_enabled: bool,
    pub library_view: LibraryView,
    pub studio: StudioOptions,
    pub rack: &'a [Nib],
    /// The place on the rack of the pen in hand.
    pub nib: usize,
}

/// The name of a tool on the bar, as the screen lists it.
fn dock_tool_label(tool: DockTool) -> &'static str {
    match tool {
        DockTool::Write => crate::tl_id!("ui.dock_tool_write"),
        DockTool::Pens => crate::tl_id!("ui.dock_tool_pens"),
        DockTool::Eraser => crate::tl_id!("ui.dock_tool_eraser"),
        DockTool::Lasso => crate::tl_id!("ui.dock_tool_lasso"),
        DockTool::Select => crate::tl_id!("ui.dock_tool_select"),
        DockTool::Shapes => crate::tl_id!("ui.dock_tool_shapes"),
        DockTool::Picture => crate::tl_id!("ui.dock_tool_picture"),
        DockTool::MathOcr => crate::tl_id!("ui.dock_tool_math_ocr"),
        DockTool::History => crate::tl_id!("ui.dock_tool_history"),
    }
}

fn edge_label(dock: ToolbarDock) -> &'static str {
    match dock {
        ToolbarDock::Bottom => crate::tl_id!("ui.edge_bottom"),
        ToolbarDock::Top => crate::tl_id!("ui.edge_top"),
        ToolbarDock::Left => crate::tl_id!("ui.edge_left"),
        ToolbarDock::Right => crate::tl_id!("ui.edge_right"),
    }
}

// i18n-ui-text: label
fn heading(ui: &mut Ui, label: &str, t: &Tokens) {
    ui.add_space(8.0);
    ui.label(RichText::new(label).strong().color(t.text));
}

/// One of a row of choices of which one is taken.
// i18n-ui-text: label
fn choice(ui: &mut Ui, label: &str, chosen: bool, t: &Tokens) -> bool {
    let button = egui::Button::new(RichText::new(label).color(if chosen { t.accent_text } else { t.text }))
        .fill(if chosen { t.row_selected } else { t.panel })
        .stroke(if chosen { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, t.border) })
        .corner_radius(CornerRadius::same(RADIUS_CONTROL))
        .selected(chosen)
        .min_size(vec2(78.0, 36.0));
    let resp = ui.add(button);
    if resp.has_focus() {
        ui.painter().rect_stroke(resp.rect.expand(1.5), CornerRadius::same(RADIUS_CONTROL + 2), Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
    resp.clicked() && !chosen
}

/// A small square button with a picture on it and a name for what it does. The picture is one
/// of the drawn icons, not a letter: the fonts the app carries have no arrows or minus sign.
// i18n-ui-text: name
fn sign_button(ui: &mut Ui, icon: crate::icons::Icon, name: &str, enabled: bool, t: &Tokens) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(36.0, 36.0), if enabled { Sense::click() } else { Sense::hover() });
    let name = name.to_string();
    response.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
    let pressed = enabled && response.is_pointer_button_down_on();
    let r = CornerRadius::same(RADIUS_CONTROL);
    if pressed {
        ui.painter().rect_filled(rect, r, t.row_selected);
        ui.painter().rect_stroke(rect, r, Stroke::new(1.5, t.accent), StrokeKind::Inside);
    } else if enabled && response.hovered() {
        ui.painter().rect_filled(rect, r, t.row_hover);
        ui.painter().rect_stroke(rect, r, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else {
        ui.painter().rect_filled(rect, r, if enabled { t.panel } else { t.faint });
        ui.painter().rect_stroke(rect, r, Stroke::new(1.0, if enabled { t.border } else { t.border_subtle }), StrokeKind::Inside);
    }
    if enabled && response.has_focus() {
        ui.painter().rect_stroke(rect.expand(1.5), CornerRadius::same(RADIUS_CONTROL + 2), Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
    let fg = if !enabled {
        t.disabled
    } else if pressed {
        t.accent
    } else {
        t.text
    };
    crate::icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), icon, fg);
    response.clicked()
}

/// Draws the screen over the page, in the middle of `view`, and returns what was asked for.
pub fn show(ctx: &Context, view: Rect, input: &ToolSettingsIn<'_>, t: &Tokens) -> Vec<Act> {
    let mut acts = Vec::new();
    let width = (view.width() - 48.0).clamp(280.0, 460.0);
    let scroll_height = (view.height() - 120.0).max(160.0);
    egui::Area::new(Id::unique("studio-tool-settings"))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(view.center())
        .constrain(true)
        .show(ctx, |ui| {
            crate::ribbon::mark_popup_open(ui.ctx());
            egui::Frame::NONE
                .fill(t.surface_elevated)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(RADIUS_POPUP))
                .shadow(t.shadow_popup())
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::tl_id!("ui.tool_settings")).strong().size(17.0).color(t.strong));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let close = ui.add(
                                egui::Button::new(RichText::new(crate::tl_id!("ui.close")).color(t.text))
                                    .fill(t.panel)
                                    .stroke(Stroke::new(1.0, t.border))
                                    .min_size(vec2(72.0, 36.0)),
                            );
                            if close.has_focus() {
                                ui.painter().rect_stroke(
                                    close.rect.expand(1.5),
                                    CornerRadius::same(RADIUS_CONTROL + 2),
                                    Stroke::new(2.0, t.accent),
                                    StrokeKind::Outside,
                                );
                            }
                            if close.clicked() {
                                acts.push(Act::ToggleToolSettings);
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("studio-tool-settings-scroll")
                        .max_height(scroll_height)
                        .min_scrolled_height(scroll_height)
                        .show(ui, |ui| {
                            ui.set_width(width - 12.0);
                            ui.spacing_mut().interact_size.y = ui.spacing().interact_size.y.max(36.0);
                            switches(ui, input, &mut acts, t);
                            roster(ui, input, &mut acts, t);
                            pens(ui, input, &mut acts, t);
                        });
                });
        });
    acts
}

/// The switches, the eraser's kind and the tool bar's edge.
fn switches(ui: &mut Ui, input: &ToolSettingsIn<'_>, acts: &mut Vec<Act>, t: &Tokens) {
    let mut finger = input.finger_drawing;
    if ui.checkbox(&mut finger, crate::tl_id!("ui.draw_with_finger")).changed() {
        acts.push(Act::ToggleFingerDrawing);
    }
    let mut straighten = input.hold_to_straighten;
    if ui.checkbox(&mut straighten, crate::tl_id!("ui.hold_to_straighten")).changed() {
        acts.push(Act::ToggleHoldToStraighten);
    }
    heading(ui, crate::tl_id!("ui.tool_settings_eraser"), t);
    ui.horizontal_wrapped(|ui| {
        for (pixel, label) in [(false, crate::tl_id!("ui.object_eraser")), (true, crate::tl_id!("ui.pixel_eraser"))] {
            if choice(ui, label, input.eraser_pixel == pixel, t) {
                acts.push(Act::EraserPixel(pixel));
            }
        }
    });
    heading(ui, crate::tl_id!("ui.tool_bar_position"), t);
    ui.horizontal_wrapped(|ui| {
        for dock in ToolbarDock::ALL {
            if choice(ui, edge_label(dock), input.studio.toolbar_dock == dock, t) {
                acts.push(Act::ToolbarDock(dock));
            }
        }
    });
    ui.add_space(6.0);
    let mut math_ocr = input.math_ocr_enabled;
    if ui.checkbox(&mut math_ocr, crate::tl_id!("ui.recognise_handwritten_equations")).changed() {
        acts.push(Act::ToggleMathOcr);
    }
    let ocr_btn = ui.add(
        egui::Button::new(RichText::new(crate::tl_id!("ui.recognition_service_open")).color(t.text))
            .fill(t.panel)
            .stroke(Stroke::new(1.0, t.border))
            .min_size(vec2(160.0, 36.0)),
    );
    if ocr_btn.has_focus() {
        ui.painter().rect_stroke(ocr_btn.rect.expand(1.5), CornerRadius::same(RADIUS_CONTROL + 2), Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
    if ocr_btn.clicked() {
        acts.push(Act::OpenMathOcrSetup);
    }
    heading(ui, crate::tl_id!("ui.library_view_heading"), t);
    ui.horizontal_wrapped(|ui| {
        for (view, label) in
            [(LibraryView::StudioGrid, crate::tl_id!("ui.library_view_grid")), (LibraryView::ClassicTree, crate::tl_id!("ui.library_view_tree"))]
        {
            if choice(ui, label, input.library_view == view, t) {
                acts.push(Act::LibraryView(view));
            }
        }
    });
}

/// Which tools are on the bar, and in what order.
fn roster(ui: &mut Ui, input: &ToolSettingsIn<'_>, acts: &mut Vec<Act>, t: &Tokens) {
    heading(ui, crate::tl_id!("ui.tools_on_the_bar"), t);
    let all = input.studio.roster.all();
    for (place, (tool, shown)) in all.into_iter().enumerate() {
        let label = dock_tool_label(tool);
        ui.horizontal(|ui| {
            let mut on = shown;
            if ui.checkbox(&mut on, label).changed() {
                let mut edited = input.studio.roster;
                edited.set_shown(tool, on);
                acts.push(Act::DockRoster(edited));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (sign, earlier, id) in
                    [(crate::icons::Icon::MoveDown, false, "ui.move_later_named"), (crate::icons::Icon::MoveUp, true, "ui.move_earlier_named")]
                {
                    let enabled = if earlier { place > 0 } else { place + 1 < all.len() };
                    let name = crate::i18n::format_message_id(id, &[("tool", label)]);
                    if sign_button(ui, sign, &name, enabled, t) {
                        let mut edited = input.studio.roster;
                        if edited.shift(tool, earlier) {
                            acts.push(Act::DockRoster(edited));
                        }
                    }
                }
            });
        });
    }
}

/// Each kind of pen with the colours it is kept in. The colour of the pen in hand can be
/// changed from the row of colours under its kind.
fn pens(ui: &mut Ui, input: &ToolSettingsIn<'_>, acts: &mut Vec<Act>, t: &Tokens) {
    heading(ui, crate::tl_id!("ui.pens_and_colours"), t);
    for tool in Tool::ALL {
        let label = crate::tl!(tool.label());
        let places: Vec<usize> = input.rack.iter().enumerate().filter(|(_, nib)| nib.tool == tool).map(|(i, _)| i).collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            ui.add_sized(vec2(92.0, 36.0), egui::Label::new(RichText::new(label).color(t.text)).truncate());
            for (n, &place) in places.iter().enumerate() {
                let nib = input.rack[place];
                let in_hand = place == input.nib;
                let (rect, swatch) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
                let name = crate::i18n::format_message_id("ui.colour_slot_named", &[("tool", label), ("n", &(n + 1).to_string())]);
                swatch.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, in_hand, &name));
                paint_studio_btn_chrome(ui, rect, &swatch, in_hand, true, t);
                ui.painter().circle_filled(rect.center(), 10.5, shown_nib_color(nib.colour, t.surface_elevated));
                ui.painter().circle_stroke(rect.center(), 10.5, Stroke::new(1.0, t.border));
                if in_hand {
                    ui.painter().circle_stroke(rect.center(), 13.0, Stroke::new(1.8, t.accent));
                }
                if swatch.clicked() {
                    acts.push(Act::InkPen(place));
                }
            }
            let can_add = places.len() < MAX_COLOURS_PER_TOOL && input.rack.len() < rack::MAX_NIBS;
            if sign_button(ui, crate::icons::Icon::Plus, &crate::i18n::format_message_id("ui.add_colour_named", &[("tool", label)]), can_add, t) {
                acts.push(Act::InkRack(RackEdit::Add(tool)));
            }
            let can_remove = !places.is_empty() && input.rack.len() > 1;
            if sign_button(
                ui,
                crate::icons::Icon::Minus,
                &crate::i18n::format_message_id("ui.remove_colour_named", &[("tool", label)]),
                can_remove,
                t,
            ) && let Some(&last) = places.last()
            {
                acts.push(Act::InkRack(RackEdit::Remove(last)));
            }
        });
        // The pen in hand, when it is of this kind: its colour and its thickness.
        if let Some(nib) = input.rack.get(input.nib).copied().filter(|nib| nib.tool == tool) {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                ui.add_space(96.0);
                for rgb in NIB_COLOURS {
                    let chosen = nib.colour == rgb;
                    let (rect, swatch) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
                    swatch.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.pen_colour")));
                    paint_studio_btn_chrome(ui, rect, &swatch, chosen, true, t);
                    ui.painter().circle_filled(rect.center(), 9.5, shown_nib_color(rgb, t.surface_elevated));
                    ui.painter().circle_stroke(rect.center(), 9.5, Stroke::new(1.0, t.border));
                    if chosen {
                        ui.painter().circle_stroke(rect.center(), 12.0, Stroke::new(1.8, t.accent));
                    }
                    if swatch.clicked() {
                        acts.push(Act::InkRack(RackEdit::Set(input.nib, Nib { colour: rgb, ..nib })));
                    }
                }
                ui.separator();
                for tenths in rack::THICKNESSES {
                    let chosen = nib.tenths.abs_diff(tenths) <= 3;
                    let (rect, dot) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
                    dot.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, chosen, crate::tl_id!("ui.pen_thickness")));
                    paint_studio_btn_chrome(ui, rect, &dot, chosen, true, t);
                    ui.painter().circle_filled(rect.center(), (f32::from(tenths) / 20.0).clamp(1.5, 8.0), if chosen { t.accent } else { t.text });
                    if dot.clicked() {
                        acts.push(Act::InkRack(RackEdit::Set(input.nib, Nib { tenths, ..nib })));
                    }
                }
            });
        }
    }
}
