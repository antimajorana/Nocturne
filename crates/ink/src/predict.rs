//! Velocity-based lookahead stroke prediction for low-latency ink preview.
//!
//! Predicted dabs are strictly ephemeral preview geometry and are never added to a
//! committed [`crate::Stroke`].

use crate::Dab;

/// A pen reading stamped with the monotonic UI time in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedDab {
    pub dab: Dab,
    pub time_secs: f64,
}

/// Tuning parameters for lookahead stroke prediction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PredictionConfig {
    /// How far ahead in seconds to project the live stroke tip.
    pub lookahead_secs: f32,
    /// Hard upper bound on total predicted distance in page points.
    pub max_distance: f32,
    /// Number of extrapolated preview dabs to emit.
    pub steps: usize,
    /// Minimum pen speed (points/sec) below which prediction produces nothing.
    pub min_speed: f32,
    /// Speed cap (points/sec) to keep wild spikes bounded.
    pub max_speed: f32,
}

impl Default for PredictionConfig {
    fn default() -> Self {
        Self { lookahead_secs: 0.014, max_distance: 20.0, steps: 3, min_speed: 25.0, max_speed: 3500.0 }
    }
}

fn valid_sample(sample: TimedDab) -> bool {
    sample.time_secs.is_finite() && sample.dab.x.is_finite() && sample.dab.y.is_finite() && sample.dab.pressure.is_finite()
}

