use crate::ribbon::Act;
use crate::theme::{RADIUS_PILL, Tokens};
use egui::{Align2, Color32, Context, Id, Order, Rect, Stroke, pos2, vec2};
use nocturne_ink::Rgb;

pub(crate) fn selection_actions(ctx: &Context, selection: Rect, view: Rect, tokens: &Tokens, acts: &mut Vec<Act>) {
    let below = selection.bottom() + 54.0 <= view.bottom();
    let x = selection.center().x.clamp(view.left(), view.right());
    let y = if below { selection.bottom() + 10.0 } else { (selection.top() - 10.0).max(view.top() + 46.0) };
    egui::Area::new(Id::unique("ink-selection-actions"))
        .order(Order::Foreground)
        .pivot(if below { Align2::CENTER_TOP } else { Align2::CENTER_BOTTOM })
        .fixed_pos(pos2(x, y))
        .constrain_to(view)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(tokens.surface_elevated)
                .stroke(Stroke::new(1.0, tokens.border))
                .corner_radius(RADIUS_PILL)
                .inner_margin(egui::Margin::same(5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().interact_size.y = 34.0;
                        if ui.button(crate::tl_id!("ui.copy")).clicked() {
                            acts.push(Act::InkCopySelection);
                        }
                        if ui.button(crate::tl_id!("ui.delete")).clicked() {
                            acts.push(Act::InkDeleteSelection);
                        }
                        ui.menu_button(crate::tl_id!("ui.colour"), |ui| {
                            crate::ribbon::mark_popup_open(ui.ctx());
                            for row in nocturne_ink::rack::PALETTE {
                                ui.horizontal(|ui| {
                                    for colour in row {
                                        let Rgb(r, g, b) = colour;
                                        let response = ui.add(egui::Button::new("").fill(Color32::from_rgb(r, g, b)).min_size(vec2(28.0, 32.0)));
                                        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, colour.hex()));
                                        if response.on_hover_text(colour.hex()).clicked() {
                                            acts.push(Act::InkColourSelection(colour));
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        });
                    });
                });
        });
}
