//! Upright colour-banded pen and highlighter nibs for the Tablet Studio dock.
//!
//! Each pen or highlighter stands perpendicular to its dock edge and points inward toward the
//! page. Inactive tools sit recessed against the outer dock edge; the active tool rises inward by
//! [`ACTIVE_NIB_RAISE`] points so the chosen pen is immediately clear at a glance.

use crate::theme::{RADIUS_POPUP, Tokens, mix, readable_on};
use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};
use nocturne_engine::ToolbarDock;
use nocturne_ink::{Rgb, Tool, rack::Nib};

/// Slot dimension along the length of the dock bar.
pub const DOCK_NIB_ALONG: f32 = 34.0;

/// Slot dimension perpendicular to the dock bar, including [`ACTIVE_NIB_RAISE`] travel room.
pub const DOCK_NIB_CROSS: f32 = 42.0;

/// How many points inward toward the canvas the active pen rises above inactive pens.
pub const ACTIVE_NIB_RAISE: f32 = 9.0;

pub(crate) fn thickness_button(ui: &mut egui::Ui, tenths: u16, chosen: bool, t: &Tokens) -> bool {
    let points = f32::from(tenths) / 10.0;
    let (rect, response) = ui.allocate_exact_size(vec2(46.0, 46.0), egui::Sense::click());
    let width = format!("{points:.1}");
    let name = crate::i18n::format_message_id("ui.size_pt", &[("size", &width)]);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, chosen, name.clone()));
    crate::tablet_chrome::paint_studio_btn_chrome(ui, rect, &response, chosen, true, t);
    let colour = if chosen { t.accent } else { t.text };
    ui.painter().circle_filled(rect.center_top() + vec2(0.0, 14.0), (points * 0.5).clamp(1.5, 8.5), colour);
    ui.painter().text(rect.center_bottom() - vec2(0.0, 6.0), egui::Align2::CENTER_BOTTOM, width, egui::FontId::proportional(11.0), colour);
    response.on_hover_text(name).clicked()
}

/// Theme-derived colours for an upright pen or highlighter silhouette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NibColors {
    pub barrel_fill: Color32,
    pub barrel_stroke: Color32,
    pub collar_fill: Color32,
    pub ink_fill: Color32,
    pub band_stroke: Color32,
}

fn luma_is_dark(c: Color32) -> bool {
    let luma = 0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b());
    luma < 128.0
}

/// Outer layout cell allocated on the dock for one pen or highlighter slot.
#[must_use]
pub fn nib_slot_size(dock: ToolbarDock) -> Vec2 {
    if dock.is_vertical() { vec2(DOCK_NIB_CROSS, DOCK_NIB_ALONG) } else { vec2(DOCK_NIB_ALONG, DOCK_NIB_CROSS) }
}

/// Interactive and painted bounding box of a pen inside `slot_rect`.
///
/// Inactive pens rest against the outer edge of `slot_rect`; an active pen shifts inward toward
/// the page by [`ACTIVE_NIB_RAISE`] points while remaining inside the dock pill.
#[must_use]
pub fn nib_visual_rect(slot_rect: Rect, dock: ToolbarDock, active: bool) -> Rect {
    let along_inset = 3.0_f32;
    if dock.is_vertical() {
        let raise = ACTIVE_NIB_RAISE.min((slot_rect.width() - 8.0).max(0.0));
        let y0 = (slot_rect.min.y + along_inset).min(slot_rect.center().y);
        let y1 = (slot_rect.max.y - along_inset).max(slot_rect.center().y);
        let (x0, x1) = match (dock, active) {
            (ToolbarDock::Left, true) => (slot_rect.min.x + raise, slot_rect.max.x),
            (ToolbarDock::Left, false) => (slot_rect.min.x, slot_rect.max.x - raise),
            (_, true) => (slot_rect.min.x, slot_rect.max.x - raise),
            (_, false) => (slot_rect.min.x + raise, slot_rect.max.x),
        };
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    } else {
        let raise = ACTIVE_NIB_RAISE.min((slot_rect.height() - 8.0).max(0.0));
        let x0 = (slot_rect.min.x + along_inset).min(slot_rect.center().x);
        let x1 = (slot_rect.max.x - along_inset).max(slot_rect.center().x);
        let (y0, y1) = match (dock, active) {
            (ToolbarDock::Top, true) => (slot_rect.min.y + raise, slot_rect.max.y),
            (ToolbarDock::Top, false) => (slot_rect.min.y, slot_rect.max.y - raise),
            (_, true) => (slot_rect.min.y, slot_rect.max.y - raise),
            (_, false) => (slot_rect.min.y + raise, slot_rect.max.y),
        };
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }
}

