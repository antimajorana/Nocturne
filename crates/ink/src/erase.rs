//! The pixel eraser: rubbing out the part of a stroke the eraser passes over, and leaving the
//! rest.
//!
//! The object eraser ([`crate::Drawing::erase_at`]) takes a whole stroke when it touches it.
//! This one cuts. A stroke the eraser crosses becomes the pieces either side of it, each a
//! stroke of its own with the pen, colour, width and pressure it had.
//!
//! Where it cuts matters. A stroke's end is drawn as a half circle as wide as the stroke is
//! there ([`crate::shape::outline`]), so a cut made at the edge of the eraser would leave ink
//! reaching into the erased part by half the stroke's width. The cut is made further out, where
//! the middle of the stroke is the eraser's radius plus half the stroke's width away: the round
//! end then just meets the eraser's edge.
//!
//! Only the stretch near the eraser is given more points. A stroke is kept as the readings the
//! pen made, a few to a centimetre; cutting needs finer steps than that, but adding them along
//! the whole stroke would swell every page's file each time the eraser touched one.

use crate::shape::{curve_bounds, distance_to_segment, spline, width_at};
use crate::{Dab, Drawing, MAX_STROKES, Stroke};

/// How far apart the points added near a cut are, in points.
const CUT_STEP: f32 = 1.75;

/// A piece shorter than this, in points, is not worth keeping: it would be a speck.
const MIN_PIECE: f32 = 0.5;

/// One point along a stroke being cut.
#[derive(Clone, Copy)]
struct Point {
    dab: Dab,
    /// One of the pen's own readings, not a point added for the cut.
    original: bool,
    erased: bool,
}

impl Drawing {
    /// Rubs out what an eraser of `radius` covers as it moves from `from` to `to`, cutting the
    /// strokes it crosses into the pieces left either side. Returns whether anything changed.
    ///
    /// The pieces take the place of the stroke they came from, in the same order. A sheet that
    /// is already full of strokes ([`MAX_STROKES`]) keeps as many pieces as there is room for.
    pub fn erase_pixel_segment(&mut self, from: (f32, f32), to: (f32, f32), radius: f32) -> bool {
        let finite = [from.0, from.1, to.0, to.1, radius].iter().all(|v| v.is_finite());
        if !finite || radius <= 0.0 {
            return false;
        }
        let mut changed = false;
        let mut kept: Vec<Stroke> = Vec::with_capacity(self.strokes.len());
        let mut room = MAX_STROKES.max(self.strokes.len());
        let strokes = std::mem::take(&mut self.strokes);
        let mut left = strokes.len();
        for stroke in strokes {
            left -= 1;
            match cut(&stroke, from, to, radius) {
                None => kept.push(stroke),
                Some(pieces) => {
                    changed = true;
                    // Room for these pieces is what the sheet may hold, less what is already
                    // kept and the strokes still to come.
                    let free = room.saturating_sub(kept.len() + left);
                    kept.extend(pieces.into_iter().take(free));
                }
            }
            room = room.max(kept.len() + left);
        }
        self.strokes = kept;
        changed
    }
}

