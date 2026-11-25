//! The regular shapes a pen can draw: a line, a box, a circle, a star.
//!
//! OneNote's Draw tab has a row of them, and a drawn shape there is ink like any other
//! stroke: it can be rubbed out, picked up and moved. Nocturne does the same, and for
//! the same reason - a shape that is ink needs no second kind of thing on the page, and
//! everything the pen can already do to a stroke works on it the day it is drawn.
//!
//! A shape is traced as the dabs of one stroke, from the corner the drag started at to
//! the corner it ended at. The pressure is even along it, because a drawn shape is not a
//! hand stroke and should not thin and thicken as if it were.

use crate::Dab;

/// The shapes offered, in the order the ribbon shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Line,
    Arrow,
    Rectangle,
    RoundedRectangle,
    Square,
    Ellipse,
    Circle,
    Diamond,
    Triangle,
    Star,
}

impl Shape {
    pub const ALL: [Shape; 10] = [
        Shape::Line,
        Shape::Arrow,
        Shape::Rectangle,
        Shape::RoundedRectangle,
        Shape::Square,
        Shape::Ellipse,
        Shape::Circle,
        Shape::Diamond,
        Shape::Triangle,
        Shape::Star,
    ];

    /// The name it is stored and scripted by.
    pub fn id(self) -> &'static str {
        match self {
            Shape::Line => "line",
            Shape::Arrow => "arrow",
            Shape::Rectangle => "rectangle",
            Shape::RoundedRectangle => "rounded-rectangle",
            Shape::Square => "square",
            Shape::Ellipse => "ellipse",
            Shape::Circle => "circle",
            Shape::Diamond => "diamond",
            Shape::Triangle => "triangle",
            Shape::Star => "star",
        }
    }

    /// What the button says.
    pub fn label(self) -> &'static str {
        match self {
            Shape::Line => "Line",
            Shape::Arrow => "Arrow",
            Shape::Rectangle => "Rectangle",
            Shape::RoundedRectangle => "Rounded",
            Shape::Square => "Square",
            Shape::Ellipse => "Ellipse",
            Shape::Circle => "Circle",
            Shape::Diamond => "Diamond",
            Shape::Triangle => "Triangle",
            Shape::Star => "Star",
        }
    }

    pub fn parse(text: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|s| s.id().eq_ignore_ascii_case(text.trim()))
    }

    /// True for the ones that are drawn inside a square, however the drag was shaped.
    fn even(self) -> bool {
        matches!(self, Shape::Square | Shape::Circle | Shape::Star)
    }

    /// True for a shape that comes back to where it started.
    pub fn closed(self) -> bool {
        !matches!(self, Shape::Line | Shape::Arrow)
    }
}

/// How finely a curve is traced. Enough that a circle the size of a page reads as round,
/// few enough that the file does not carry hundreds of points for one shape.
const ROUND_STEPS: usize = 48;

