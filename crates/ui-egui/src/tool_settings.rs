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
use crate::tablet_chrome::{NIB_COLOURS, shown_nib_color};
use crate::theme::Tokens;
use egui::{Align2, Context, Id, Order, Rect, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use nocturne_engine::{DockTool, StudioOptions, ToolbarDock};
use nocturne_ink::Tool;
use nocturne_ink::rack::{self, Nib};

/// The most colours one kind of pen is kept in.
pub const MAX_COLOURS_PER_TOOL: usize = 12;

/// What the screen shows: the choices as they stand.
pub struct ToolSettingsIn<'a> {
    pub finger_drawing: bool,
    pub hold_to_straighten: bool,
    pub eraser_pixel: bool,
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
    let button = egui::Button::new(RichText::new(label).color(if chosen { t.accent } else { t.text })).selected(chosen).min_size(vec2(78.0, 34.0));
    ui.add(button).clicked() && !chosen
}

/// A small square button with a picture on it and a name for what it does. The picture is one
/// of the drawn icons, not a letter: the fonts the app carries have no arrows or minus sign.
// i18n-ui-text: name
fn sign_button(ui: &mut Ui, icon: crate::icons::Icon, name: &str, enabled: bool, t: &Tokens) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 30.0), if enabled { Sense::click() } else { Sense::hover() });
    let name = name.to_string();
    response.widget_info(move || egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 6.0, t.row_hover);
    }
    ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, t.container_border), StrokeKind::Inside);
    crate::icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), icon, if enabled { t.text } else { t.faint });
    response.clicked()
}

/// Draws the screen over the page, in the middle of `view`, and returns what was asked for.
pub fn show(ctx: &Context, view: Rect, input: &ToolSettingsIn<'_>, t: &Tokens) -> Vec<Act> {
    let mut acts = Vec::new();
    let width = (view.width() - 48.0).clamp(280.0, 460.0);
    egui::Area::new(Id::new("studio-tool-settings"))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(view.center())
        .constrain(true)
        .show(ctx, |ui| {
            crate::ribbon::mark_popup_open(ui.ctx());
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(14)).corner_radius(14.0).show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::tl_id!("ui.tool_settings")).strong().size(17.0).color(t.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(crate::tl_id!("ui.close")).min_size(vec2(64.0, 32.0))).clicked() {
                            acts.push(Act::ToggleToolSettings);
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("studio-tool-settings-scroll")
                    .max_height((view.height() - 120.0).max(160.0))
                    .min_scrolled_height((view.height() - 120.0).max(160.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 12.0);
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
    ui.horizontal(|ui| {
        for (pixel, label) in [(false, crate::tl_id!("ui.object_eraser")), (true, crate::tl_id!("ui.pixel_eraser"))] {
            if choice(ui, label, input.eraser_pixel == pixel, t) {
                acts.push(Act::EraserPixel(pixel));
            }
        }
    });
    heading(ui, crate::tl_id!("ui.tool_bar_position"), t);
    ui.horizontal(|ui| {
        for dock in ToolbarDock::ALL {
            if choice(ui, edge_label(dock), input.studio.toolbar_dock == dock, t) {
                acts.push(Act::ToolbarDock(dock));
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
    let ring = crate::theme::mix(t.panel, t.text, 0.28);
    for tool in Tool::ALL {
        let label = crate::tl!(tool.label());
        let places: Vec<usize> = input.rack.iter().enumerate().filter(|(_, nib)| nib.tool == tool).map(|(i, _)| i).collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_sized(vec2(92.0, 28.0), egui::Label::new(RichText::new(label).color(t.text)).truncate());
            for (n, &place) in places.iter().enumerate() {
                let nib = input.rack[place];
                let in_hand = place == input.nib;
                let (rect, swatch) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                let name = crate::i18n::format_message_id("ui.colour_slot_named", &[("tool", label), ("n", &(n + 1).to_string())]);
                swatch.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, in_hand, &name));
                ui.painter().circle_filled(rect.center(), 10.0, shown_nib_color(nib.colour, t.panel));
                ui.painter().circle_stroke(rect.center(), 10.0, Stroke::new(0.8, ring));
                if in_hand {
                    ui.painter().rect_stroke(rect, 7.0, Stroke::new(1.6, t.accent), StrokeKind::Inside);
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
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_space(96.0);
                for rgb in NIB_COLOURS {
                    let (rect, swatch) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                    swatch.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.pen_colour")));
                    ui.painter().circle_filled(rect.center(), 8.5, shown_nib_color(rgb, t.panel));
                    ui.painter().circle_stroke(rect.center(), 8.5, Stroke::new(0.8, ring));
                    if nib.colour == rgb {
                        ui.painter().circle_stroke(rect.center(), 10.5, Stroke::new(1.8, t.accent));
                    }
                    if swatch.clicked() {
                        acts.push(Act::InkRack(RackEdit::Set(input.nib, Nib { colour: rgb, ..nib })));
                    }
                }
                ui.separator();
                for tenths in rack::THICKNESSES {
                    let chosen = nib.tenths.abs_diff(tenths) <= 3;
                    let (rect, dot) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                    dot.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, chosen, crate::tl_id!("ui.pen_thickness")));
                    if chosen {
                        ui.painter().rect_filled(rect, 5.0, t.accent.gamma_multiply(0.18));
                    }
                    ui.painter().circle_filled(rect.center(), (f32::from(tenths) / 20.0).clamp(1.2, 7.5), t.text);
                    if dot.clicked() {
                        acts.push(Act::InkRack(RackEdit::Set(input.nib, Nib { tenths, ..nib })));
                    }
                }
            });
        }
    }
}
