//! Strokes to a picture: dark ink on white paper, as a PNG, with no drawing library.
//!
//! Each stroke is the closed outline the page itself fills (`nocturne_ink::shape::outline`),
//! filled here by scanline with the non-zero rule, a few sub-lines to a pixel row so edges
//! are smooth. The ink is drawn black whatever its colour, because a reader of handwriting
//! wants contrast, not the pen's colour. Highlighter is left out: it lies over the writing.

use super::OcrError;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use nocturne_ink::{Stroke, Tool, shape};

/// The longest side of the picture, in pixels.
pub const MAX_SIDE: u32 = 1024;
/// White paper around the ink, in pixels.
const MARGIN: f32 = 24.0;
/// Pixels to a point for small writing. Larger writing is scaled down to fit `MAX_SIDE`.
const MAX_SCALE: f32 = 4.0;
/// Sub-lines to one pixel row.
const SUB: usize = 4;
/// Coordinates beyond this are a damaged file, not handwriting.
const SANE: f32 = 1.0e6;
/// Minimum rendered stroke width in pixels so thin or light-pressure pencil and brush strokes
/// stay crisp black ink when downscaled for OCR encoders.
const MIN_STROKE_PX: f32 = 2.6;

/// Rasterized monochrome PNG image together with its pixel dimensions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RasterImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

type Polygon = Vec<(f32, f32)>;

fn valid_strokes(strokes: &[Stroke]) -> Vec<&Stroke> {
    strokes
        .iter()
        .filter(|s| s.tool != Tool::Highlighter)
        .filter(|s| !s.dabs.is_empty())
        .filter(|s| s.dabs.iter().all(|d| d.x.is_finite() && d.y.is_finite() && d.x.abs() < SANE && d.y.abs() < SANE))
        .collect()
}

fn estimate_scale(valid: &[&Stroke]) -> Option<f32> {
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    let mut any = false;
    for stroke in valid {
        let half = if stroke.width.is_finite() && stroke.width > 0.0 { stroke.width * 0.5 } else { 0.5 };
        for d in &stroke.dabs {
            x0 = x0.min(d.x - half);
            y0 = y0.min(d.y - half);
            x1 = x1.max(d.x + half);
            y1 = y1.max(d.y + half);
            any = true;
        }
    }
    if !any {
        return None;
    }
    let longest = (x1 - x0).max(y1 - y0).max(1.0);
    Some(MAX_SCALE.min((MAX_SIDE as f32 - 2.0 * MARGIN) / longest))
}

/// Ensures every point along `stroke` renders at least `min_width_pt` wide before outlining,
/// lifting very thin or low-pressure pencil/brush strokes without bloating already-bold strokes.
fn lift_stroke_for_ocr(stroke: &Stroke, min_width_pt: f32) -> Stroke {
    let mut lifted = stroke.clone();
    if !lifted.width.is_finite() || lifted.width < min_width_pt {
        lifted.width = min_width_pt;
    }
    if shape::width_at(&lifted, 1.0) < min_width_pt {
        lifted.curve = Default::default();
        lifted.gain = 1.0;
    }
    for i in 0..lifted.dabs.len() {
        let p = lifted.dabs[i].pressure;
        if shape::width_at(&lifted, p) < min_width_pt {
            let mut lo = if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
            let mut hi = 1.0f32;
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                if shape::width_at(&lifted, mid) >= min_width_pt {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            lifted.dabs[i].pressure = hi;
        }
    }
    lifted
}

fn polygons(valid: &[&Stroke], min_width_pt: f32) -> Vec<Polygon> {
    valid
        .iter()
        .map(|s| {
            let lifted = lift_stroke_for_ocr(s, min_width_pt);
            shape::outline(&lifted)
        })
        .filter(|poly| poly.len() >= 3 && poly.iter().all(|(x, y)| x.is_finite() && y.is_finite()))
        .collect()
}

/// How much of each pixel the polygons cover, 0 to 1, row by row.
fn coverage(polys: &[Polygon], width: usize, height: usize) -> Vec<f32> {
    let mut cover = vec![0.0f32; width * height];
    let weight = 1.0 / SUB as f32;
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    for poly in polys {
        let (top, bottom) = poly.iter().fold((f32::MAX, f32::MIN), |(lo, hi), (_, y)| (lo.min(*y), hi.max(*y)));
        let first = (top.floor().max(0.0)) as usize;
        let last = (bottom.ceil().max(0.0) as usize).min(height);
        for row in first..last {
            for sub in 0..SUB {
                let y = row as f32 + (sub as f32 + 0.5) * weight;
                crossings.clear();
                for (i, a) in poly.iter().enumerate() {
                    let b = poly[(i + 1) % poly.len()];
                    if (a.1 <= y) != (b.1 <= y) {
                        let x = a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0);
                        crossings.push((x, if b.1 > a.1 { 1 } else { -1 }));
                    }
                }
                crossings.sort_by(|l, r| l.0.total_cmp(&r.0));
                let mut winding = 0;
                for pair in crossings.windows(2) {
                    winding += pair[0].1;
                    if winding != 0 {
                        fill_span(&mut cover[row * width..(row + 1) * width], pair[0].0, pair[1].0, weight);
                    }
                }
            }
        }
    }
    cover
}