/// Computes contrast-checked barrel, collar, ink, and band-outline colours from `Tokens`.
#[must_use]
pub fn nib_colors_for_theme(t: &Tokens, colour: Rgb, active: bool) -> NibColors {
    let barrel_fill = mix(t.editor, t.panel, if active { 0.12 } else { 0.38 });
    let barrel_stroke = if active { t.accent } else { t.border };
    let collar_fill = mix(barrel_fill, t.muted, 0.22);
    let ink_fill = crate::tablet_chrome::shown_nib_color(colour, t.surface_elevated);
    let band_stroke = readable_on(t.border, barrel_fill, 2.5, luma_is_dark(barrel_fill));
    NibColors { barrel_fill, barrel_stroke, collar_fill, ink_fill, band_stroke }
}

/// Paints hover, pressed, and keyboard-focus affordances behind an upright dock nib without
/// altering the resting pixel geometry of the pen silhouette.
#[allow(clippy::too_many_arguments)]
pub fn paint_nib_state(painter: &Painter, slot_rect: Rect, visual_rect: Rect, active: bool, hovered: bool, pressed: bool, focused: bool, t: &Tokens) {
    let r = CornerRadius::same(RADIUS_POPUP);
    if pressed {
        let hit = slot_rect.shrink(1.0);
        painter.rect_filled(hit, r, t.row_selected);
        painter.rect_stroke(hit, r, Stroke::new(1.5, t.accent), StrokeKind::Inside);
    } else if hovered {
        let hi = if active { slot_rect.shrink(1.0) } else { visual_rect.expand(1.5) };
        painter.rect_filled(hi, r, t.row_hover);
        painter.rect_stroke(hi, r, Stroke::new(1.0, t.border_subtle), StrokeKind::Inside);
    }
    if focused {
        painter.rect_stroke(slot_rect.expand(1.0), CornerRadius::same(RADIUS_POPUP + 2), Stroke::new(2.0, t.accent), StrokeKind::Outside);
    }
}

/// Maps local upright pen coordinates `(u, v)` (`u` in `[-1.0, 1.0]` across width, `v` in
/// `[0.0, 1.0]` from outer barrel base to inward writing tip) onto screen coordinates.
fn nib_pt(visual_rect: Rect, dock: ToolbarDock, u: f32, v: f32) -> Pos2 {
    match dock {
        ToolbarDock::Bottom => {
            let half_w = visual_rect.width() * 0.5;
            pos2(visual_rect.center().x + u * half_w, visual_rect.max.y - v * visual_rect.height())
        }
        ToolbarDock::Top => {
            let half_w = visual_rect.width() * 0.5;
            pos2(visual_rect.center().x + u * half_w, visual_rect.min.y + v * visual_rect.height())
        }
        ToolbarDock::Left => {
            let half_w = visual_rect.height() * 0.5;
            pos2(visual_rect.min.x + v * visual_rect.width(), visual_rect.center().y + u * half_w)
        }
        ToolbarDock::Right => {
            let half_w = visual_rect.height() * 0.5;
            pos2(visual_rect.max.x - v * visual_rect.width(), visual_rect.center().y + u * half_w)
        }
    }
}

