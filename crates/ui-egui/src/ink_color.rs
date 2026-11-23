//! Reusable, theme-token-compliant full RGB ink colour picker and custom-colour toggle button
//! shared by the Tablet Studio tool popover and Tool Settings.

use crate::tablet_chrome::{NIB_COLOURS, paint_studio_btn_chrome, shown_nib_color};
use crate::theme::{RADIUS_CONTROL, RADIUS_POPUP, Tokens};
use egui::{Color32, CornerRadius, Id, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use nocturne_ink::Rgb;

/// Parses a `#rrggbb`, `#rgb`, or bare `rrggbb` hex colour string.
fn parse_hex_input(raw: &str) -> Option<Rgb> {
    let trimmed = raw.trim();
    Rgb::parse(trimmed)
        .or_else(|| if trimmed.len() == 6 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) { Rgb::parse(&format!("#{trimmed}")) } else { None })
}

/// Renders a compact, touch-friendly RGB colour picker (preview swatch, editable `#rrggbb` hex
/// input, explicit `R`/`G`/`B` 0–255 sliders, and an HSV colour square + hue bar).
///
/// Returns `Some(next)` whenever the user changes the colour away from `current`. Does not close
/// any enclosing popover so dragging sliders or the colour square remains smooth.
pub fn show_rgb_picker(ui: &mut Ui, id: Id, current: Rgb, t: &Tokens) -> Option<Rgb> {
    let mut next = current;
    ui.push_id(id, |ui| {
        egui::Frame::NONE
            .fill(t.panel)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(RADIUS_POPUP))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_max_width(244.0);
                ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
                ui.spacing_mut().slider_width = 148.0;

                let hex_edit_id = id.with("hex-input");
                let hex_state_id = id.with("hex-state");
                let focused = ui.ctx().memory(|m| m.has_focus(hex_edit_id));
                let mut hex_buf = ui
                    .ctx()
                    .data(|d| d.get_temp::<(Rgb, String)>(hex_state_id))
                    .map_or_else(|| current.hex(), |(prev, buf)| if !focused || prev != current { current.hex() } else { buf });

                // Top row: live colour preview swatch + editable monospace `#rrggbb` readout.
                let mut hex_edited = false;
                let mut hex_has_focus = false;
                ui.horizontal(|ui| {
                    let (sw_rect, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                    let sw_radius = CornerRadius::same(RADIUS_CONTROL);
                    ui.painter().rect_filled(sw_rect, sw_radius, Color32::from_rgb(next.0, next.1, next.2));
                    ui.painter().rect_stroke(sw_rect, sw_radius, Stroke::new(1.0, t.border), StrokeKind::Inside);

                    let hex_resp =
                        ui.add(egui::TextEdit::singleline(&mut hex_buf).id(hex_edit_id).desired_width(86.0).font(egui::TextStyle::Monospace));
                    hex_has_focus = hex_resp.has_focus();
                    if hex_resp.changed() {
                        hex_edited = true;
                        if let Some(parsed) = parse_hex_input(&hex_buf) {
                            next = parsed;
                        }
                    }
                    ui.label(RichText::new(next.hex().to_ascii_uppercase()).monospace().small().color(t.muted));
                });

                // Touch-friendly R, G, B channel sliders (0..=255).
                let (mut r, mut g, mut b) = (next.0, next.1, next.2);
                let mut sliders_changed = false;
                ui.horizontal(|ui| {
                    // i18n-exempt: RGB channel initial.
                    ui.label(RichText::new("R").monospace().strong().color(t.text));
                    sliders_changed |= ui.add(egui::Slider::new(&mut r, 0..=255)).changed();
                });
                ui.horizontal(|ui| {
                    // i18n-exempt: RGB channel initial.
                    ui.label(RichText::new("G").monospace().strong().color(t.text));
                    sliders_changed |= ui.add(egui::Slider::new(&mut g, 0..=255)).changed();
                });
                ui.horizontal(|ui| {
                    // i18n-exempt: RGB channel initial.
                    ui.label(RichText::new("B").monospace().strong().color(t.text));
                    sliders_changed |= ui.add(egui::Slider::new(&mut b, 0..=255)).changed();
                });
                if sliders_changed {
                    next = Rgb(r, g, b);
                }

                // Visual 2D saturation-value square + hue spectrum bar (emoji-free so Nocturne's
                // strict MissingGlyphPolicy never trips on egui's built-in clipboard emoji).
                let picker_changed = show_hsv_square_and_hue(ui, id.with("hsv"), &mut next, t);

                let stored_hex = if (focused || hex_has_focus || hex_edited) && !sliders_changed && !picker_changed { hex_buf } else { next.hex() };
                ui.ctx().data_mut(|d| d.insert_temp(hex_state_id, (next, stored_hex)));
            });
    });
    (next != current).then_some(next)
}

