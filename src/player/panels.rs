//! The panels that open above the control bar, and the season the episodes
//! panel shows.

use std::time::{Duration, Instant};

use crate::motion;

/// A panel above the control bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Panel {
    Tracks,
    Episodes,
    Shortcuts,
}

impl Panel {
    pub(super) const ALL: [Panel; 3] = [Panel::Tracks, Panel::Episodes, Panel::Shortcuts];

    pub(super) fn index(self) -> usize {
        self as usize
    }
}

/// The episodes panel going from one season's list to another's.
#[derive(Debug, Clone, Copy)]
struct SeasonSwitch {
    /// The season shown before.
    from: u16,
    at: Instant,
}

/// A season switch part-way: the old list dissolving into the new one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Dissolve {
    pub(super) from: u16,
    /// 0 (just switched) to 1, eased.
    pub(super) progress: f32,
}

/// How lit `tab`'s season is (0 to 1) while `shown` is the season listed.
pub(super) fn tab_light(tab: u16, shown: u16, dissolve: Option<Dissolve>) -> f32 {
    match dissolve {
        _ if tab == shown => dissolve.map_or(1.0, |dissolve| dissolve.progress),
        Some(dissolve) if tab == dissolve.from => 1.0 - dissolve.progress,
        _ => 0.0,
    }
}

/// Which panel is open, and what the episodes panel shows.
pub(super) struct Panels {
    open: Option<Panel>,
    /// Season shown in the episodes panel.
    season: u16,
    /// The last change of season, while its lists dissolve.
    season_switch: Option<SeasonSwitch>,
}

impl Panels {
    pub(super) fn new(season: u16) -> Self {
        Self {
            open: None,
            season,
            season_switch: None,
        }
    }

    pub(super) fn open(&self) -> Option<Panel> {
        self.open
    }

    pub(super) fn is_open(&self, panel: Panel) -> bool {
        self.open == Some(panel)
    }

    pub(super) fn close(&mut self) {
        self.open = None;
    }

    /// Opens `panel`, or closes it if it's already open. Returns what's open.
    pub(super) fn toggle(&mut self, panel: Panel) -> Option<Panel> {
        self.open = (self.open != Some(panel)).then_some(panel);
        self.open
    }

    pub(super) fn season(&self) -> u16 {
        self.season
    }

    /// Shows `season` in the episodes panel at once (as the panel opens).
    pub(super) fn set_season(&mut self, season: u16) {
        self.season = season;
    }

    /// Switches the episodes panel to `season`, dissolving from the one shown.
    pub(super) fn switch_season(&mut self, season: u16, now: Instant) {
        if self.season != season {
            self.season_switch = Some(SeasonSwitch {
                from: self.season,
                at: now,
            });
            self.season = season;
        }
    }

    /// The season switch under way, if its dissolve is still to be drawn.
    pub(super) fn dissolve(&self) -> Option<Dissolve> {
        let switch = self.season_switch?;
        let t = switch.at.elapsed().as_secs_f32() / motion::OVERLAY.time().as_secs_f32();
        (motion::enabled() && t < 1.0 && switch.from != self.season).then(|| Dissolve {
            from: switch.from,
            progress: motion::ease_in_out(t),
        })
    }

    /// Whether a season switch is within `time` of its start: keep drawing.
    pub(super) fn switching(&self, time: Duration) -> bool {
        self.season_switch
            .is_some_and(|switch| switch.at.elapsed() < time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_toggles_and_replaces_another() {
        let mut panels = Panels::new(1);
        assert_eq!(panels.toggle(Panel::Tracks), Some(Panel::Tracks));
        assert_eq!(panels.toggle(Panel::Episodes), Some(Panel::Episodes));
        assert!(!panels.is_open(Panel::Tracks));
        assert_eq!(panels.toggle(Panel::Episodes), None);
        assert_eq!(panels.open(), None);
    }

    #[test]
    fn picking_the_shown_season_is_not_a_switch() {
        let mut panels = Panels::new(2);
        panels.switch_season(2, Instant::now());
        assert!(panels.season_switch.is_none());
        panels.switch_season(3, Instant::now());
        assert_eq!(panels.season(), 3);
        assert_eq!(panels.season_switch.map(|switch| switch.from), Some(2));
    }

    #[test]
    fn the_highlight_moves_from_the_old_season_to_the_new() {
        let dissolve = Some(Dissolve {
            from: 1,
            progress: 0.25,
        });
        assert_eq!(tab_light(2, 2, dissolve), 0.25);
        assert_eq!(tab_light(1, 2, dissolve), 0.75);
        assert_eq!(tab_light(3, 2, dissolve), 0.0);
        assert_eq!(tab_light(2, 2, None), 1.0);
        assert_eq!(tab_light(1, 2, None), 0.0);
    }
}