/// Adds `weight` to the pixels of one row between `from` and `to`, less at a part-covered end.
fn fill_span(row: &mut [f32], from: f32, to: f32, weight: f32) {
    let (from, to) = (from.max(0.0), to.min(row.len() as f32));
    if to <= from {
        return;
    }
    let (first, last) = (from.floor() as usize, (to.ceil() as usize).min(row.len()));
    for (x, px) in row.iter_mut().enumerate().take(last).skip(first) {
        let covered = (to.min(x as f32 + 1.0) - from.max(x as f32)).clamp(0.0, 1.0);
        *px += covered * weight;
    }
}

/// The picture of these strokes together with its pixel dimensions.
/// Returns [`OcrError::NoInk`] if there is nothing in `strokes` to draw.
pub fn render_png(strokes: &[Stroke]) -> Result<RasterImage, OcrError> {
    let valid = valid_strokes(strokes);
    let Some(est_scale) = estimate_scale(&valid) else { return Err(OcrError::NoInk) };
    let min_width_pt = MIN_STROKE_PX / est_scale.max(1e-4);
    let mut polys = polygons(&valid, min_width_pt);
    let Some(&(x, y)) = polys.first().and_then(|p| p.first()) else { return Err(OcrError::NoInk) };
    let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
    for (px, py) in polys.iter().flatten() {
        (x0, y0, x1, y1) = (x0.min(*px), y0.min(*py), x1.max(*px), y1.max(*py));
    }
    let longest = (x1 - x0).max(y1 - y0).max(1.0);
    let scale = MAX_SCALE.min((MAX_SIDE as f32 - 2.0 * MARGIN) / longest);
    let side = |extent: f32| ((extent * scale + 2.0 * MARGIN).ceil() as u32).clamp(32, MAX_SIDE);
    let (width, height) = (side(x1 - x0), side(y1 - y0));
    for (px, py) in polys.iter_mut().flatten() {
        *px = (*px - x0) * scale + MARGIN;
        *py = (*py - y0) * scale + MARGIN;
    }
    let pixels: Vec<u8> =
        coverage(&polys, width as usize, height as usize).into_iter().map(|c| (255.0 * (1.0 - c.clamp(0.0, 1.0))).round() as u8).collect();
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(&pixels, width, height, ExtendedColorType::L8).map_err(|_| OcrError::Picture)?;
    Ok(RasterImage { png: out, width, height })
}

/// The picture of these strokes. `NoInk` if there is nothing in them to draw.
pub fn png(strokes: &[Stroke]) -> Result<Vec<u8>, OcrError> {
    Ok(render_png(strokes)?.png)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_ink::{Dab, Rgb};

    #[test]
    fn a_square_drawn_twice_or_backwards_is_filled_once() {
        let square = vec![(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0)];
        let backwards: Polygon = square.iter().rev().copied().collect();
        for polys in [vec![square.clone()], vec![backwards], vec![square.clone(), square]] {
            let cover = coverage(&polys, 8, 8);
            let at = |x: usize, y: usize| cover[y * 8 + x].clamp(0.0, 1.0);
            assert!((at(3, 3) - 1.0).abs() < 1e-4 && (at(5, 5) - 1.0).abs() < 1e-4, "inside is full");
            assert!(at(0, 0) == 0.0 && at(7, 7) == 0.0 && at(6, 3) == 0.0 && at(1, 3) == 0.0, "outside is empty");
        }
        // Half a pixel in from the left edge covers half of that pixel.
        let cover = coverage(&[vec![(2.5, 2.0), (6.0, 2.0), (6.0, 6.0), (2.5, 6.0)]], 8, 8);
        assert!((cover[3 * 8 + 2] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn render_png_reports_dimensions_and_lifts_thin_low_pressure_strokes() {
        // Wide equation-scale horizontal stroke with whisper-thin brush width and tiny pressure.
        let mut thin_brush = Stroke::new(Tool::BrushPen, Rgb::BLACK, 0.6);
        thin_brush.dabs = (0..=20).map(|i| Dab::new(i as f32 * 40.0, 50.0, 0.05)).collect();

        let rendered = render_png(&[thin_brush]).expect("render thin stroke");
        let img = image::load_from_memory(&rendered.png).expect("decode png").to_luma8();
        assert_eq!(rendered.width, img.width());
        assert_eq!(rendered.height, img.height());
        assert!(rendered.width > rendered.height * 4, "wide stroke preserves wide aspect ratio");

        // At the horizontal center column, the stroke must span at least 2 fully dark pixels vertically.
        let mid_x = img.width() / 2;
        let dark_rows = (0..img.height()).filter(|&y| img.get_pixel(mid_x, y).0[0] < 64).count();
        assert!(dark_rows >= 2, "expected lifted stroke to render at least 2 solid dark pixel rows, got {dark_rows}");
    }
}
