//! Monotonic cubic Bezier pressure-response curves.
//!
//! A curve maps a normalized pen pressure `p` in `0.0..=1.0` to a felt pressure in `0.0..=1.0`
//! before the stroke's gain and tool floor are applied.

/// Normalized pressure response curve.
#[derive(Clone, Copy, Debug, Default)]
pub enum PressureCurve {
    #[default]
    Linear,
    Soft,
    Firm,
    SCurve,
    Custom {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
}

fn norm_bits(v: f32) -> u32 {
    if v == 0.0 {
        0.0f32.to_bits()
    } else if v.is_nan() {
        f32::NAN.to_bits()
    } else {
        v.to_bits()
    }
}

impl PartialEq for PressureCurve {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Linear, Self::Linear) | (Self::Soft, Self::Soft) | (Self::Firm, Self::Firm) | (Self::SCurve, Self::SCurve) => true,
            (Self::Custom { x1: ax1, y1: ay1, x2: ax2, y2: ay2 }, Self::Custom { x1: bx1, y1: by1, x2: bx2, y2: by2 }) => {
                norm_bits(*ax1) == norm_bits(*bx1)
                    && norm_bits(*ay1) == norm_bits(*by1)
                    && norm_bits(*ax2) == norm_bits(*bx2)
                    && norm_bits(*ay2) == norm_bits(*by2)
            }
            _ => false,
        }
    }
}

impl Eq for PressureCurve {}

impl PressureCurve {
    /// Built-in preset curves shown in the UI.
    pub const PRESETS: [PressureCurve; 4] = [PressureCurve::Linear, PressureCurve::Soft, PressureCurve::Firm, PressureCurve::SCurve];

    /// Stable identifier for settings and SVG `nc:curve`.
    pub fn id(self) -> &'static str {
        match self {
            PressureCurve::Linear => "linear",
            PressureCurve::Soft => "soft",
            PressureCurve::Firm => "firm",
            PressureCurve::SCurve => "s-curve",
            PressureCurve::Custom { .. } => "custom",
        }
    }

    /// Human-readable preset label.
    pub fn label(self) -> &'static str {
        match self {
            PressureCurve::Linear => "Linear",
            PressureCurve::Soft => "Soft",
            PressureCurve::Firm => "Firm",
            PressureCurve::SCurve => "S-curve",
            PressureCurve::Custom { .. } => "Custom",
        }
    }

    /// Serializes the curve for settings or `nc:curve`.
    pub fn write(self) -> String {
        match self.sane() {
            PressureCurve::Linear => "linear".to_string(),
            PressureCurve::Soft => "soft".to_string(),
            PressureCurve::Firm => "firm".to_string(),
            PressureCurve::SCurve => "s-curve".to_string(),
            PressureCurve::Custom { x1, y1, x2, y2 } => format!("custom:{x1:.4},{y1:.4},{x2:.4},{y2:.4}"),
        }
    }

    /// Parses a serialized curve. Returns `None` on unrecognized input.
    pub fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if let Some(preset) = Self::PRESETS.into_iter().find(|c| c.id().eq_ignore_ascii_case(trimmed)) {
            return Some(preset);
        }
        let coords = trimmed.strip_prefix("custom:")?;
        let mut parts = coords.split(',');
        let x1: f32 = parts.next()?.trim().parse().ok()?;
        let y1: f32 = parts.next()?.trim().parse().ok()?;
        let x2: f32 = parts.next()?.trim().parse().ok()?;
        let y2: f32 = parts.next()?.trim().parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        if !x1.is_finite() || !y1.is_finite() || !x2.is_finite() || !y2.is_finite() {
            return Some(PressureCurve::Linear);
        }
        Some(PressureCurve::Custom { x1, y1, x2, y2 }.sane())
    }

    /// Enforces monotonic control points `0 <= x1 <= x2 <= 1` and `0 <= y1 <= y2 <= 1`.
    /// Non-finite coordinates fall back to `Linear`.
    pub fn sane(self) -> Self {
        match self {
            PressureCurve::Custom { x1, y1, x2, y2 } => {
                if !x1.is_finite() || !y1.is_finite() || !x2.is_finite() || !y2.is_finite() {
                    return PressureCurve::Linear;
                }
                let x1 = x1.clamp(0.0, 1.0);
                let x2 = x2.clamp(x1, 1.0);
                let y1 = y1.clamp(0.0, 1.0);
                let y2 = y2.clamp(y1, 1.0);
                PressureCurve::Custom {
                    x1: if x1 == 0.0 { 0.0 } else { x1 },
                    y1: if y1 == 0.0 { 0.0 } else { y1 },
                    x2: if x2 == 0.0 { 0.0 } else { x2 },
                    y2: if y2 == 0.0 { 0.0 } else { y2 },
                }
            }
            preset => preset,
        }
    }

    /// Normalized cubic Bezier control points `(x1, y1, x2, y2)` with `P0 = (0, 0)` and `P3 = (1, 1)`.
    pub fn controls(self) -> (f32, f32, f32, f32) {
        match self.sane() {
            PressureCurve::Linear => (1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0),
            PressureCurve::Soft => (0.18, 0.52, 0.45, 0.88),
            PressureCurve::Firm => (0.55, 0.12, 0.82, 0.48),
            PressureCurve::SCurve => (0.42, 0.06, 0.58, 0.94),
            PressureCurve::Custom { x1, y1, x2, y2 } => (x1, y1, x2, y2),
        }
    }

    /// Evaluates the monotonic curve at pressure `p` in `0.0..=1.0`.
    pub fn eval(self, pressure: f32) -> f32 {
        if !pressure.is_finite() || pressure <= 0.0 {
            return 0.0;
        }
        if pressure >= 1.0 {
            return 1.0;
        }
        if matches!(self, PressureCurve::Linear) {
            return pressure;
        }
        let (x1, y1, x2, y2) = self.controls();
        let mut lo = 0.0_f32;
        let mut hi = 1.0_f32;
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            let x = bezier_1d(x1, x2, mid);
            if x < pressure {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let t = 0.5 * (lo + hi);
        bezier_1d(y1, y2, t).clamp(0.0, 1.0)
    }
}

