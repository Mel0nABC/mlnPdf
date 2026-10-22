//! Discrete zoom steps and the Ctrl+wheel accumulator.

/// Discrete zoom levels, smallest first. 1.0 draws the page at its point size.
pub const ZOOM_STEPS: [f64; 8] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0];

pub const ZOOM_DEFAULT: f64 = 1.0;

/// Accumulated scroll delta needed before one zoom step, so a trackpad gesture
/// does not change the zoom on every event while a single mouse-wheel notch
/// (delta 0.5 or 1.0 depending on the toolkit) still counts as one step.
pub const SCROLL_ZOOM_THRESHOLD: f64 = 0.5;

/// Next larger zoom step, clamped at the top of `ZOOM_STEPS`.
pub fn next_zoom(current: f64) -> f64 {
    ZOOM_STEPS
        .iter()
        .copied()
        .find(|step| *step > current + f64::EPSILON)
        .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
}

/// Next smaller zoom step, clamped at the bottom of `ZOOM_STEPS`.
pub fn previous_zoom(current: f64) -> f64 {
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|step| *step < current - f64::EPSILON)
        .unwrap_or(ZOOM_STEPS[0])
}

/// Turns accumulated Ctrl+scroll motion into at most one zoom step.
///
/// Returns the new accumulator and, when the threshold was crossed, the
/// direction: `-1` zoom in, `1` zoom out. A direction change discards the
/// leftover so opposite gestures never cancel each other.
pub fn scroll_zoom_step(accumulated: f64, dy: f64) -> (f64, Option<i8>) {
    if dy == 0.0 {
        return (accumulated, None);
    }

    let same_direction = accumulated == 0.0 || accumulated.signum() == dy.signum();
    let total = if same_direction { accumulated + dy } else { dy };

    if total.abs() < SCROLL_ZOOM_THRESHOLD {
        (total, None)
    } else if total > 0.0 {
        (0.0, Some(1))
    } else {
        (0.0, Some(-1))
    }
}

pub fn zoom_text(zoom: f64) -> String {
    format!("{:.0} %", zoom * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_advance_one_level_and_clamp() {
        assert_eq!(next_zoom(1.0), 1.5);
        assert_eq!(next_zoom(1.5), 2.0);
        assert_eq!(next_zoom(ZOOM_STEPS[ZOOM_STEPS.len() - 1]), ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        assert_eq!(previous_zoom(1.0), 0.75);
        assert_eq!(previous_zoom(ZOOM_STEPS[0]), ZOOM_STEPS[0]);
    }

    #[test]
    fn a_value_between_steps_snaps_to_the_next_one() {
        assert_eq!(next_zoom(1.2), 1.5);
        assert_eq!(previous_zoom(1.2), 1.0);
    }

    #[test]
    fn scroll_accumulates_and_discards_on_direction_change() {
        assert_eq!(scroll_zoom_step(0.0, 0.4), (0.4, None));
        assert_eq!(scroll_zoom_step(0.4, 0.6), (0.0, Some(1)));
        assert_eq!(scroll_zoom_step(0.0, -1.0), (0.0, Some(-1)));
        assert_eq!(scroll_zoom_step(0.7, -0.2), (-0.2, None));
        assert_eq!(scroll_zoom_step(0.7, 0.0), (0.7, None));
    }

    #[test]
    fn text_is_a_rounded_percentage() {
        assert_eq!(zoom_text(1.0), "100 %");
        assert_eq!(zoom_text(0.5), "50 %");
        assert_eq!(zoom_text(1.47), "147 %");
    }
}