/// Predicts a short, corner-damped lookahead tail from recent timed pen readings.
///
/// Returns an empty vector when history is insufficient, stale, stationary, non-finite,
/// or turning sharply at a corner/cusp.
pub fn predict_tail(history: &[TimedDab], config: PredictionConfig) -> Vec<Dab> {
    if history.len() < 2
        || config.steps == 0
        || !config.lookahead_secs.is_finite()
        || config.lookahead_secs <= 0.0
        || !config.max_distance.is_finite()
        || config.max_distance <= 0.0
    {
        return Vec::new();
    }

    let last = history[history.len() - 1];
    let prev = history[history.len() - 2];
    if !valid_sample(last) || !valid_sample(prev) {
        return Vec::new();
    }

    let dt = last.time_secs - prev.time_secs;
    if !(1e-4..=0.1).contains(&dt) {
        return Vec::new();
    }
    #[allow(clippy::cast_possible_truncation)]
    let dt_f32 = dt as f32;
    let v_last = ((last.dab.x - prev.dab.x) / dt_f32, (last.dab.y - prev.dab.y) / dt_f32);
    let speed_last = v_last.0.hypot(v_last.1);
    if !speed_last.is_finite() {
        return Vec::new();
    }

    let mut turn_factor = 1.0_f32;
    let mut vx = v_last.0;
    let mut vy = v_last.1;

    if history.len() >= 3 {
        let prev2 = history[history.len() - 3];
        if valid_sample(prev2) {
            let dt2 = prev.time_secs - prev2.time_secs;
            if (1e-4..=0.1).contains(&dt2) {
                #[allow(clippy::cast_possible_truncation)]
                let dt2_f32 = dt2 as f32;
                let v_prev = ((prev.dab.x - prev2.dab.x) / dt2_f32, (prev.dab.y - prev2.dab.y) / dt2_f32);
                let speed_prev = v_prev.0.hypot(v_prev.1);
                if speed_prev.is_finite() && speed_prev > 1e-3 && speed_last > 1e-3 {
                    let u_prev = (v_prev.0 / speed_prev, v_prev.1 / speed_prev);
                    let u_last = (v_last.0 / speed_last, v_last.1 / speed_last);
                    let cos_theta = (u_prev.0 * u_last.0 + u_prev.1 * u_last.1).clamp(-1.0, 1.0);
                    if !cos_theta.is_finite() {
                        return Vec::new();
                    }
                    if cos_theta < 0.5 {
                        // Sharp corner or reversal: damp prediction cleanly toward zero.
                        turn_factor = (cos_theta.max(0.0) / 0.5).clamp(0.0, 1.0);
                    } else {
                        vx = v_prev.0 * 0.35 + v_last.0 * 0.65;
                        vy = v_prev.1 * 0.35 + v_last.1 * 0.65;
                    }
                }
            }
        }
    }

    let speed = vx.hypot(vy);
    let min_speed = if config.min_speed.is_finite() { config.min_speed.max(0.0) } else { 25.0 };
    let max_speed = if config.max_speed.is_finite() { config.max_speed.max(min_speed) } else { 3500.0 };
    if !speed.is_finite() || speed < min_speed || speed_last < min_speed || turn_factor <= 0.0 {
        return Vec::new();
    }

    let cap_scale = (max_speed / speed).min(1.0);
    vx *= cap_scale;
    vy *= cap_scale;

    let mut dx = vx * config.lookahead_secs * turn_factor;
    let mut dy = vy * config.lookahead_secs * turn_factor;
    let dist = dx.hypot(dy);
    if !dist.is_finite() || dist < 0.5 {
        return Vec::new();
    }
    if dist > config.max_distance {
        let clamp = config.max_distance / dist;
        dx *= clamp;
        dy *= clamp;
    }

    let pressure = last.dab.pressure.clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(config.steps);
    for k in 1..=config.steps {
        #[allow(clippy::cast_precision_loss)]
        let u = k as f32 / config.steps as f32;
        // Monotonic decelerating taper: u * (1.35 - 0.35 * u) goes 0 -> 1 cleanly.
        let taper = u * (1.35 - 0.35 * u);
        out.push(Dab::new(last.dab.x + dx * taper, last.dab.y + dy * taper, pressure));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn td(x: f32, y: f32, p: f32, t: f64) -> TimedDab {
        TimedDab { dab: Dab { x, y, pressure: p }, time_secs: t }
    }

    #[test]
    fn stationary_single_point_and_stale_histories_produce_no_prediction() {
        let cfg = PredictionConfig::default();
        assert!(predict_tail(&[], cfg).is_empty());
        assert!(predict_tail(&[td(10.0, 10.0, 0.5, 1.0)], cfg).is_empty());
        // Stationary
        assert!(predict_tail(&[td(10.0, 10.0, 0.5, 1.0), td(10.0, 10.0, 0.5, 1.01)], cfg).is_empty());
        // Below min_speed (1 pt in 0.1s = 10 pt/s < 25)
        assert!(predict_tail(&[td(10.0, 10.0, 0.5, 1.0), td(11.0, 10.0, 0.5, 1.1)], cfg).is_empty());
        // Stale interval > 0.1s
        assert!(predict_tail(&[td(0.0, 0.0, 0.5, 1.0), td(100.0, 0.0, 0.5, 1.25)], cfg).is_empty());
        // Non-increasing timestamp
        assert!(predict_tail(&[td(0.0, 0.0, 0.5, 1.0), td(100.0, 0.0, 0.5, 1.0)], cfg).is_empty());
    }

    #[test]
    fn fast_motion_predicts_forward_within_max_distance() {
        let cfg = PredictionConfig::default();
        let history = [td(0.0, 20.0, 0.6, 1.000), td(10.0, 20.0, 0.6, 1.010), td(20.0, 20.0, 0.6, 1.020)];
        let tail = predict_tail(&history, cfg);
        assert_eq!(tail.len(), cfg.steps);
        let mut prev_x = 20.0;
        for dab in &tail {
            assert!(dab.x > prev_x, "predicted tip should move forward: {dab:?}");
            assert!((dab.y - 20.0).abs() < 0.01);
            assert!((dab.pressure - 0.6).abs() < 0.01);
            assert!((dab.x - 20.0).hypot(dab.y - 20.0) <= cfg.max_distance + 0.01);
            prev_x = dab.x;
        }

        // Ultra-fast spike is bounded by max_distance.
        let extreme = [td(0.0, 0.0, 0.8, 1.000), td(500.0, 0.0, 0.8, 1.001)];
        let bounded = predict_tail(&extreme, cfg);
        assert!(!bounded.is_empty());
        let tip = bounded.last().expect("tip");
        assert!((tip.x - 500.0).hypot(tip.y) <= cfg.max_distance + 0.01);
    }

    #[test]
    fn sharp_corner_and_reversal_damp_overshoot() {
        let cfg = PredictionConfig::default();
        // 180-degree reversal -> 0 tail
        let reverse = [td(0.0, 0.0, 0.5, 1.000), td(20.0, 0.0, 0.5, 1.010), td(0.0, 0.0, 0.5, 1.020)];
        assert!(predict_tail(&reverse, cfg).is_empty());

        // 90-degree corner (cos_theta == 0) -> 0 tail
        let right_angle = [td(0.0, 0.0, 0.5, 1.000), td(20.0, 0.0, 0.5, 1.010), td(20.0, 20.0, 0.5, 1.020)];
        assert!(predict_tail(&right_angle, cfg).is_empty());
    }

    #[test]
    fn non_finite_inputs_never_produce_nan_or_infinite_dabs() {
        let cfg = PredictionConfig::default();
        let bad = [td(f32::NAN, 0.0, 0.5, 1.0), td(10.0, f32::INFINITY, 0.5, 1.01)];
        assert!(predict_tail(&bad, cfg).is_empty());

        let bad_time = [td(0.0, 0.0, 0.5, f64::NAN), td(10.0, 0.0, 0.5, 1.01)];
        assert!(predict_tail(&bad_time, cfg).is_empty());

        let bad_cfg = PredictionConfig { lookahead_secs: f32::NAN, max_distance: 10.0, steps: 3, min_speed: 10.0, max_speed: 100.0 };
        assert!(predict_tail(&[td(0.0, 0.0, 0.5, 1.0), td(10.0, 0.0, 0.5, 1.01)], bad_cfg).is_empty());
    }

    #[test]
    fn decelerating_below_min_speed_and_extreme_hairpin_produce_no_prediction() {
        let cfg = PredictionConfig::default();
        // Fast segment (2000 pt/s) followed by slow tail (10 pt/s < min_speed = 25 pt/s) must not overshoot.
        let decel = [td(0.0, 0.0, 0.5, 1.00), td(20.0, 0.0, 0.5, 1.01), td(20.1, 0.0, 0.5, 1.02)];
        assert!(predict_tail(&decel, cfg).is_empty());

        // Extreme-coordinate 180-degree hairpin reversal must not overflow dot product to NaN.
        let extreme_hairpin = [td(0.0, 0.0, 0.5, 1.000), td(1e17, 0.0, 0.5, 1.001), td(0.0, 0.0, 0.5, 1.002)];
        assert!(predict_tail(&extreme_hairpin, cfg).is_empty());
    }
}
