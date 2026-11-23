//! Collapsible infinite-canvas minimap and 1-tap Fit All Content math.

use crate::icons::{self, Icon};
use crate::theme::Tokens;
use crate::touch_ux::{MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT};
use egui::{Context, Id, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

const MAP_W: f32 = 178.0;
const MAP_H: f32 = 118.0;
const HEADER_H: f32 = 30.0;
const DEFAULT_SHEET_W: f32 = 780.0;
const DEFAULT_SHEET_H: f32 = 520.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MinimapOut {
    pub pan_to_page_center: Option<Pos2>,
    pub fit_all: bool,
    pub toggle_collapsed: bool,
    pub occupied_rect: Option<Rect>,
}

/// Computes the union bounding box in unzoomed page coordinates across all text/media containers
/// and all ink strokes (including negative coordinates when strokes reach left of or above `origin`).
#[must_use]
pub fn content_bounds_page(container_frames: &[Rect], stroke_bounds: &[[f32; 4]]) -> Rect {
    let mut bounds: Option<Rect> = None;
    for r in container_frames {
        if r.is_finite() && r.width() > 0.5 && r.height() > 0.5 {
            bounds = Some(bounds.map_or(*r, |acc| acc.union(*r)));
        }
    }
    for &[x0, y0, x1, y1] in stroke_bounds {
        let sr = Rect::from_min_max(pos2(x0, y0), pos2(x1.max(x0 + 1.0), y1.max(y0 + 1.0)));
        if sr.is_finite() {
            bounds = Some(bounds.map_or(sr, |acc| acc.union(sr)));
        }
    }
    bounds.unwrap_or_else(|| Rect::from_min_size(Pos2::ZERO, vec2(DEFAULT_SHEET_W, DEFAULT_SHEET_H)))
}

/// Computes the zoom percentage (`MIN_ZOOM_PERCENT..=100`) that frames `bounds_page` inside `view_size`.
#[must_use]
pub fn compute_fit_zoom_percent(bounds_page: Rect, view_size: Vec2) -> u32 {
    let avail_w = (view_size.x - 56.0).max(64.0);
    let avail_h = (view_size.y - 56.0).max(64.0);
    let bw = bounds_page.width().max(64.0);
    let bh = bounds_page.height().max(64.0);
    let scale = (avail_w / bw).min(avail_h / bh);
    if !scale.is_finite() || scale <= 0.0 {
        return 100;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let raw = (scale * 100.0).floor().clamp(f32::from(MIN_ZOOM_PERCENT as u16), 100.0) as u32;
    raw.clamp(MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT.min(100))
}

/// Computes the visible `view_rect` mapped into unzoomed page coordinates relative to `origin`.
#[must_use]
pub fn viewport_in_page(view_rect: Rect, origin: Pos2, zoom_scale: f32) -> Rect {
    let z = zoom_scale.max(0.05);
    Rect::from_min_max(
        pos2((view_rect.min.x - origin.x) / z, (view_rect.min.y - origin.y) / z),
        pos2((view_rect.max.x - origin.x) / z, (view_rect.max.y - origin.y) / z),
    )
}

/// Computes the updated `(canvas_pan, new_scroll_offset_y)` that centers `target_page` inside
/// `view_rect` across both `Pointer::Write` (`ScrollArea`) and ink modes (`canvas_pan.y`),
/// handling negative ink coordinates (`y < 0`) seamlessly.
#[must_use]
pub fn compute_pan_and_scroll_for_page_center(
    target_page: Pos2,
    view_rect: Rect,
    origin: Pos2,
    zoom_scale: f32,
    current_scroll_y: f32,
    content_height_y: f32,
    current_canvas_pan: Vec2,
) -> (Vec2, f32) {
    let z = zoom_scale.max(0.05);
    let cur_screen_x = origin.x + target_page.x * z;
    let new_pan_x = (current_canvas_pan.x + (view_rect.center().x - cur_screen_x)).clamp(-1.0e6, 1.0e6);

    // `origin.y` already includes `current_canvas_pan.y` and `-current_scroll_y`.
    // Recover where `target_page.y` sits when `canvas_pan.y == 0` at `current_scroll_y`, clamp
    // `ScrollArea` to `[0.0, max_scroll_y]`, and carry any residual (< 0 or > max_scroll_y) in `canvas_pan.y`:
    let unpanned_screen_y = (origin.y - current_canvas_pan.y) + target_page.y * z;
    let needed_shift_y = view_rect.center().y - unpanned_screen_y;
    let raw_scroll_y = current_scroll_y - needed_shift_y;
    let max_scroll_y = (content_height_y - view_rect.height()).clamp(0.0, 1.0e7);
    let clamped_scroll_y = raw_scroll_y.clamp(0.0, max_scroll_y);
    let new_pan_y = (clamped_scroll_y - raw_scroll_y).clamp(-1.0e6, 1.0e6);
    (vec2(new_pan_x, new_pan_y), clamped_scroll_y)
}

/// Renders the collapsible infinite-canvas minimap at `Order::Foreground` in the top-right
/// corner of `view_rect` when no modal/overlay is open.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ctx: &Context,
    view_rect: Rect,
    origin: Pos2,
    zoom_percent: u32,
    collapsed: bool,
    top_clearance: f32,
    right_clearance: f32,
    container_frames: &[Rect],
    stroke_bounds: &[[f32; 4]],
    tokens: &Tokens,
) -> MinimapOut {
    let mut out = MinimapOut::default();
    if view_rect.width() < 240.0 || view_rect.height() < 180.0 {
        return out;
    }
    #[allow(clippy::cast_precision_loss)]
    let zoom_scale = (zoom_percent as f32 / 100.0).max(0.05);
    let content_page = content_bounds_page(container_frames, stroke_bounds);
    let view_page = viewport_in_page(view_rect, origin, zoom_scale);
    let drag_world_id = Id::new("infinite-canvas-minimap-drag-world");
    let live_world = content_page.union(view_page).expand(24.0);
    let world = ctx.data(|d| d.get_temp::<Rect>(drag_world_id)).unwrap_or(live_world);

    let panel_w = if collapsed { 148.0 } else { MAP_W + 12.0 };
    let panel_h = if collapsed { HEADER_H + 8.0 } else { HEADER_H + MAP_H + 14.0 };
    let max_x = view_rect.max.x - 12.0 - right_clearance.max(0.0);
    // Top right, as the reference canvas has it: under whatever already sits along the top edge.
    let min_y = view_rect.min.y + 12.0 + top_clearance.max(0.0);
    let min_pos = pos2((max_x - panel_w).max(view_rect.min.x + 8.0), min_y.min((view_rect.max.y - panel_h - 8.0).max(view_rect.min.y + 8.0)));

    let area_resp = egui::Area::new(Id::new("infinite-canvas-minimap"))
        .order(Order::Foreground)
        .fixed_pos(min_pos)
        .constrain(false)
        .movable(false)
        .show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(tokens.surface_elevated.gamma_multiply(0.96))
                .stroke(Stroke::new(1.0, tokens.border))
                .corner_radius(crate::theme::RADIUS_CARD)
                .shadow(tokens.shadow_popup())
                .inner_margin(egui::Margin::same(6));
            frame.show(ui, |ui| {
                ui.set_width(panel_w - 12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let fold_label = if collapsed { crate::tl_id!("ui.expand_minimap") } else { crate::tl_id!("ui.collapse_minimap") };
                    let map_btn = ui
                        .add(
                            egui::Button::new(egui::RichText::new(crate::tl_id!("ui.minimap")).small().color(tokens.text)).min_size(vec2(48.0, 24.0)),
                        )
                        .on_hover_text(fold_label);
                    if map_btn.clicked() {
                        out.toggle_collapsed = true;
                    }
                    let zoom_text = format!("{zoom_percent}%");
                    ui.label(egui::RichText::new(zoom_text).small().color(tokens.muted));
                    let fit_btn = ui
                        .add(
                            egui::Button::new(egui::RichText::new(crate::tl_id!("ui.fit_all_short")).small().color(tokens.text))
                                .min_size(vec2(42.0, 24.0)),
                        )
                        .on_hover_text(crate::tl_id!("ui.fit_all_content_tip"));
                    if fit_btn.clicked() {
                        out.fit_all = true;
                    }
                });

                if !collapsed {
                    ui.add_space(4.0);
                    let (map_rect, resp) = ui.allocate_exact_size(vec2(MAP_W, MAP_H), Sense::click_and_drag());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::tl_id!("ui.minimap_canvas_tip")));
                    let painter = ui.painter_at(map_rect);
                    painter.rect_filled(map_rect, crate::theme::RADIUS_CONTROL, tokens.editor);
                    painter.rect_stroke(map_rect, crate::theme::RADIUS_CONTROL, Stroke::new(1.0, tokens.border_subtle), StrokeKind::Inside);

                    let pad_map = map_rect.shrink(4.0);
                    let sx = pad_map.width() / world.width().max(1.0);
                    let sy = pad_map.height() / world.height().max(1.0);
                    let s = sx.min(sy);
                    let drawn_size = vec2(world.width() * s, world.height() * s);
                    let map_origin = pad_map.center() - drawn_size * 0.5;

                    let to_map = |p: Pos2| pos2(map_origin.x + (p.x - world.min.x) * s, map_origin.y + (p.y - world.min.y) * s);
                    let rect_to_map = |r: Rect| Rect::from_min_max(to_map(r.min), to_map(r.max));

                    for cr in container_frames {
                        if cr.is_finite() {
                            let mr = rect_to_map(*cr).intersect(map_rect);
                            if mr.width() > 0.5 && mr.height() > 0.5 {
                                painter.rect_filled(mr, 1.0, tokens.border_subtle);
                            }
                        }
                    }
                    let ink_fill = tokens.text.gamma_multiply(0.45);
                    for &[x0, y0, x1, y1] in stroke_bounds {
                        let sr = Rect::from_min_max(pos2(x0, y0), pos2(x1.max(x0 + 2.0), y1.max(y0 + 2.0)));
                        if sr.is_finite() {
                            let mut mr = rect_to_map(sr);
                            if mr.width() < 1.5 {
                                mr.max.x = mr.min.x + 1.5;
                            }
                            if mr.height() < 1.5 {
                                mr.max.y = mr.min.y + 1.5;
                            }
                            let clipped = mr.intersect(map_rect);
                            if clipped.is_positive() {
                                painter.rect_filled(clipped, 0.5, ink_fill);
                            }
                        }
                    }

                    let vr = rect_to_map(view_page).intersect(map_rect);
                    if vr.is_positive() {
                        painter.rect_filled(vr, 2.0, tokens.accent.gamma_multiply(0.14));
                        painter.rect_stroke(vr, 2.0, Stroke::new(1.5, tokens.accent), StrokeKind::Inside);
                    } else {
                        let c = to_map(view_page.center()).clamp(map_rect.min, map_rect.max);
                        icons::paint(&painter, Rect::from_center_size(c, vec2(8.0, 8.0)), Icon::Pointer, tokens.accent);
                    }

                    if resp.dragged() || resp.is_pointer_button_down_on() {
                        ctx.data_mut(|d| {
                            if d.get_temp::<Rect>(drag_world_id).is_none() {
                                d.insert_temp(drag_world_id, world);
                            }
                        });
                    } else {
                        ctx.data_mut(|d| d.remove::<Rect>(drag_world_id));
                    }
                    if (resp.clicked() || resp.dragged())
                        && let Some(ptr) = resp.interact_pointer_pos()
                        && s > 0.0
                    {
                        let px = world.min.x + (ptr.x - map_origin.x) / s;
                        let py = world.min.y + (ptr.y - map_origin.y) / s;
                        if px.is_finite() && py.is_finite() {
                            out.pan_to_page_center = Some(pos2(px, py));
                        }
                    }
                } else {
                    ctx.data_mut(|d| d.remove::<Rect>(drag_world_id));
                }
            });
        });
    out.occupied_rect = Some(area_resp.response.rect);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_zoom_clamps_to_15_floor_and_100_ceiling() {
        let huge = Rect::from_min_max(pos2(-1200.0, -800.0), pos2(6800.0, 5200.0));
        assert_eq!(compute_fit_zoom_percent(huge, vec2(1024.0, 768.0)), MIN_ZOOM_PERCENT);

        let medium = Rect::from_min_max(pos2(0.0, 0.0), pos2(2000.0, 1400.0));
        let z = compute_fit_zoom_percent(medium, vec2(1060.0, 800.0));
        assert!((45..=55).contains(&z), "expected ~50%, got {z}");

        let tiny = Rect::from_min_max(pos2(0.0, 0.0), pos2(120.0, 80.0));
        assert_eq!(compute_fit_zoom_percent(tiny, vec2(1024.0, 768.0)), 100);
    }

    #[test]
    fn pan_and_scroll_for_page_center_handles_positive_and_negative_ink_bounds() {
        let view = Rect::from_min_size(pos2(200.0, 50.0), vec2(800.0, 600.0));
        // Positive target below top: absorbed by ScrollArea offset_y with canvas_pan.y == 0.
        let origin = pos2(300.0, 60.0);
        let (pan, scroll_y) = compute_pan_and_scroll_for_page_center(pos2(1000.0, 2000.0), view, origin, 0.5, 0.0, 2400.0, Vec2::ZERO);
        assert_eq!(pan.y, 0.0);
        assert!((scroll_y - 710.0).abs() < 0.5, "got scroll_y={scroll_y}");
        let landed_x = origin.x + pan.x + 1000.0 * 0.5;
        assert!((landed_x - view.center().x).abs() < 0.5);

        // Negative Y target above top of sheet: scroll clamps at 0.0 and canvas_pan.y carries downward shift!
        let (pan_neg, scroll_neg) = compute_pan_and_scroll_for_page_center(pos2(-400.0, -800.0), view, origin, 0.5, 0.0, 2400.0, Vec2::ZERO);
        assert_eq!(scroll_neg, 0.0);
        assert!(pan_neg.y > 600.0, "expected downward canvas_pan.y for negative ink, got {pan_neg:?}");
        let landed_y = origin.y + pan_neg.y + (-800.0) * 0.5;
        assert!((landed_y - view.center().y).abs() < 0.5);

        // Overview zoom where content_height_y <= view.height(): max_scroll_y == 0 and canvas_pan.y carries upward shift!
        let (pan_ov, scroll_ov) = compute_pan_and_scroll_for_page_center(pos2(1000.0, 2000.0), view, origin, 0.25, 0.0, 580.0, Vec2::ZERO);
        assert_eq!(scroll_ov, 0.0);
        let landed_ov_y = origin.y + pan_ov.y + 2000.0 * 0.25;
        assert!((landed_ov_y - view.center().y).abs() < 0.5);
    }
}
