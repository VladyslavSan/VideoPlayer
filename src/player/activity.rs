//! What keeps the controls on screen: recent mouse or key activity, the mouse
//! resting on the bar, and how far the mouse must move to bring them back.

use std::time::{Duration, Instant};

use gpui::{Pixels, Point};

/// How long the controls stay after the last activity.
const CONTROLS_TIMEOUT: Duration = Duration::from_millis(2500);
/// While the controls are hidden, the mouse has to travel this far within
/// `REVEAL_WINDOW` to bring them back, so small nudges don't.
const REVEAL_DISTANCE: f32 = 40.0;
const REVEAL_WINDOW: Duration = Duration::from_millis(500);

/// Where and when the mouse started moving while the controls were hidden.
#[derive(Debug, Clone, Copy)]
struct RevealStart {
    origin: Point<Pixels>,
    since: Instant,
}

/// The viewer's recent activity.
pub(super) struct Activity {
    last: Instant,
    /// The controls come back once the mouse has moved far enough from here.
    reveal_from: Option<RevealStart>,
    bar_hovered: bool,
}

impl Activity {
    pub(super) fn new() -> Self {
        Self {
            last: Instant::now(),
            reveal_from: None,
            bar_hovered: false,
        }
    }

    /// The viewer did something: the controls stay a while longer.
    pub(super) fn poke(&mut self) {
        self.last = Instant::now();
    }

    pub(super) fn set_bar_hovered(&mut self, hovered: bool) {
        self.bar_hovered = hovered;
    }

    /// Whether activity alone keeps the controls up: the mouse rests on the
    /// bar, or something happened a moment ago.
    pub(super) fn keeps_controls(&self) -> bool {
        self.bar_hovered || self.last.elapsed() < CONTROLS_TIMEOUT
    }

    /// Hides the controls now, as if the mouse had been idle.
    pub(super) fn hide_controls(&mut self) {
        self.bar_hovered = false;
        self.last = Instant::now()
            .checked_sub(CONTROLS_TIMEOUT)
            .unwrap_or_else(Instant::now);
    }

    /// Whether a mouse move to `position` should bring hidden controls back:
    /// only a deliberate one, not a nudge or jitter.
    pub(super) fn reveals(&mut self, position: Point<Pixels>, now: Instant) -> bool {
        match self.reveal_from {
            Some(start) if now.duration_since(start.since) < REVEAL_WINDOW => {
                let (dx, dy) = (
                    f32::from(position.x - start.origin.x),
                    f32::from(position.y - start.origin.y),
                );
                if dx.hypot(dy) >= REVEAL_DISTANCE {
                    self.reveal_from = None;
                    true
                } else {
                    false
                }
            }
            // Start measuring from here (also after a pause, so slow drift
            // never adds up).
            _ => {
                self.reveal_from = Some(RevealStart {
                    origin: position,
                    since: now,
                });
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, px};

    use super::*;

    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    #[test]
    fn a_nudge_does_not_bring_the_controls_back() {
        let mut activity = Activity::new();
        let now = Instant::now();
        assert!(!activity.reveals(at(100.0, 100.0), now));
        assert!(!activity.reveals(at(120.0, 110.0), now + Duration::from_millis(100)));
    }

    #[test]
    fn a_deliberate_move_brings_the_controls_back() {
        let mut activity = Activity::new();
        let now = Instant::now();
        assert!(!activity.reveals(at(100.0, 100.0), now));
        // 30 across and 40 down is 50 away.
        assert!(activity.reveals(at(130.0, 140.0), now + Duration::from_millis(200)));
        // Measuring starts over after that.
        assert!(!activity.reveals(at(400.0, 400.0), now + Duration::from_millis(250)));
    }

    #[test]
    fn slow_drift_never_adds_up() {
        let mut activity = Activity::new();
        let now = Instant::now();
        assert!(!activity.reveals(at(100.0, 100.0), now));
        assert!(!activity.reveals(at(130.0, 100.0), now + Duration::from_millis(600)));
        assert!(!activity.reveals(at(160.0, 100.0), now + Duration::from_millis(1200)));
    }

    #[test]
    fn hiding_the_controls_ends_what_kept_them_up() {
        let mut activity = Activity::new();
        activity.set_bar_hovered(true);
        assert!(activity.keeps_controls());
        activity.hide_controls();
        assert!(!activity.keeps_controls());
        activity.poke();
        assert!(activity.keeps_controls());
    }
}