/// The pieces of `stroke` left by the eraser, or `None` when it does not touch the stroke.
fn cut(stroke: &Stroke, from: (f32, f32), to: (f32, f32), radius: f32) -> Option<Vec<Stroke>> {
    let widest = width_at(stroke, 1.0) / 2.0;
    let reach = radius + widest;
    // The cheap box round the curve, not the stroke's full bounds: those resample the whole stroke,
    // and this runs for every stroke on the page on every frame the eraser is down.
    let (x0, y0, x1, y1) = curve_bounds(stroke)?;
    let (ex0, ey0, ex1, ey1) = (from.0.min(to.0) - reach, from.1.min(to.1) - reach, from.0.max(to.0) + reach, from.1.max(to.1) + reach);
    if ex1 < x0 || ex0 > x1 || ey1 < y0 || ey0 > y1 {
        return None;
    }
    let away = |d: Dab| distance_between(d, from, to);
    let erased = |d: Dab| away(d) < radius + width_at(stroke, d.pressure) / 2.0;

    let dabs = &stroke.dabs;
    let curved = dabs.len() >= 3;
    let at = |i: isize| dabs[(i.max(0) as usize).min(dabs.len() - 1)];
    let mut points: Vec<Point> = Vec::with_capacity(dabs.len() + 16);
    for (i, pair) in dabs.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        points.push(Point { dab: a, original: true, erased: erased(a) });
        let span = (b.x - a.x).hypot(b.y - a.y);
        // A piece of the stroke that stays further from the eraser than its own length (a
        // curve bows out by less than that) cannot be cut, and is left as the pen made it.
        if away(a).min(away(b)) > reach + span {
            continue;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let steps = ((span / CUT_STEP).ceil() as usize).clamp(1, 4096);
        for s in 1..steps {
            #[allow(clippy::cast_precision_loss)]
            let t = s as f32 / steps as f32;
            #[allow(clippy::cast_possible_wrap)]
            let i = i as isize;
            let dab = if curved {
                spline(at(i - 1), a, b, at(i + 2), t)
            } else {
                Dab { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, pressure: a.pressure + (b.pressure - a.pressure) * t }
            };
            points.push(Point { dab, original: false, erased: erased(dab) });
        }
    }
    let last = *dabs.last()?;
    points.push(Point { dab: last, original: true, erased: erased(last) });
    if !points.iter().any(|p| p.erased) {
        return None;
    }

    // Each run of points the eraser left is a piece. Its ends are moved to where the cut is.
    let mut pieces = Vec::new();
    let mut run: Vec<Point> = Vec::new();
    for (i, point) in points.iter().enumerate() {
        if point.erased {
            if let (Some(inside), false) = (run.last().copied(), run.is_empty()) {
                run.push(Point { dab: edge(inside.dab, point.dab, &erased), original: false, erased: false });
            }
            finish(stroke, &mut run, &mut pieces);
        } else {
            if run.is_empty()
                && let Some(before) = i.checked_sub(1).and_then(|j| points.get(j)).filter(|p| p.erased)
            {
                run.push(Point { dab: edge(point.dab, before.dab, &erased), original: false, erased: false });
            }
            run.push(*point);
        }
    }
    finish(stroke, &mut run, &mut pieces);
    Some(pieces)
}