#[inline]
fn bezier_1d(c1: f32, c2: f32, t: f32) -> f32 {
    let inv = 1.0 - t;
    let inv2 = inv * inv;
    let t2 = t * t;
    3.0 * inv2 * t * c1 + 3.0 * inv * t2 * c2 + t2 * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_and_monotonicity_hold_for_all_presets_and_custom() {
        let custom = PressureCurve::Custom { x1: 0.25, y1: 0.70, x2: 0.75, y2: 0.85 };
        let curves = [PressureCurve::Linear, PressureCurve::Soft, PressureCurve::Firm, PressureCurve::SCurve, custom];
        for curve in curves {
            assert_eq!(curve.eval(0.0), 0.0, "{curve:?} at 0");
            assert_eq!(curve.eval(1.0), 1.0, "{curve:?} at 1");
            assert_eq!(curve.eval(-0.5), 0.0, "{curve:?} below 0");
            assert_eq!(curve.eval(1.5), 1.0, "{curve:?} above 1");
            assert_eq!(curve.eval(f32::NAN), 0.0, "{curve:?} NaN");
            assert_eq!(curve.eval(f32::INFINITY), 0.0, "{curve:?} +inf");
            assert_eq!(curve.eval(f32::NEG_INFINITY), 0.0, "{curve:?} -inf");

            let mut prev = 0.0_f32;
            for step in 0..=200 {
                let p = step as f32 / 200.0;
                let y = curve.eval(p);
                assert!(y.is_finite() && (0.0..=1.0).contains(&y), "{curve:?} out of range at {p}: {y}");
                assert!(y + 1e-6 >= prev, "{curve:?} non-monotonic at {p}: {y} < {prev}");
                prev = y;
            }
        }
    }

    #[test]
    fn presets_produce_distinct_mid_pressure_responses() {
        let p = 0.35;
        let soft = PressureCurve::Soft.eval(p);
        let linear = PressureCurve::Linear.eval(p);
        let firm = PressureCurve::Firm.eval(p);
        assert!(soft > linear + 0.1, "soft {soft} should lift light pressure above linear {linear}");
        assert!(firm < linear - 0.1, "firm {firm} should keep light pressure below linear {linear}");
        assert!(PressureCurve::SCurve.eval(0.15) < PressureCurve::Linear.eval(0.15));
        assert!(PressureCurve::SCurve.eval(0.85) > PressureCurve::Linear.eval(0.85));
    }

    #[test]
    fn non_finite_and_inverted_custom_coordinates_are_sanitized() {
        assert_eq!(PressureCurve::Custom { x1: f32::NAN, y1: 0.2, x2: 0.5, y2: 0.8 }.sane(), PressureCurve::Linear);
        assert_eq!(PressureCurve::Custom { x1: 0.2, y1: f32::INFINITY, x2: 0.5, y2: 0.8 }.sane(), PressureCurve::Linear);

        let clamped = PressureCurve::Custom { x1: 0.8, y1: 0.9, x2: 0.2, y2: -0.4 }.sane();
        assert_eq!(clamped, PressureCurve::Custom { x1: 0.8, y1: 0.9, x2: 0.8, y2: 0.9 });
        let (x1, y1, x2, y2) = clamped.controls();
        assert!(x1 <= x2 && y1 <= y2 && (0.0..=1.0).contains(&x1) && (0.0..=1.0).contains(&y2));
    }

    #[test]
    fn curve_round_trips_through_string_serialization() {
        for curve in PressureCurve::PRESETS {
            assert_eq!(PressureCurve::parse(&curve.write()), Some(curve));
        }
        let custom = PressureCurve::Custom { x1: 0.25, y1: 0.40, x2: 0.65, y2: 0.90 };
        let parsed = PressureCurve::parse(&custom.write()).expect("parse custom");
        assert_eq!(parsed, custom);
        assert_eq!(PressureCurve::parse("nonsense"), None);
        assert_eq!(PressureCurve::parse("custom:0.1,0.2"), None);
        assert_eq!(PressureCurve::parse("custom:0.1,0.2,0.3,0.4,0.5"), None);
    }
}
