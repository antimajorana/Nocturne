//! Picking ink up: which strokes a pointer or a lasso has caught, and what can be done to
//! them once they are held.
//!
//! Ink is a flat list of strokes in the file, and a selection is never written down: it is
//! what someone is holding at this moment, like a hand on a sheet of paper. So everything
//! here takes the strokes it works on by their place in that list, and none of it knows
//! about a window.

use crate::{Drawing, Stroke, shape};

/// The box around the strokes at these places, with the width of each line taken in.
/// `None` when nothing is held, or when nothing held is really there.
pub fn bounds(drawing: &Drawing, held: &[usize]) -> Option<(f32, f32, f32, f32)> {
    held.iter()
        .filter_map(|i| drawing.strokes.get(*i))
        .filter_map(Stroke::bounds)
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}

/// The topmost stroke under a point, which is the last one drawn there: ink laid down later
/// sits on top, so that is what a hand reaches first.
pub fn at(drawing: &Drawing, x: f32, y: f32, radius: f32) -> Option<usize> {
    drawing.strokes.iter().enumerate().rev().find(|(_, s)| shape::touches(s, x, y, radius)).map(|(i, _)| i)
}

/// Every stroke drawn wholly inside a lasso. Wholly, not partly: a loop thrown round two
/// words should not also drag in the long line that happens to pass through it.
pub fn inside(drawing: &Drawing, loop_: &[(f32, f32)]) -> Vec<usize> {
    if loop_.len() < 3 {
        return Vec::new();
    }
    drawing
        .strokes
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.dabs.is_empty() && s.dabs.iter().all(|d| encloses(loop_, d.x, d.y)))
        .map(|(i, _)| i)
        .collect()
}

/// Every stroke a rectangle (two corners, in any order) touches: a reading inside it, or the
/// line between two readings crossing it. A selection box takes ink this way, as it takes the
/// containers it touches.
pub fn touching(drawing: &Drawing, rect: (f32, f32, f32, f32)) -> Vec<usize> {
    let (x0, x1) = (rect.0.min(rect.2), rect.0.max(rect.2));
    let (y0, y1) = (rect.1.min(rect.3), rect.1.max(rect.3));
    let inside = |x: f32, y: f32| x >= x0 && x <= x1 && y >= y0 && y <= y1;
    // Liang-Barsky: whether the segment from a to b passes through the rectangle.
    let crosses = |(ax, ay): (f32, f32), (bx, by): (f32, f32)| {
        let (dx, dy) = (bx - ax, by - ay);
        let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
        for (p, q) in [(-dx, ax - x0), (dx, x1 - ax), (-dy, ay - y0), (dy, y1 - ay)] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
                if t0 > t1 {
                    return false;
                }
            }
        }
        true
    };
    drawing
        .strokes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.dabs.iter().any(|d| inside(d.x, d.y)) || s.dabs.windows(2).any(|w| crosses((w[0].x, w[0].y), (w[1].x, w[1].y))))
        .map(|(i, _)| i)
        .collect()
}