/// Where between `kept` and `gone` the cut falls: the last place walking from `kept` that the
/// eraser does not cover, found by halving.
fn edge(kept: Dab, gone: Dab, erased: &impl Fn(Dab) -> bool) -> Dab {
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    let at = |t: f32| Dab {
        x: kept.x + (gone.x - kept.x) * t,
        y: kept.y + (gone.y - kept.y) * t,
        pressure: kept.pressure + (gone.pressure - kept.pressure) * t,
    };
    for _ in 0..12 {
        let mid = (lo + hi) / 2.0;
        if erased(at(mid)) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    at(lo)
}

/// Makes a stroke of the run of points gathered so far, if it is long enough to keep, and
/// empties the run.
///
/// The points added for the cut are thinned again, so the piece is not heavier than the cut
/// needs: close together at a cut end, and further apart the further from it, until the pen's
/// own readings take over. They are thinned by degrees and not dropped at a fixed distance,
/// because the stroke is drawn as a curve through its points, and a curve through a long gap
/// followed at once by short ones swings past its end.
fn finish(stroke: &Stroke, run: &mut Vec<Point>, pieces: &mut Vec<Stroke>) {
    let points = std::mem::take(run);
    let Some((first, last)) = points.first().zip(points.last()) else { return };
    let lone_dot = stroke.dabs.len() == 1;
    // How far along the piece each point is.
    let mut along = Vec::with_capacity(points.len());
    let mut length = 0.0_f32;
    for (i, point) in points.iter().enumerate() {
        if let Some(before) = i.checked_sub(1).and_then(|j| points.get(j)) {
            length += (point.dab.x - before.dab.x).hypot(point.dab.y - before.dab.y);
        }
        along.push(length);
    }
    if length < MIN_PIECE && !lone_dot {
        return;
    }
    // How far a point is from the nearest end that was cut. An end that is one of the pen's own
    // readings was not cut, and no added point is kept for it.
    let (cut_at_start, cut_at_end) = (!first.original, !last.original);
    let from_cut = |s: f32| match (cut_at_start, cut_at_end) {
        (true, true) => s.min(length - s),
        (true, false) => s,
        (false, true) => length - s,
        (false, false) => f32::INFINITY,
    };
    let end_index = points.len() - 1;
    let mut keep: Vec<bool> =
        points.iter().enumerate().map(|(i, p)| i == 0 || i == end_index || (p.original && from_cut(along[i]) > SPACING_FLOOR * 0.35)).collect();
    // The next kept point after each index, to measure the gap an added point would leave.
    let mut next_kept = vec![end_index; points.len()];
    for i in (0..end_index).rev() {
        next_kept[i] = if keep[i + 1] { i + 1 } else { next_kept[i + 1] };
    }
    let mut last_kept = 0;
    for i in 1..end_index {
        if !keep[i] && !points[i].original {
            let gap = (SPACING_FLOOR).max(from_cut(along[i]) * SPACING_GROWTH);
            if along[i] - along[last_kept] >= gap && along[next_kept[i]] - along[i] >= gap {
                keep[i] = true;
            }
        }
        if keep[i] {
            last_kept = i;
        }
    }
    let dabs: Vec<Dab> = points.iter().zip(&keep).filter(|(_, keep)| **keep).map(|(p, _)| p.dab.sane()).collect();
    pieces.push(Stroke { dabs: dabs.into(), ..stroke.clone() });
}

/// The closest two points of a piece are left to each other near a cut, a little under the
/// step they were added at so the first of them is kept.
const SPACING_FLOOR: f32 = CUT_STEP * 0.9;

/// How fast the spacing of the added points grows with distance from the cut.
const SPACING_GROWTH: f32 = 0.5;

/// How far a point is from the path the eraser's middle took.
fn distance_between(d: Dab, from: (f32, f32), to: (f32, f32)) -> f32 {
    distance_to_segment(d.x, d.y, from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Rgb, Tool, shape};

    fn line(x0: f32, x1: f32, y: f32, width: f32) -> Stroke {
        let mut s = Stroke::new(Tool::Pen, Rgb(0x18, 0x55, 0xc4), width);
        s.dabs = vec![Dab::new(x0, y, 1.0), Dab::new(x1, y, 1.0)].into();
        s
    }

    fn sheet(strokes: Vec<Stroke>) -> Drawing {
        let mut d = Drawing::new(400.0, 300.0);
        d.strokes = strokes;
        d
    }

    fn ends(s: &Stroke) -> (f32, f32) {
        (s.dabs.first().map_or(f32::NAN, |d| d.x), s.dabs.last().map_or(f32::NAN, |d| d.x))
    }

    #[test]
    fn a_stroke_crossed_by_the_eraser_becomes_the_two_pieces_either_side() {
        let mut d = sheet(vec![line(0.0, 200.0, 100.0, 4.0)]);
        assert!(d.erase_pixel_segment((100.0, 80.0), (100.0, 120.0), 5.0));
        assert_eq!(d.strokes.len(), 2, "a cut, not a whole stroke gone");
        // Half the stroke's width is 2, so the cut is 5 + 2 from the eraser's middle: the round
        // end of each piece then meets the eraser's edge and no ink is left inside it.
        let (a0, a1) = ends(&d.strokes[0]);
        let (b0, b1) = ends(&d.strokes[1]);
        assert!((a0 - 0.0).abs() < 1e-3 && (a1 - 93.0).abs() < 0.05, "left piece runs 0 to 93: {a0} to {a1}");
        assert!((b0 - 107.0).abs() < 0.05 && (b1 - 200.0).abs() < 1e-3, "right piece runs 107 to 200: {b0} to {b1}");
        for piece in &d.strokes {
            assert_eq!((piece.tool, piece.colour, piece.width), (Tool::Pen, Rgb(0x18, 0x55, 0xc4), 4.0), "each piece is the same pen");
            assert!(shape::outline(piece).iter().all(|(x, y)| (x - 100.0).hypot(y - 100.0) >= 5.0 - 0.05), "no ink is left under the eraser");
        }
    }

    #[test]
    fn the_pieces_carry_only_a_few_more_points_than_the_stroke_had() {
        let mut d = sheet(vec![line(0.0, 200.0, 100.0, 4.0)]);
        assert!(d.erase_pixel_segment((100.0, 100.0), (100.0, 100.0), 5.0));
        let points: usize = d.strokes.iter().map(|s| s.dabs.len()).sum();
        assert!(points <= 30, "a 200 point line cut once does not become a hundred points: {points}");

        // A long handwritten stroke keeps its own readings away from the cut.
        let mut wave = Stroke::new(Tool::Pen, Rgb::BLACK, 3.0);
        wave.dabs = (0..=100).map(|i| Dab::new(i as f32 * 4.0, 100.0 + (i as f32 * 0.4).sin() * 12.0, 0.7)).collect();
        let before = wave.dabs.len();
        let mut d = sheet(vec![wave]);
        assert!(d.erase_pixel_segment((200.0, 60.0), (200.0, 140.0), 6.0));
        assert_eq!(d.strokes.len(), 2);
        let after: usize = d.strokes.iter().map(|s| s.dabs.len()).sum();
        assert!(after <= before + 24, "points are added near the cut only: {before} before, {after} after");
    }

    #[test]
    fn what_the_eraser_does_not_touch_is_left_exactly_as_it_was() {
        let far = line(0.0, 200.0, 20.0, 4.0);
        let near_miss = line(0.0, 200.0, 107.5, 4.0);
        let mut dot = Stroke::new(Tool::Pen, Rgb::BLACK, 4.0);
        dot.dabs = vec![Dab::new(300.0, 100.0, 1.0)].into();
        let before = vec![far, near_miss, dot];
        let mut d = sheet(before.clone());
        assert!(!d.erase_pixel_segment((100.0, 100.0), (120.0, 100.0), 5.0), "5 + 2 does not reach a stroke 7.5 away");
        assert_eq!(d.strokes, before);
        for bad in [f32::NAN, f32::INFINITY, 0.0, -3.0] {
            assert!(!d.erase_pixel_segment((100.0, 20.0), (100.0, 20.0), bad), "a radius of {bad} rubs nothing out");
        }
        assert!(!d.erase_pixel_segment((f32::NAN, 20.0), (100.0, 20.0), 5.0));
        assert_eq!(d.strokes, before);
    }

    #[test]
    fn a_stroke_wholly_under_the_eraser_goes_and_a_speck_is_not_left_behind() {
        let mut dot = Stroke::new(Tool::Pen, Rgb::BLACK, 4.0);
        dot.dabs = vec![Dab::new(100.0, 100.0, 1.0)].into();
        // The short line's right end sticks 0.3 out from under the eraser: too little to keep.
        let mut d = sheet(vec![dot, line(98.0, 107.3, 100.0, 4.0), line(0.0, 300.0, 200.0, 4.0)]);
        assert!(d.erase_pixel_segment((100.0, 100.0), (100.0, 100.0), 5.0));
        assert_eq!(d.strokes.len(), 1, "the dot and the short line are gone, with no speck left");
        assert_eq!(ends(&d.strokes[0]), (0.0, 300.0), "the line the eraser never reached is untouched");
    }

    #[test]
    fn the_pieces_stay_in_the_strokes_place_in_the_drawing() {
        let (under, cut_me, over) = (line(0.0, 50.0, 10.0, 4.0), line(0.0, 200.0, 100.0, 4.0), line(0.0, 50.0, 190.0, 4.0));
        let mut d = sheet(vec![under.clone(), cut_me, over.clone()]);
        assert!(d.erase_pixel_segment((100.0, 100.0), (100.0, 100.0), 5.0));
        assert_eq!(d.strokes.len(), 4);
        assert_eq!(d.strokes[0], under);
        assert_eq!(d.strokes[3], over, "what was drawn over the cut stroke is still over its pieces");
    }

    #[test]
    fn a_sweep_rubs_out_everything_along_its_path() {
        let mut d = sheet(vec![line(0.0, 200.0, 100.0, 4.0)]);
        assert!(d.erase_pixel_segment((60.0, 100.0), (140.0, 100.0), 5.0));
        assert_eq!(d.strokes.len(), 2);
        assert!((ends(&d.strokes[0]).1 - 53.0).abs() < 0.05, "cut 7 before where the sweep began");
        assert!((ends(&d.strokes[1]).0 - 147.0).abs() < 0.05, "and 7 after where it ended");
    }

    #[test]
    fn a_full_sheet_never_grows_past_its_limit() {
        let mut strokes: Vec<Stroke> = (0..MAX_STROKES - 1)
            .map(|i| {
                let mut s = Stroke::new(Tool::Pen, Rgb::BLACK, 1.0);
                s.dabs = vec![Dab::new(1000.0 + i as f32, 1000.0, 1.0)].into();
                s
            })
            .collect();
        strokes.push(line(0.0, 200.0, 100.0, 4.0));
        let mut d = sheet(strokes);
        assert!(d.erase_pixel_segment((100.0, 100.0), (100.0, 100.0), 5.0));
        assert_eq!(d.strokes.len(), MAX_STROKES, "one piece is kept where there is room for one");
    }
}
