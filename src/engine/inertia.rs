// SPDX-FileCopyrightText: 2026 Mel0nABC <mel0n@mel0n.dev>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Inertia for the hand tool: how far a release keeps scrolling.
//!
//! Nothing here knows GTK: these are pure functions over pixels and
//! milliseconds, so the numbers behind the feel can be checked without a
//! display.

/// Friction of the coast, in 1/s: total travel ≈ velocity / friction.
pub const FLING_FRICTION: f64 = 4.0;

/// Velocity under which the coast is over, in px/s.
pub const FLING_STOP: f64 = 20.0;

/// Span of the drag history the release velocity is measured over, in ms.
/// Long enough to smooth a jittery pointer, short enough to notice that the
/// pointer stopped before the button came up.
pub const FLING_WINDOW_MS: f64 = 100.0;

/// Velocity in px/s that a drag hands over when it ends.
///
/// `samples` are `(time_ms, position)` in increasing time and `now_ms` is the
/// release instant, so the average speed over the last `window_ms` is what the
/// coast carries. Two guards keep the feeling honest: a pointer that stood
/// still for longer than that window hands over no speed at all, and samples
/// that share one instant cannot describe a movement.
pub fn sampled_velocity(samples: &[(f64, f64)], now_ms: f64, window_ms: f64) -> f64 {
    let Some(&(last_at, last_position)) = samples.last() else {
        return 0.0;
    };
    if now_ms - last_at > window_ms {
        return 0.0;
    }

    let window_start = now_ms - window_ms;
    // The oldest sample still inside the window; the whole history when the
    // drag was shorter than the window.
    let (base_at, base_position) = samples
        .iter()
        .rev()
        .find(|(at, _)| *at <= window_start)
        .copied()
        .unwrap_or(samples[0]);
    let elapsed_ms = last_at - base_at;
    if elapsed_ms <= 0.0 {
        return 0.0;
    }
    (last_position - base_position) / (elapsed_ms / 1000.0)
}

/// Velocity of the coast after `dt_ms`, in px/s. Friction is exponential, so
/// the braking is proportional to the speed it still has.
pub fn decayed_velocity(velocity: f64, dt_ms: f64) -> f64 {
    velocity * (-FLING_FRICTION * (dt_ms / 1000.0)).exp()
}

/// `true` when the coast is slow enough that it should be cut.
pub fn fling_finished(velocity: f64) -> bool {
    velocity.abs() <= FLING_STOP
}

/// Clamps `value` to `[lower, upper]`. An inverted range (content smaller than
/// the viewport) is corrected with `upper.max(lower)`: both ends then fall on
/// the minimum.
pub fn clamp_scroll(value: f64, lower: f64, upper: f64) -> f64 {
    value.clamp(lower, upper.max(lower))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples every 16 ms from `0..=duration_ms`, moving at `speed` px/ms.
    fn samples_until(duration_ms: f64, speed: f64) -> Vec<(f64, f64)> {
        let mut samples = Vec::new();
        let mut at = 0.0;
        while at <= duration_ms {
            samples.push((at, at * speed));
            at += 16.0;
        }
        samples
    }

    #[test]
    fn release_speed_is_the_average_of_the_last_window() {
        let samples = samples_until(320.0, 0.4);
        let velocity = sampled_velocity(&samples, 320.0, FLING_WINDOW_MS);
        assert!(
            (velocity - 400.0).abs() < 20.0,
            "a 400 px/s constant drag hands over about 400: {velocity}"
        );
    }

    #[test]
    fn a_pointer_that_stopped_before_releasing_hands_over_no_speed() {
        let samples = samples_until(320.0, 0.4);
        let velocity = sampled_velocity(&samples, 320.0 + FLING_WINDOW_MS + 1.0, FLING_WINDOW_MS);
        assert_eq!(velocity, 0.0, "el puntero se quedó quieto: sin inercia");
    }

    #[test]
    fn samples_without_elapsed_time_give_no_speed() {
        assert_eq!(sampled_velocity(&[], 0.0, FLING_WINDOW_MS), 0.0);
        assert_eq!(sampled_velocity(&[(10.0, 5.0)], 10.0, FLING_WINDOW_MS), 0.0);
        assert_eq!(
            sampled_velocity(&[(10.0, 5.0), (10.0, 9.0)], 10.0, FLING_WINDOW_MS),
            0.0
        );
    }

    #[test]
    fn only_the_last_window_counts() {
        // 1 px/ms for 200 ms, then 0.1 px/ms for the last 100 ms.
        let mut samples = samples_until(200.0, 1.0);
        let mut at = 200.0;
        while at <= 300.0 {
            samples.push((at, 200.0 + (at - 200.0) * 0.1));
            at += 16.0;
        }
        let velocity = sampled_velocity(&samples, 300.0, FLING_WINDOW_MS);
        assert!(
            (velocity - 100.0).abs() < 15.0,
            "la ventana final manda: {velocity}"
        );
    }

    #[test]
    fn release_speed_keeps_the_direction() {
        let mut samples = samples_until(160.0, 0.4);
        for (_, position) in &mut samples {
            *position = -*position;
        }
        let velocity = sampled_velocity(&samples, 160.0, FLING_WINDOW_MS);
        assert!(velocity < -300.0, "hacia atrás es negativa: {velocity}");
    }

    #[test]
    fn the_coast_brakes_keeps_the_sign_and_travels_velocity_over_friction() {
        let dt_ms = 16.0;
        let dt_s = dt_ms / 1000.0;

        for start in [1000.0_f64, -1000.0] {
            let mut velocity = start;
            let mut travelled = 0.0;
            while !fling_finished(velocity) {
                travelled += velocity * dt_s;
                let next = decayed_velocity(velocity, dt_ms);
                assert!(next.abs() < velocity.abs(), "frena siempre: {velocity} -> {next}");
                assert_eq!(next.signum(), start.signum(), "conserva el signo");
                velocity = next;
            }
            let expected = start / FLING_FRICTION;
            let error = (travelled - expected).abs() / expected.abs();
            assert!(error < 0.05, "recorrido {travelled} contra {expected}");
        }
    }

    #[test]
    fn fling_finished_marks_the_threshold() {
        assert!(fling_finished(0.0));
        assert!(fling_finished(FLING_STOP));
        assert!(fling_finished(-FLING_STOP));
        assert!(!fling_finished(FLING_STOP + 0.5));
        assert!(!fling_finished(-FLING_STOP - 0.5));
    }

    #[test]
    fn clamp_scroll_cuts_both_ends_and_handles_an_inverted_range() {
        assert_eq!(clamp_scroll(-30.0, 0.0, 500.0), 0.0);
        assert_eq!(clamp_scroll(900.0, 0.0, 500.0), 500.0);
        assert_eq!(clamp_scroll(120.0, 0.0, 500.0), 120.0);
        assert_eq!(clamp_scroll(40.0, 500.0, 100.0), 500.0, "rango invertido: al mínimo");
        assert_eq!(clamp_scroll(-40.0, 500.0, 100.0), 500.0, "rango invertido por debajo");
    }
}