/// Whether a closed path goes round a point,/// Whether a closed path goes round a point, by counting the times a ray leaving the point
/// crosses it: an odd number means inside. The path is treated as closed whether or not its
/// last point meets its first, because a hand drawing a loop rarely closes it exactly.
pub fn encloses(loop_: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = loop_.len() - 1;
    for i in 0..loop_.len() {
        let (xi, yi) = loop_[i];
        let (xj, yj) = loop_[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Moves the strokes at these places by (`dx`, `dy`), expanding the drawing's view box so
/// moved ink is never clipped outside it.
pub fn move_by(drawing: &mut Drawing, held: &[usize], dx: f32, dy: f32) {
    if !dx.is_finite() || !dy.is_finite() {
        return;
    }
    let mut moved = false;
    for i in held {
        if let Some(s) = drawing.strokes.get_mut(*i) {
            for d in &mut s.dabs {
                #[allow(clippy::cast_possible_truncation)]
                {
                    d.x = (f64::from(d.x) + f64::from(dx)).clamp(-1.0e6, 1.0e6) as f32;
                    d.y = (f64::from(d.y) + f64::from(dy)).clamp(-1.0e6, 1.0e6) as f32;
                }
            }
            moved = true;
        }
    }
    if moved && let Some(box_) = bounds(drawing, held) {
        drawing.include_bounds(box_);
    }
}

/// Stretches them about (`ox`, `oy`), expanding the drawing's view box so scaled ink is never
/// clipped outside it. The line's own width follows the smaller of the two factors, so a stroke
/// made half as wide does not keep a fat edge.
pub fn scale_about(drawing: &mut Drawing, held: &[usize], ox: f32, oy: f32, fx: f32, fy: f32) {
    if !ox.is_finite() || !oy.is_finite() {
        return;
    }
    let sane = |f: f32| if f.is_finite() { f.clamp(0.05, 20.0) } else { 1.0 };
    let (fx, fy) = (sane(fx), sane(fy));
    let mut scaled = false;
    for i in held {
        if let Some(s) = drawing.strokes.get_mut(*i) {
            for d in &mut s.dabs {
                #[allow(clippy::cast_possible_truncation)]
                {
                    d.x = (f64::from(ox) + (f64::from(d.x) - f64::from(ox)) * f64::from(fx)).clamp(-1.0e6, 1.0e6) as f32;
                    d.y = (f64::from(oy) + (f64::from(d.y) - f64::from(oy)) * f64::from(fy)).clamp(-1.0e6, 1.0e6) as f32;
                }
            }
            s.width = (s.width * fx.min(fy)).clamp(crate::MIN_WIDTH, crate::MAX_WIDTH);
            scaled = true;
        }
    }
    if scaled && let Some(box_) = bounds(drawing, held) {
        drawing.include_bounds(box_);
    }
}

/// Moves every stroke whose top bound is at or below `y` vertically by `dy`, expanding the
/// drawing's view box so shifted ink is never clipped outside it. Returns how many strokes
/// moved.
pub fn shift_below(drawing: &mut Drawing, y: f32, dy: f32) -> usize {
    if !y.is_finite() || !dy.is_finite() || dy == 0.0 {
        return 0;
    }
    let below: Vec<usize> =
        drawing.strokes.iter().enumerate().filter(|(_, s)| s.bounds().is_some_and(|(_, top, _, _)| top >= y)).map(|(i, _)| i).collect();
    if below.is_empty() {
        return 0;
    }
    move_by(drawing, &below, 0.0, dy);
    below.len()
}

/// Takes the strokes at these places out, and gives back how many went. Places that are not
/// there are ignored, so a stale selection cannot remove the wrong ink.
pub fn remove(drawing: &mut Drawing, held: &[usize]) -> usize {
    let mut going: Vec<usize> = held.iter().copied().filter(|i| *i < drawing.strokes.len()).collect();
    going.sort_unstable();
    going.dedup();
    for i in going.iter().rev() {
        drawing.strokes.remove(*i);
    }
    going.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dab, Rgb, Stroke, Tool};

    /// A straight stroke from (x0, y) to (x1, y).
    fn line(x0: f32, x1: f32, y: f32) -> Stroke {
        let mut s = Stroke::new(Tool::Pen, Rgb::BLACK, 2.0);
        s.dabs.push(Dab::new(x0, y, 1.0));
        s.dabs.push(Dab::new((x0 + x1) / 2.0, y, 1.0));
        s.dabs.push(Dab::new(x1, y, 1.0));
        s
    }

    fn sheet() -> Drawing {
        let mut d = Drawing::new(400.0, 300.0);
        d.add(line(10.0, 60.0, 20.0));
        d.add(line(10.0, 60.0, 50.0));
        d.add(line(200.0, 300.0, 200.0));
        d
    }

    /// A selection box takes every stroke it touches, as it takes every container it touches:
    /// any part inside, or a line passing right through it between two readings.
    #[test]
    fn a_rectangle_takes_every_stroke_it_touches() {
        let d = sheet();
        assert_eq!(touching(&d, (0.0, 0.0, 30.0, 30.0)), vec![0], "the end of the first line is inside");
        assert_eq!(touching(&d, (0.0, 10.0, 100.0, 60.0)), vec![0, 1], "both short lines");
        assert_eq!(touching(&d, (240.0, 190.0, 260.0, 210.0)), vec![2], "the long line passes through, no reading inside");
        assert_eq!(touching(&d, (100.0, 100.0, 150.0, 150.0)), Vec::<usize>::new(), "nothing there");
        assert_eq!(touching(&d, (30.0, 60.0, 0.0, 0.0)), vec![0, 1], "corners in any order");
    }

    #[test]
    fn the_box_round_what_is_held_takes_in_the_width_of_the_lines() {
        let d = sheet();
        assert_eq!(bounds(&d, &[]), None, "nothing held, no box");
        assert_eq!(bounds(&d, &[99]), None, "nor for ink that is not there");
        let (x0, y0, x1, y1) = bounds(&d, &[0, 1]).expect("a box");
        assert!((x0 - 9.0).abs() < 0.01 && (x1 - 61.0).abs() < 0.01, "a point either side for the line's width: {x0} {x1}");
        assert!((y0 - 19.0).abs() < 0.01 && (y1 - 51.0).abs() < 0.01, "{y0} {y1}");
    }

    #[test]
    fn a_point_picks_up_the_stroke_under_it_and_the_topmost_when_they_overlap() {
        let mut d = sheet();
        assert_eq!(at(&d, 30.0, 20.0, 1.0), Some(0));
        assert_eq!(at(&d, 30.0, 50.0, 1.0), Some(1));
        assert_eq!(at(&d, 30.0, 120.0, 1.0), None, "nothing there");

        // Ink laid down later sits on top, so that is what a hand reaches first.
        d.add(line(10.0, 60.0, 20.0));
        assert_eq!(at(&d, 30.0, 20.0, 1.0), Some(3), "the newer one");
    }

    #[test]
    fn a_lasso_takes_what_is_wholly_inside_it_and_leaves_the_rest() {
        let d = sheet();
        let round_both = [(0.0, 0.0), (80.0, 0.0), (80.0, 70.0), (0.0, 70.0)];
        assert_eq!(inside(&d, &round_both), vec![0, 1]);

        let round_first = [(0.0, 0.0), (80.0, 0.0), (80.0, 35.0), (0.0, 35.0)];
        assert_eq!(inside(&d, &round_first), vec![0]);

        // A loop that only clips the end of a stroke does not take it: a lasso thrown
        // round two words must not drag in the long line passing through them.
        let clips = [(0.0, 0.0), (30.0, 0.0), (30.0, 35.0), (0.0, 35.0)];
        assert!(inside(&d, &clips).is_empty(), "half a stroke is not a stroke");

        for silly in [vec![], vec![(0.0, 0.0)], vec![(0.0, 0.0), (10.0, 10.0)]] {
            assert!(inside(&d, &silly).is_empty(), "a loop needs three points to be a loop");
        }
    }

    #[test]
    fn a_loop_encloses_what_is_in_it_however_it_was_drawn() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(encloses(&square, 5.0, 5.0));
        assert!(!encloses(&square, 15.0, 5.0));
        assert!(!encloses(&square, 5.0, -5.0));
        // A hand never closes a loop exactly, and it must not have to.
        let open = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 1.0)];
        assert!(encloses(&open, 5.0, 5.0));
    }

    #[test]
    fn held_ink_moves_and_nothing_else_does() {
        let mut d = sheet();
        let before = d.strokes[2].clone();
        move_by(&mut d, &[0], 100.0, 5.0);
        assert!((d.strokes[0].dabs[0].x - 110.0).abs() < 0.01);
        assert!((d.strokes[0].dabs[0].y - 25.0).abs() < 0.01);
        assert_eq!(d.strokes[2], before, "the ink that was not held stayed put");

        // A move that is not a number does nothing rather than losing the ink.
        let now = d.strokes[0].clone();
        move_by(&mut d, &[0], f32::NAN, 1.0);
        assert_eq!(d.strokes[0], now);
    }

    #[test]
    fn held_ink_stretches_about_the_corner_it_is_dragged_from() {
        let mut d = sheet();
        scale_about(&mut d, &[0], 10.0, 20.0, 2.0, 1.0);
        assert!((d.strokes[0].dabs[0].x - 10.0).abs() < 0.01, "the corner it was pulled from stays put");
        assert!((d.strokes[0].dabs[2].x - 110.0).abs() < 0.01, "and the far end goes twice as far");
        assert!((d.strokes[0].dabs[2].y - 20.0).abs() < 0.01, "with nothing happening the other way");
        assert!((d.strokes[0].width - 2.0).abs() < 0.01, "a stroke stretched one way keeps its weight");

        let mut d = sheet();
        scale_about(&mut d, &[0], 0.0, 0.0, 0.5, 0.5);
        assert!((d.strokes[0].width - 1.0).abs() < 0.01, "made half the size, it is drawn half as heavily");
    }

    #[test]
    fn deleting_held_ink_takes_the_right_strokes_and_survives_a_stale_hold() {
        let mut d = sheet();
        let third = d.strokes[2].clone();
        assert_eq!(remove(&mut d, &[0, 1]), 2);
        assert_eq!(d.strokes.len(), 1);
        assert_eq!(d.strokes[0], third, "the one that was not held is the one that is left");

        // A hold naming ink that has already gone, or the same ink twice, is survivable.
        assert_eq!(remove(&mut d, &[0, 0, 7]), 1);
        assert!(d.strokes.is_empty());
        assert_eq!(remove(&mut d, &[3]), 0);
    }

    fn viewport_contains_stroke(drawing: &Drawing, index: usize) {
        let ring = crate::shape::outline(&drawing.strokes[index]);
        let (vx0, vy0) = (drawing.view_x, drawing.view_y);
        let (vx1, vy1) = (drawing.view_x + drawing.width, drawing.view_y + drawing.height);
        for (x, y) in ring {
            assert!(
                x >= vx0 - 0.01 && y >= vy0 - 0.01 && x <= vx1 + 0.01 && y <= vy1 + 0.01,
                "outline point ({x}, {y}) clipped outside viewBox ({vx0}, {vy0}, {vx1}, {vy1})"
            );
        }
    }

    #[test]
    fn move_by_expands_view_box_in_every_direction_without_shrinking() {
        let mut d = sheet();
        // Moving inside the existing sheet keeps the view box unchanged.
        move_by(&mut d, &[0], 20.0, 10.0);
        assert_eq!((d.view_x, d.view_y, d.width, d.height), (0.0, 0.0, 400.0, 300.0));

        // Moving past the bottom-right expands width and height while preserving top-left.
        move_by(&mut d, &[2], 250.0, 180.0);
        assert_eq!((d.view_x, d.view_y), (0.0, 0.0));
        assert!(d.width >= 551.0 && d.height >= 381.0, "bottom-right expanded: {}x{}", d.width, d.height);
        viewport_contains_stroke(&d, 2);

        // Moving past the top-left expands view_x and view_y negatively while keeping the old right/bottom.
        let right_before = d.view_x + d.width;
        let bottom_before = d.view_y + d.height;
        move_by(&mut d, &[0], -80.0, -70.0);
        assert!(d.view_x <= -51.0 && d.view_y <= -41.0, "top-left expanded negatively: {}, {}", d.view_x, d.view_y);
        assert!((d.view_x + d.width - right_before).abs() < 0.01, "right edge did not shrink");
        assert!((d.view_y + d.height - bottom_before).abs() < 0.01, "bottom edge did not shrink");
        for i in 0..d.strokes.len() {
            viewport_contains_stroke(&d, i);
        }

        // The expanded view box round-trips cleanly through SVG.
        let xml = crate::svg::write(&d, crate::svg::Flavour::Editable);
        let (back, flat) = crate::svg::read(&xml).expect("read moved drawing");
        assert_eq!(flat, 0);
        assert_eq!((back.view_x, back.view_y, back.width, back.height), (d.view_x, d.view_y, d.width, d.height));
    }

    #[test]
    fn scale_about_expands_view_box_for_positive_and_negative_growth() {
        let mut d = sheet();
        // Scale stroke 2 past the bottom-right edge and widen it at the same time.
        scale_about(&mut d, &[2], 200.0, 200.0, 3.0, 3.0);
        assert!(d.view_x + d.width >= 503.0, "right edge covers scaled stroke + half-width: {}", d.view_x + d.width);
        viewport_contains_stroke(&d, 2);

        // Scale stroke 0 about a point to its right/bottom so it expands into negative coordinates.
        let right_before = d.view_x + d.width;
        let bottom_before = d.view_y + d.height;
        scale_about(&mut d, &[0], 60.0, 20.0, 3.0, 2.0);
        assert!(d.view_x <= -92.0, "left edge expanded negatively: {}", d.view_x);
        assert!((d.view_x + d.width - right_before).abs() < 0.01, "right edge preserved");
        assert!((d.view_y + d.height - bottom_before).abs() < 0.01, "bottom edge preserved");
        viewport_contains_stroke(&d, 0);

        // Non-finite origin is ignored rather than corrupting strokes or view box.
        let before = d.clone();
        scale_about(&mut d, &[0], f32::NAN, 0.0, 2.0, 2.0);
        assert_eq!(d, before);
    }

    #[test]
    fn shift_below_moves_only_lower_strokes_and_expands_view_box() {
        let mut d = sheet();
        let top_stroke = d.strokes[0].clone();

        // Shift everything at or below y = 40 down past the bottom of the 300pt sheet.
        let moved = shift_below(&mut d, 40.0, 180.0);
        assert_eq!(moved, 2, "strokes 1 and 2 sit below y = 40");
        assert_eq!(d.strokes[0], top_stroke, "stroke above the cut stayed put");
        assert!((d.strokes[1].dabs[0].y - 230.0).abs() < 0.01);
        assert!((d.strokes[2].dabs[0].y - 380.0).abs() < 0.01);
        assert!(d.view_y + d.height >= 381.0, "view box expanded downward: {}", d.view_y + d.height);
        for i in 0..d.strokes.len() {
            viewport_contains_stroke(&d, i);
        }

        // Shift everything up past y = 0 to verify negative top expansion too.
        let moved_up = shift_below(&mut d, 0.0, -60.0);
        assert_eq!(moved_up, 3);
        assert!(d.view_y <= -41.0, "view box expanded upward: {}", d.view_y);
        for i in 0..d.strokes.len() {
            viewport_contains_stroke(&d, i);
        }

        // Zero, non-finite, or empty shifts are no-ops.
        let before = d.clone();
        assert_eq!(shift_below(&mut d, 0.0, 0.0), 0);
        assert_eq!(shift_below(&mut d, f32::NAN, 10.0), 0);
        assert_eq!(shift_below(&mut d, 0.0, f32::INFINITY), 0);
        assert_eq!(shift_below(&mut d, 1000.0, 50.0), 0);
        assert_eq!(d, before);
    }
}