/// Emoji-free 2D Saturation-Value square + horizontal Hue rainbow bar.
fn show_hsv_square_and_hue(ui: &mut Ui, id: Id, colour: &mut Rgb, t: &Tokens) -> bool {
    let mut changed = false;
    let hsv_id = id.with("hue-cache");
    let mut hsv = egui::ecolor::Hsva::from_srgb([colour.0, colour.1, colour.2]);
    // Keep the user's chosen hue when saturation or value hits 0 so dragging to black/white
    // does not jump the hue handle back to red.
    if (hsv.s < 0.001 || hsv.v < 0.001)
        && let Some(saved_h) = ui.ctx().data(|d| d.get_temp::<f32>(hsv_id))
    {
        hsv.h = saved_h;
    }

    // 2D Saturation (x) × Value (y) square.
    let sv_size = vec2(224.0, 112.0);
    let (sv_rect, sv_resp) = ui.allocate_exact_size(sv_size, Sense::click_and_drag());
    if (sv_resp.dragged() || sv_resp.clicked())
        && let Some(pos) = sv_resp.interact_pointer_pos()
    {
        hsv.s = ((pos.x - sv_rect.min.x) / sv_rect.width()).clamp(0.0, 1.0);
        hsv.v = (1.0 - (pos.y - sv_rect.min.y) / sv_rect.height()).clamp(0.0, 1.0);
        changed = true;
    }
    paint_sv_square(ui, sv_rect, hsv, t);

    // Horizontal Hue rainbow bar (0..=1).
    let hue_size = vec2(224.0, 18.0);
    let (hue_rect, hue_resp) = ui.allocate_exact_size(hue_size, Sense::click_and_drag());
    if (hue_resp.dragged() || hue_resp.clicked())
        && let Some(pos) = hue_resp.interact_pointer_pos()
    {
        hsv.h = ((pos.x - hue_rect.min.x) / hue_rect.width()).clamp(0.0, 1.0);
        changed = true;
    }
    paint_hue_bar(ui, hue_rect, hsv.h, t);

    ui.ctx().data_mut(|d| d.insert_temp(hsv_id, hsv.h));
    if changed {
        let [r, g, b] = hsv.to_srgb();
        *colour = Rgb(r, g, b);
    }
    changed
}

