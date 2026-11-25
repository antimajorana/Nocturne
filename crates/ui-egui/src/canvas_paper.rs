use crate::ribbon::{RuleLines, paint_paper_canvas};
use crate::theme::Tokens;
use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};
use nocturne_store::page::CanvasLayout;

const MARGIN: f32 = 22.0;
// The title ends above the body origin, so the sheet needs room for it.
const TOP_MARGIN: f32 = 48.0;
const GAP: f32 = 18.0;

pub(crate) fn sheet_size(layout: CanvasLayout) -> Option<Vec2> {
    match layout {
        CanvasLayout::Infinite => None,
        CanvasLayout::Portrait => Some(vec2(820.0, 1160.0)),
        CanvasLayout::Landscape => Some(vec2(1160.0, 820.0)),
    }
}

pub(crate) fn sheet_end(layout: CanvasLayout, content_bottom: f32) -> Option<f32> {
    let height = sheet_size(layout)?.y;
    let stride = height + GAP;
    let index = ((content_bottom + TOP_MARGIN + MARGIN).max(0.0) / stride).floor();
    Some((index + 1.0) * stride - GAP - TOP_MARGIN)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint(
    painter: &Painter,
    viewport: Rect,
    origin: Pos2,
    zoom: f32,
    sheet: Vec2,
    rules: RuleLines,
    lines: (Color32, Color32),
    paper: Color32,
    t: &Tokens,
) {
    let clip = viewport.intersect(painter.clip_rect());
    if !clip.is_positive() {
        return;
    }
    let painter = painter.with_clip_rect(clip);
    painter.rect_filled(clip, 0.0, t.panel);
    let start = origin - vec2(MARGIN, TOP_MARGIN) * zoom;
    let sheet = sheet * zoom;
    let stride = sheet.y + GAP * zoom;
    // Visit only paper visible in this frame. Sheet breaks stay in page coordinates
    // while the camera moves. Ink and text keep their existing page coordinates.
    let first = ((clip.min.y - start.y) / stride).floor().max(0.0);
    let last = ((clip.max.y - start.y) / stride).floor();
    let mut page = first;
    while page <= last {
        let rect = Rect::from_min_size(pos2(start.x, start.y + page * stride), sheet);
        if rect.intersects(clip) {
            painter.rect_filled(rect, 0.0, paper);
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, t.border_subtle), StrokeKind::Inside);
            if rules.on() {
                let visible = rect.intersect(clip);
                paint_paper_canvas(&painter.with_clip_rect(visible), visible, rules, zoom, origin, lines.0, lines.1);
            }
        }
        page += 1.0;
    }
}