/// Paints an upright colour-banded pen or chisel highlighter inside `visual_rect`, oriented
/// inward toward the canvas for `dock`.
pub fn paint_upright_nib(painter: &Painter, visual_rect: Rect, dock: ToolbarDock, nib: Nib, active: bool, colors: NibColors) {
    let pt = |u: f32, v: f32| nib_pt(visual_rect, dock, u, v);
    let poly = |coords: &[(f32, f32)], fill: Color32, stroke: Stroke| {
        let pts: Vec<Pos2> = coords.iter().map(|&(u, v)| pt(u, v)).collect();
        painter.add(Shape::convex_polygon(pts, fill, stroke));
    };
    let outline = |coords: &[(f32, f32)], stroke: Stroke| {
        let pts: Vec<Pos2> = coords.iter().map(|&(u, v)| pt(u, v)).collect();
        painter.add(Shape::closed_line(pts, stroke));
    };
    let seg = |a: (f32, f32), b: (f32, f32), stroke: Stroke| {
        painter.line_segment([pt(a.0, a.1), pt(b.0, b.1)], stroke);
    };

    let body_stroke = Stroke::new(if active { 1.5 } else { 1.0 }, colors.barrel_stroke);
    let band_edge = Stroke::new(0.8, colors.band_stroke);
    let tip_edge = Stroke::new(0.75, colors.band_stroke);

    match nib.tool {
        Tool::Highlighter => {
            let b = 0.86;
            let body = [(-b, 0.02), (b, 0.02), (b, 0.58), (0.56, 0.74), (-0.56, 0.74), (-b, 0.58)];
            poly(&body, colors.barrel_fill, Stroke::NONE);
            poly(&[(-b, 0.16), (b, 0.16), (b, 0.32), (-b, 0.32)], colors.ink_fill, Stroke::NONE);
            seg((-b, 0.16), (b, 0.16), band_edge);
            seg((-b, 0.32), (b, 0.32), band_edge);
            outline(&body, body_stroke);
            // Angled chisel tip pointing inward toward the page.
            poly(&[(-0.54, 0.74), (0.54, 0.74), (0.54, 0.99), (-0.54, 0.87)], colors.ink_fill, tip_edge);
        }
        Tool::FountainPen => {
            let b = 0.72;
            let barrel = [(-b, 0.02), (b, 0.02), (b, 0.58), (-b, 0.58)];
            poly(&barrel, colors.barrel_fill, Stroke::NONE);
            poly(&[(-b, 0.16), (b, 0.16), (b, 0.32), (-b, 0.32)], colors.ink_fill, Stroke::NONE);
            seg((-b, 0.16), (b, 0.16), band_edge);
            seg((-b, 0.32), (b, 0.32), band_edge);
            outline(&barrel, body_stroke);
            poly(&[(-b, 0.58), (b, 0.58), (0.44, 0.69), (-0.44, 0.69)], colors.collar_fill, body_stroke);
            // Diamond fountain nib with breather hole; upper tip stays solid ink colour.
            poly(&[(-0.42, 0.69), (0.42, 0.69), (0.48, 0.78), (0.16, 0.99), (-0.16, 0.99), (-0.48, 0.78)], colors.ink_fill, tip_edge);
            painter.circle_filled(pt(0.0, 0.76), 1.0, colors.barrel_fill);
            seg((0.0, 0.70), (0.0, 0.76), Stroke::new(0.7, colors.barrel_fill));
        }
        Tool::BrushPen => {
            let b = 0.74;
            let barrel = [(-b, 0.02), (b, 0.02), (b, 0.58), (-b, 0.58)];
            poly(&barrel, colors.barrel_fill, Stroke::NONE);
            poly(&[(-b, 0.16), (b, 0.16), (b, 0.32), (-b, 0.32)], colors.ink_fill, Stroke::NONE);
            seg((-b, 0.16), (b, 0.16), band_edge);
            seg((-b, 0.32), (b, 0.32), band_edge);
            outline(&barrel, body_stroke);
            poly(&[(-b, 0.58), (b, 0.58), (0.44, 0.70), (-0.44, 0.70)], colors.collar_fill, body_stroke);
            poly(&[(-0.44, 0.70), (0.44, 0.70), (0.28, 0.85), (0.09, 0.99), (-0.09, 0.99), (-0.28, 0.85)], colors.ink_fill, tip_edge);
        }
        Tool::Pencil => {
            let b = 0.70;
            let barrel = [(-b, 0.02), (b, 0.02), (b, 0.58), (-b, 0.58)];
            poly(&barrel, colors.barrel_fill, Stroke::NONE);
            poly(&[(-b, 0.16), (b, 0.16), (b, 0.32), (-b, 0.32)], colors.ink_fill, Stroke::NONE);
            seg((-b, 0.16), (b, 0.16), band_edge);
            seg((-b, 0.32), (b, 0.32), band_edge);
            // Off-centre hexagonal facet hints keep v=0.50 on the centre line pure barrel_fill.
            for u in [-0.34_f32, 0.34] {
                seg((u, 0.36), (u, 0.55), Stroke::new(0.7, colors.collar_fill));
            }
            outline(&barrel, body_stroke);
            poly(&[(-b, 0.58), (b, 0.58), (0.34, 0.78), (-0.34, 0.78)], colors.collar_fill, body_stroke);
            poly(&[(-0.34, 0.78), (0.34, 0.78), (0.14, 0.99), (-0.14, 0.99)], colors.ink_fill, tip_edge);
        }
        Tool::Pen => {
            let b = 0.72;
            let tip_u = (nib.width() * 0.055).clamp(0.17, 0.30);
            let body = [(-b, 0.02), (b, 0.02), (b, 0.56), (0.36, 0.78), (-0.36, 0.78), (-b, 0.56)];
            poly(&body, colors.barrel_fill, Stroke::NONE);
            poly(&[(-b, 0.16), (b, 0.16), (b, 0.32), (-b, 0.32)], colors.ink_fill, Stroke::NONE);
            seg((-b, 0.16), (b, 0.16), band_edge);
            seg((-b, 0.32), (b, 0.32), band_edge);
            outline(&body, body_stroke);
            poly(
                &[(-0.36, 0.78), (0.36, 0.78), (tip_u, 0.93), (tip_u * 0.55, 0.99), (-tip_u * 0.55, 0.99), (-tip_u, 0.93)],
                colors.ink_fill,
                tip_edge,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tablet_chrome::NIB_COLOURS;
    use crate::theme::{Theme, contrast};

    #[test]
    fn active_nib_shifts_inward_on_all_four_edges_while_staying_inside_slot() {
        assert!((DOCK_NIB_ALONG - 34.0).abs() < 0.01);
        for dock in ToolbarDock::ALL {
            let slot = Rect::from_min_size(pos2(100.0, 200.0), nib_slot_size(dock));
            let active = nib_visual_rect(slot, dock, true);
            let rest = nib_visual_rect(slot, dock, false);
            assert!(slot.contains_rect(active) && slot.contains_rect(rest), "{dock:?}: visual rect stays inside slot");
            match dock {
                ToolbarDock::Bottom => assert!((rest.min.y - active.min.y - ACTIVE_NIB_RAISE).abs() < 0.01),
                ToolbarDock::Top => assert!((active.max.y - rest.max.y - ACTIVE_NIB_RAISE).abs() < 0.01),
                ToolbarDock::Left => assert!((active.max.x - rest.max.x - ACTIVE_NIB_RAISE).abs() < 0.01),
                ToolbarDock::Right => assert!((rest.min.x - active.min.x - ACTIVE_NIB_RAISE).abs() < 0.01),
            }
        }
    }

    #[test]
    fn nib_colours_meet_contrast_targets_across_all_fourteen_themes() {
        for theme in Theme::all() {
            let tokens = theme.tokens();
            for rgb in NIB_COLOURS {
                for active in [false, true] {
                    let c = nib_colors_for_theme(&tokens, rgb, active);
                    assert!(contrast(c.barrel_stroke, tokens.panel) >= 3.0);
                    assert!(contrast(c.barrel_stroke, tokens.surface_elevated) >= 3.0);
                    assert!(contrast(c.band_stroke, c.barrel_fill) >= 2.2);
                }
            }
        }
    }
}