/// The dabs of one stroke tracing `shape` between the two corners of the drag.
///
/// `pressure` is the same at every dab: a drawn shape has an even line, not the swell of a
/// hand stroke. A drag that went nowhere gives no dabs, so a click does not leave a dot.
pub fn trace(shape: Shape, from: (f32, f32), to: (f32, f32), pressure: f32) -> Vec<Dab> {
    let (x0, y0) = from;
    let (mut x1, mut y1) = to;
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return Vec::new();
    }
    if shape.even() {
        // A square, a circle and a star keep their shape whatever the drag did: the
        // shorter side decides, and the longer one follows the way the drag went.
        let side = (x1 - x0).abs().min((y1 - y0).abs());
        x1 = x0 + side * (x1 - x0).signum();
        y1 = y0 + side * (y1 - y0).signum();
    }
    if (x1 - x0).abs() < 1.0 && (y1 - y0).abs() < 1.0 {
        return Vec::new();
    }
    let (left, right) = (x0.min(x1), x0.max(x1));
    let (top, bottom) = (y0.min(y1), y0.max(y1));
    let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
    let (rx, ry) = ((right - left) / 2.0, (bottom - top) / 2.0);
    let points: Vec<(f32, f32)> = match shape {
        Shape::Line => vec![(x0, y0), (x1, y1)],
        Shape::Arrow => return along(&arrow_points(from, (x1, y1)), pressure),
        Shape::Rectangle | Shape::Square => {
            vec![(left, top), (right, top), (right, bottom), (left, bottom), (left, top)]
        }
        Shape::RoundedRectangle => rounded(left, top, right, bottom),
        Shape::Ellipse | Shape::Circle => (0..=ROUND_STEPS)
            .map(|i| {
                let a = i as f32 / ROUND_STEPS as f32 * std::f32::consts::TAU;
                (cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect(),
        Shape::Diamond => vec![(cx, top), (right, cy), (cx, bottom), (left, cy), (cx, top)],
        Shape::Triangle => vec![(cx, top), (right, bottom), (left, bottom), (cx, top)],
        Shape::Star => star(cx, cy, rx, ry),
    };
    along(&points, pressure)
}

/// Fits a square to a near-closed freehand outline, preserving its rotation and center.
pub fn trace_square(drawn: &[Dab], zoom: f32, pressure: f32) -> Option<Vec<Dab>> {
    // Split a closed loop along its diagonal. The farthest point on either
    // side of that diagonal is the other corner on that half of the loop.
    let first = *drawn.first()?;
    let last = *drawn.last()?;
    let farthest = |from: Dab| (0..drawn.len()).max_by(|&a, &b| distance(from, drawn[a]).total_cmp(&distance(from, drawn[b])));
    let a = farthest(first)?;
    let c = farthest(drawn[a])?;
    let diagonal = distance(drawn[a], drawn[c]);
    if diagonal * zoom < 28.0 || distance(first, last) > diagonal * 0.19 {
        return None;
    }
    let arc = |start: usize, end: usize| (start..drawn.len()).chain(0..end).take(if end >= start { end - start } else { drawn.len() - start + end });
    let corner = |start, end| {
        arc(start, end).max_by(|&i, &j| cross(drawn[start], drawn[end], drawn[i]).abs().total_cmp(&cross(drawn[start], drawn[end], drawn[j]).abs()))
    };
    let b = corner(a, c)?;
    let d = corner(c, a)?;
    let corners = [a, b, c, d];
    let mut side_sum = 0.0;
    let mut shortest = f32::INFINITY;
    let mut longest = 0.0_f32;
    let mut sign = 0.0;
    for i in 0..4 {
        let (start, end, next) = (corners[i], corners[(i + 1) % 4], corners[(i + 2) % 4]);
        let side = distance(drawn[start], drawn[end]);
        let angle = cross(drawn[start], drawn[end], drawn[next]) / (side * distance(drawn[end], drawn[next]));
        if !(0.52 * diagonal..=0.88 * diagonal).contains(&side)
            || !angle.is_finite()
            || angle.abs() < 0.86
            || (i > 0 && angle * sign < 0.0)
            || arc(start, end).any(|j| cross(drawn[start], drawn[end], drawn[j]).abs() > 0.11 * side * side)
        {
            return None;
        }
        side_sum += side;
        shortest = shortest.min(side);
        longest = longest.max(side);
        sign = angle.signum();
    }
    if longest > shortest * 1.35 {
        return None;
    }

    let [a, b, c, d] = corners.map(|i| drawn[i]);
    let (cx, cy) = ((a.x + b.x + c.x + d.x) * 0.25, (a.y + b.y + c.y + d.y) * 0.25);
    // Average all four sides in the same direction before normalizing.
    let ux = b.x - a.x + c.x - d.x + sign * (c.y - b.y + d.y - a.y);
    let uy = b.y - a.y + c.y - d.y - sign * (c.x - b.x + d.x - a.x);
    let half = side_sum * 0.125 / ux.hypot(uy);
    let (ux, uy) = (ux * half, uy * half);
    let (vx, vy) = (-sign * uy, sign * ux);
    let points = [
        (cx - ux - vx, cy - uy - vy),
        (cx + ux - vx, cy + uy - vy),
        (cx + ux + vx, cy + uy + vy),
        (cx - ux + vx, cy - uy + vy),
        (cx - ux - vx, cy - uy - vy),
    ];
    Some(along(&points, pressure))
}

fn distance(a: Dab, b: Dab) -> f32 {
    (b.x - a.x).hypot(b.y - a.y)
}

fn cross(a: Dab, b: Dab, p: Dab) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

/// Samples the straight edges and repeats the corners to preserve them during smoothing.
fn along(points: &[(f32, f32)], pressure: f32) -> Vec<Dab> {
    const STEP: f32 = 7.0;
    const CORNER: usize = 3;
    let mut out: Vec<Dab> = Vec::with_capacity(points.len() * 4);
    let mut push = |x: f32, y: f32| out.push(Dab { x, y, pressure });
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        for _ in 0..CORNER {
            push(x0, y0);
        }
        let length = (x1 - x0).hypot(y1 - y0);
        let steps = (length / STEP).floor() as usize;
        for i in 1..steps {
            let t = i as f32 / steps as f32;
            push(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
        }
    }
    if let Some(&(x, y)) = points.last() {
        for _ in 0..CORNER {
            push(x, y);
        }
    }
    out
}

/// A line with a head on the end, drawn as one stroke that goes up one side of the head
/// and back down the other, the way a pen draws an arrow without leaving the paper.
fn arrow_points(from: (f32, f32), to: (f32, f32)) -> Vec<(f32, f32)> {
    let (x0, y0) = from;
    let (x1, y1) = to;
    let (dx, dy) = (x1 - x0, y1 - y0);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 {
        return Vec::new();
    }
    let (ux, uy) = (dx / length, dy / length);
    // The head is a fifth of the line, within reason, and its barbs stand out at 30
    // degrees or so: cos and sin of that, near enough for a drawn arrow.
    let head = (length * 0.2).clamp(6.0, 40.0);
    let (bx, by) = (x1 - ux * head, y1 - uy * head);
    let (px, py) = (-uy * head * 0.5, ux * head * 0.5);
    vec![(x0, y0), (x1, y1), (bx + px, by + py), (x1, y1), (bx - px, by - py)]
}

/// A rectangle with its corners cut round, traced corner by corner.
fn rounded(left: f32, top: f32, right: f32, bottom: f32) -> Vec<(f32, f32)> {
    let width = (right - left).abs();
    let height = (bottom - top).abs();
    let short = width.min(height);
    let max_r = (0.5 * short).max(0.0);
    let r = (short * 0.2).clamp(2.0, 40.0).min(max_r);
    if r <= f32::EPSILON {
        return vec![(left, top), (right, top), (right, bottom), (left, bottom), (left, top)];
    }
    let corner = |cx: f32, cy: f32, from: f32| {
        (0..=6).map(move |i| {
            let a = from + i as f32 / 6.0 * std::f32::consts::FRAC_PI_2;
            (cx + r * a.cos(), cy + r * a.sin())
        })
    };
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(34);
    out.push((left + r, top));
    out.push((right - r, top));
    out.extend(corner(right - r, top + r, -std::f32::consts::FRAC_PI_2));
    out.push((right, bottom - r));
    out.extend(corner(right - r, bottom - r, 0.0));
    out.push((left + r, bottom));
    out.extend(corner(left + r, bottom - r, std::f32::consts::FRAC_PI_2));
    out.push((left, top + r));
    out.extend(corner(left + r, top + r, std::f32::consts::PI));
    out.push((left + r, top));
    out
}

/// A five-pointed star inside the box, drawn in one line as a child draws it.
fn star(cx: f32, cy: f32, rx: f32, ry: f32) -> Vec<(f32, f32)> {
    let inner = 0.382; // the ratio that makes a five-pointed star look right
    (0..=10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 10.0 * std::f32::consts::TAU;
            let (sx, sy) = if i % 2 == 0 { (rx, ry) } else { (rx * inner, ry * inner) };
            (cx + sx * a.cos(), cy + sy * a.sin())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_fit_cleans_up_a_rotated_square_without_snapping_curves_or_open_shapes() {
        let tilt = 0.37_f32;
        let (sin, cos) = tilt.sin_cos();
        let rough = [(0.0, 0.0), (22.0, -2.0), (44.0, 1.0), (42.0, 23.0), (45.0, 42.0), (21.0, 44.0), (0.0, 43.0), (-2.0, 22.0), (2.0, 2.0)];
        let drawn: Vec<_> = rough.into_iter().map(|(x, y)| Dab::new(90.0 + x * cos - y * sin, 30.0 + x * sin + y * cos, 0.4)).collect();
        let fitted = trace_square(&drawn, 1.0, 0.6).expect("a rough square");
        assert_eq!(fitted.first(), fitted.last());
        assert!(fitted.iter().all(|dab| dab.pressure == 0.6));
        let corners: Vec<_> = fitted.windows(3).filter_map(|w| (w[0] == w[1] && w[1] == w[2]).then_some(w[0])).collect();
        assert_eq!(corners.len(), 5);
        let side = distance(corners[0], corners[1]);
        assert!(corners.windows(2).all(|w| (distance(w[0], w[1]) - side).abs() < 0.001));
        assert!((cross(corners[0], corners[1], corners[2]).abs() - side * side).abs() < 0.01);
        assert!(trace_square(&drawn[..7], 1.0, 0.6).is_none());
        assert!(trace_square(&trace(Shape::Circle, (0.0, 0.0), (70.0, 70.0), 0.7), 1.0, 0.6).is_none());
        assert!(trace_square(&trace(Shape::Triangle, (0.0, 0.0), (70.0, 70.0), 0.7), 1.0, 0.6).is_none());
        assert!(trace_square(&trace(Shape::Rectangle, (0.0, 0.0), (64.0, 42.0), 0.7), 1.0, 0.6).is_none());
    }

    fn box_of(dabs: &[Dab]) -> (f32, f32, f32, f32) {
        let left = dabs.iter().map(|d| d.x).fold(f32::MAX, f32::min);
        let right = dabs.iter().map(|d| d.x).fold(f32::MIN, f32::max);
        let top = dabs.iter().map(|d| d.y).fold(f32::MAX, f32::min);
        let bottom = dabs.iter().map(|d| d.y).fold(f32::MIN, f32::max);
        (left, top, right, bottom)
    }

    #[test]
    fn every_shape_stays_inside_the_drag_and_is_drawn_in_one_line() {
        for shape in Shape::ALL {
            let dabs = trace(shape, (10.0, 20.0), (110.0, 120.0), 0.6);
            assert!(dabs.len() >= 2, "{}: a shape has a line to it", shape.id());
            let (left, top, right, bottom) = box_of(&dabs);
            assert!(
                left >= 9.9 && top >= 19.9 && right <= 110.1 && bottom <= 120.1,
                "{} stays in the drag: {left},{top} {right},{bottom}",
                shape.id()
            );
            assert!(dabs.iter().all(|d| (d.pressure - 0.6).abs() < 0.001), "{}: an even line", shape.id());
            if shape.closed() {
                let (first, last) = (dabs[0], dabs[dabs.len() - 1]);
                assert!((first.x - last.x).abs() < 0.5 && (first.y - last.y).abs() < 0.5, "{} comes back to where it started", shape.id());
            }
        }
    }

    #[test]
    fn a_square_a_circle_and_a_star_are_even_however_the_drag_went() {
        for shape in [Shape::Square, Shape::Circle] {
            let dabs = trace(shape, (0.0, 0.0), (200.0, 50.0), 0.5);
            let (left, top, right, bottom) = box_of(&dabs);
            let (w, h) = (right - left, bottom - top);
            assert!((w - h).abs() < 1.0, "{}: {w} by {h} is not even", shape.id());
        }
        // A star is drawn in an even circle, but its own box is not square: the top point
        // reaches the edge and the bottom two do not. What matters is that the drag's
        // longer side did not stretch it.
        let star = trace(Shape::Star, (0.0, 0.0), (200.0, 50.0), 0.5);
        let (left, top, right, bottom) = box_of(&star);
        assert!(right - left <= 50.5 && bottom - top <= 50.5, "the star kept to the shorter side: {left},{top} {right},{bottom}");
        // And the shorter side follows the way the drag went, up and to the left as well.
        let up = trace(Shape::Square, (100.0, 100.0), (20.0, 0.0), 0.5);
        let (left, top, right, bottom) = box_of(&up);
        assert!((right - left - 80.0).abs() < 1.0 && (bottom - top - 80.0).abs() < 1.0, "{left},{top} {right},{bottom}");
        assert!(right <= 100.5 && bottom <= 100.5, "drawn up and to the left of where it started");
    }

    #[test]
    fn a_drag_that_went_nowhere_leaves_nothing() {
        for shape in Shape::ALL {
            assert!(trace(shape, (5.0, 5.0), (5.4, 5.2), 0.5).is_empty(), "{}: a click is not a shape", shape.id());
        }
        assert!(trace(Shape::Line, (f32::NAN, 0.0), (10.0, 10.0), 0.5).is_empty(), "nonsense in, nothing out");
    }

    #[test]
    fn shapes_are_named_the_same_way_coming_and_going() {
        for shape in Shape::ALL {
            assert_eq!(Shape::parse(shape.id()), Some(shape));
            assert_eq!(Shape::parse(&shape.id().to_uppercase()), Some(shape));
        }
        assert_eq!(Shape::parse("hexagon"), None);
    }

    #[test]
    fn thin_rounded_rectangles_clamp_corner_radius_and_never_invert() {
        let cases: [((f32, f32), (f32, f32)); 10] = [
            // Thin horizontal bands (including zero height)
            ((10.0, 20.0), (110.0, 20.0)),
            ((10.0, 20.0), (110.0, 20.5)),
            ((10.0, 20.0), (110.0, 21.0)),
            ((10.0, 20.0), (110.0, 22.0)),
            ((10.0, 20.0), (110.0, 23.5)),
            ((10.0, 20.0), (110.0, 24.0)),
            // Thin vertical bands (including zero width and reversed drag)
            ((20.0, 10.0), (20.0, 110.0)),
            ((20.0, 10.0), (20.5, 110.0)),
            ((22.0, 110.0), (20.0, 10.0)),
            ((23.5, 10.0), (20.0, 110.0)),
        ];
        for (from, to) in cases {
            let (left, right) = (from.0.min(to.0), from.0.max(to.0));
            let (top, bottom) = (from.1.min(to.1), from.1.max(to.1));
            let pts = rounded(left, top, right, bottom);
            assert!(pts.len() >= 5, "non-empty outline for {from:?}..{to:?}");
            for &(x, y) in &pts {
                assert!(
                    x.is_finite() && y.is_finite() && x >= left - 1e-4 && x <= right + 1e-4 && y >= top - 1e-4 && y <= bottom + 1e-4,
                    "rounded rect {from:?}..{to:?} escaped bounds at ({x}, {y})"
                );
            }
            // In screen coordinates (+y down), a non-inverted clockwise outline has non-negative
            // signed area, and a thin non-zero-area rect has strictly positive area.
            let signed_area2: f32 = pts.windows(2).map(|w| w[0].0 * w[1].1 - w[1].0 * w[0].1).sum();
            let width = right - left;
            let height = bottom - top;
            if width > 0.0 && height > 0.0 {
                assert!(signed_area2 > 0.0, "inverted/bow-tie rounded rect for {from:?}..{to:?}: 2*area={signed_area2}");
                // Check straight side endpoints never cross over each other.
                assert!(pts[0].0 <= pts[1].0 + 1e-4, "top edge reversed for {from:?}..{to:?}: {:?} -> {:?}", pts[0], pts[1]);
                assert!(pts[8].1 <= pts[9].1 + 1e-4, "right edge reversed for {from:?}..{to:?}: {:?} -> {:?}", pts[8], pts[9]);
                assert!(pts[16].0 >= pts[17].0 - 1e-4, "bottom edge reversed for {from:?}..{to:?}: {:?} -> {:?}", pts[16], pts[17]);
                assert!(pts[24].1 >= pts[25].1 - 1e-4, "left edge reversed for {from:?}..{to:?}: {:?} -> {:?}", pts[24], pts[25]);
            }

            let dabs = trace(Shape::RoundedRectangle, from, to, 0.7);
            assert!(dabs.len() >= 2);
            let (dl, dt, dr, db) = box_of(&dabs);
            assert!(
                dl >= left - 1e-3 && dr <= right + 1e-3 && dt >= top - 1e-3 && db <= bottom + 1e-3,
                "traced dabs escaped box for {from:?}..{to:?}: {dl},{dt}..{dr},{db}"
            );
        }

        // Normal rounded rectangles keep their 20% radius (clamped to [2, 40]).
        let normal = rounded(0.0, 0.0, 100.0, 100.0);
        assert!((normal[0].0 - 20.0).abs() < 1e-4 && (normal[1].0 - 80.0).abs() < 1e-4);
        let large = rounded(0.0, 0.0, 300.0, 300.0);
        assert!((large[0].0 - 40.0).abs() < 1e-4 && (large[1].0 - 260.0).abs() < 1e-4);
    }
}