fn paint_sv_square(ui: &Ui, rect: egui::Rect, hsv: egui::ecolor::Hsva, t: &Tokens) {
    let mut mesh = egui::Mesh::default();
    let cols = 12usize;
    let rows = 12usize;
    for row in 0..=rows {
        let v = 1.0 - row as f32 / rows as f32;
        let y = egui::lerp(rect.min.y..=rect.max.y, row as f32 / rows as f32);
        for col in 0..=cols {
            let s = col as f32 / cols as f32;
            let x = egui::lerp(rect.min.x..=rect.max.x, s);
            let [r, g, b] = egui::ecolor::Hsva::new(hsv.h, s, v, 1.0).to_srgb();
            mesh.colored_vertex(pos2(x, y), Color32::from_rgb(r, g, b));
        }
    }
    let stride = (cols + 1) as u32;
    for row in 0..rows as u32 {
        for col in 0..cols as u32 {
            let tl = row * stride + col;
            let tr = tl + 1;
            let bl = tl + stride;
            let br = bl + 1;
            mesh.add_triangle(tl, tr, bl);
            mesh.add_triangle(tr, br, bl);
        }
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let knob = pos2(egui::lerp(rect.min.x..=rect.max.x, hsv.s), egui::lerp(rect.max.y..=rect.min.y, hsv.v));
    ui.painter().circle_stroke(knob, 6.0, Stroke::new(2.0, Color32::WHITE));
    ui.painter().circle_stroke(knob, 7.2, Stroke::new(1.2, Color32::BLACK));
}

fn paint_hue_bar(ui: &Ui, rect: egui::Rect, hue: f32, t: &Tokens) {
    let mut mesh = egui::Mesh::default();
    let stops = 18usize;
    for i in 0..=stops {
        let h = i as f32 / stops as f32;
        let x = egui::lerp(rect.min.x..=rect.max.x, h);
        let [r, g, b] = egui::ecolor::Hsva::new(h, 1.0, 1.0, 1.0).to_srgb();
        let c = Color32::from_rgb(r, g, b);
        mesh.colored_vertex(pos2(x, rect.min.y), c);
        mesh.colored_vertex(pos2(x, rect.max.y), c);
    }
    for i in 0..stops as u32 {
        let tl = i * 2;
        let bl = tl + 1;
        let tr = tl + 2;
        let br = tl + 3;
        mesh.add_triangle(tl, tr, bl);
        mesh.add_triangle(tr, br, bl);
    }
    ui.painter().add(egui::Shape::mesh(mesh));
    ui.painter().rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let x = egui::lerp(rect.min.x..=rect.max.x, hue.clamp(0.0, 1.0));
    let handle = egui::Rect::from_center_size(pos2(x, rect.center().y), vec2(6.0, rect.height() + 2.0));
    ui.painter().rect_stroke(handle, CornerRadius::same(2), Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
    ui.painter().rect_stroke(handle, CornerRadius::same(2), Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
}

/// Paints a compact, theme-token-compliant button that toggles the full RGB ink colour picker.
///
/// Shows an RGB triad when a preset swatch is active, or the active custom swatch (framed with
/// an accent selection ring) when a non-preset RGB colour is selected.
pub fn custom_color_button(ui: &mut Ui, size: Vec2, open: bool, active_colour: Rgb, is_preset: bool, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let selected = open || !is_preset;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, crate::tl_id!("ui.more_colours")));
    paint_studio_btn_chrome(ui, rect, &resp, selected, true, t);

    let center = rect.center();
    let radius = (size.x.min(size.y) * 0.30).clamp(7.0, 11.5);
    let red = shown_nib_color(NIB_COLOURS[2], t.surface_elevated);
    let green = shown_nib_color(NIB_COLOURS[3], t.surface_elevated);
    let blue = shown_nib_color(NIB_COLOURS[1], t.surface_elevated);

    if is_preset {
        let offset = radius * 0.44;
        let dot_r = radius * 0.54;
        let triad = [
            (pos2(center.x, center.y - offset), red),
            (pos2(center.x - offset * 0.866, center.y + offset * 0.5), green),
            (pos2(center.x + offset * 0.866, center.y + offset * 0.5), blue),
        ];
        for (dot_center, dot_colour) in triad {
            ui.painter().circle_filled(dot_center, dot_r, dot_colour);
            ui.painter().circle_stroke(dot_center, dot_r, Stroke::new(0.8, t.border));
        }
        if open {
            ui.painter().circle_stroke(center, radius + 2.5, Stroke::new(1.8, t.accent));
        }
    } else {
        let swatch = shown_nib_color(active_colour, t.surface_elevated);
        ui.painter().circle_filled(center, radius, swatch);
        ui.painter().circle_stroke(center, radius, Stroke::new(1.0, t.border));
        ui.painter().circle_stroke(center, radius + 2.5, Stroke::new(2.0, t.accent));
        // Small RGB badge on the top-right edge so the button remains recognizable as custom RGB.
        let badge = pos2(rect.max.x - 7.0, rect.min.y + 7.0);
        ui.painter().circle_filled(pos2(badge.x, badge.y - 2.0), 2.1, red);
        ui.painter().circle_filled(pos2(badge.x - 1.8, badge.y + 1.2), 2.1, green);
        ui.painter().circle_filled(pos2(badge.x + 1.8, badge.y + 1.2), 2.1, blue);
    }

    resp.on_hover_text(crate::tl_id!("ui.choose_a_custom_ink_colour"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn run_frame(ctx: &egui::Context, events: Vec<egui::Event>, f: impl FnMut(&mut Ui)) {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(480.0, 480.0))), events, ..Default::default() };
        let mut out = ctx.run_ui(raw, f);
        out.textures_delta.clear();
    }

    #[test]
    fn rgb_picker_and_custom_button_render_across_all_14_themes() {
        let themes = Theme::all();
        assert_eq!(themes.len(), 14);
        let sample = Rgb(0x42, 0x9b, 0xc3);
        for theme in themes {
            let ctx = egui::Context::default();
            ctx.set_visuals(theme.visuals());
            let t = theme.tokens();
            let mut emitted = None;
            run_frame(&ctx, Vec::new(), |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let _ = custom_color_button(ui, vec2(36.0, 36.0), false, sample, true, &t);
                    let _ = custom_color_button(ui, vec2(36.0, 36.0), true, sample, false, &t);
                    emitted = show_rgb_picker(ui, Id::unique(("test-rgb-picker", theme.id())), sample, &t);
                });
            });
            assert_eq!(emitted, None, "unmodified picker in theme {} must not emit a change", theme.id());
        }
    }

    #[test]
    fn rgb_picker_emits_updated_rgb_from_hex_input() {
        let ctx = egui::Context::default();
        let theme = Theme::DEFAULT;
        ctx.set_visuals(theme.visuals());
        let t = theme.tokens();
        let picker_id = Id::unique("unit-test-rgb-picker");
        let start = Rgb(0x20, 0x20, 0x20);
        let mut emitted = None;

        // Frame 1: lay out the picker.
        run_frame(&ctx, Vec::new(), |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                emitted = show_rgb_picker(ui, picker_id, start, &t);
            });
        });
        assert_eq!(emitted, None);

        // Focus the hex TextEdit and let egui settle focus before sending keyboard input.
        ctx.memory_mut(|m| m.request_focus(picker_id.with("hex-input")));
        run_frame(&ctx, Vec::new(), |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                emitted = show_rgb_picker(ui, picker_id, start, &t);
            });
        });

        let key = |k, pressed, modifiers| egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers };
        run_frame(
            &ctx,
            vec![
                key(egui::Key::A, true, egui::Modifiers::COMMAND),
                key(egui::Key::A, false, egui::Modifiers::COMMAND),
                key(egui::Key::Backspace, true, egui::Modifiers::NONE),
                key(egui::Key::Backspace, false, egui::Modifiers::NONE),
            ],
            |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    emitted = show_rgb_picker(ui, picker_id, start, &t);
                });
            },
        );
        run_frame(&ctx, vec![egui::Event::Text("#3a7bd5".to_string())], |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                emitted = show_rgb_picker(ui, picker_id, start, &t);
            });
        });
        assert_eq!(emitted, Some(Rgb(0x3a, 0x7b, 0xd5)));
    }

    #[test]
    fn rgb_picker_retains_partial_hex_input_across_idle_frames() {
        let ctx = egui::Context::default();
        let theme = Theme::DEFAULT;
        ctx.set_visuals(theme.visuals());
        let t = theme.tokens();
        let picker_id = Id::unique("unit-test-rgb-picker-idle");
        let start = Rgb(0x20, 0x20, 0x20);
        let state_id = picker_id.with("hex-state");
        let step = |events: Vec<egui::Event>| {
            let mut emitted = None;
            run_frame(&ctx, events, |ui| {
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    emitted = show_rgb_picker(ui, picker_id, start, &t);
                });
            });
            emitted
        };

        // Lay out picker, focus hex input, and settle focus.
        step(Vec::new());
        ctx.memory_mut(|m| m.request_focus(picker_id.with("hex-input")));
        step(Vec::new());

        // Select all + Backspace to clear the hex input, then run multiple idle frames.
        let key = |k, pressed, modifiers| egui::Event::Key { key: k, physical_key: Some(k), pressed, repeat: false, modifiers };
        step(vec![
            key(egui::Key::A, true, egui::Modifiers::COMMAND),
            key(egui::Key::A, false, egui::Modifiers::COMMAND),
            key(egui::Key::Backspace, true, egui::Modifiers::NONE),
            key(egui::Key::Backspace, false, egui::Modifiers::NONE),
        ]);
        step(Vec::new());
        step(Vec::new());
        assert_eq!(
            ctx.data(|d| d.get_temp::<(Rgb, String)>(state_id)).map(|(_, buf)| buf),
            Some(String::new()),
            "cleared hex input must not be wiped back to start.hex() on idle frames"
        );

        // Type partial "#3a", wait across idle frames, verify it is retained.
        let emitted = step(vec![egui::Event::Text("#3a".to_string())]);
        assert_eq!(emitted, None);
        step(Vec::new());
        step(Vec::new());
        assert_eq!(
            ctx.data(|d| d.get_temp::<(Rgb, String)>(state_id)).map(|(_, buf)| buf),
            Some("#3a".to_string()),
            "partial hex input must survive idle frames without being reset"
        );

        // Finish typing "7bd5" and verify the complete colour is emitted.
        let emitted = step(vec![egui::Event::Text("7bd5".to_string())]);
        assert_eq!(emitted, Some(Rgb(0x3a, 0x7b, 0xd5)));
    }
}
