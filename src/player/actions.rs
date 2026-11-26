//! What the player's buttons and keys do: pausing, seeking, volume, speed,
//! panels, skipping a marked part and moving between marks and episodes.

use gpui::Context;

use super::markers::{Direction, SkipTarget};
use super::panels::Panel;
use super::{Player, format_time, view};

/// The speeds the speed keys and buttons step through.
const SPEEDS: [f64; 7] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];
/// Normal speed's place in `SPEEDS`: where stepping starts from an odd speed.
const NORMAL_SPEED: usize = 2;

/// The speed one step from `speed` in `direction` (-1 or 1), within `SPEEDS`.
fn next_speed(speed: f64, direction: i32) -> f64 {
    let current = SPEEDS
        .iter()
        .position(|s| (s - speed).abs() < 0.01)
        .unwrap_or(NORMAL_SPEED);
    let next = (current as i32 + direction).clamp(0, SPEEDS.len() as i32 - 1) as usize;
    SPEEDS[next]
}

impl Player {
    pub(super) fn toggle_pause(&mut self) {
        if let Some(play) = self.stream.toggle_play_after_stall() {
            // Applied once the stream is back.
            self.status.paused = !play;
            return;
        }
        self.video.toggle_pause();
        self.status.paused = !self.status.paused;
    }

    pub(super) fn skip(&mut self, seconds: f64) {
        self.video.seek_by(seconds);
    }

    pub(super) fn set_volume(&mut self, volume: f64) {
        let volume = volume.clamp(0.0, 100.0);
        self.video.set_volume(volume);
        self.status.volume = volume;
        if self.status.muted && volume > 0.0 {
            self.video.set_muted(false);
            self.status.muted = false;
        }
    }

    pub(super) fn toggle_mute(&mut self) {
        self.status.muted = !self.status.muted;
        self.video.set_muted(self.status.muted);
    }

    pub(super) fn step_speed(&mut self, direction: i32) {
        let speed = next_speed(self.status.speed, direction);
        self.video.set_speed(speed);
        self.status.speed = speed;
        self.toast(format!("Speed {}", view::format_speed(speed)));
    }

    /// Opens `panel`, or closes it if it's already open.
    pub(super) fn toggle_panel(&mut self, panel: Panel) {
        match self.panels.toggle(panel) {
            Some(Panel::Tracks) => self.tracks.read(&self.video),
            Some(Panel::Episodes) => {
                if let Some(season) = self.playing.current_season() {
                    self.panels.set_season(season);
                }
            }
            _ => {}
        }
    }

    /// What the skip button offers now, while a part can be skipped.
    pub(super) fn skip_target(&self) -> Option<SkipTarget> {
        self.markers.skip_target(self.status.position)
    }

    pub(super) fn skip_segment(&mut self) {
        if let Some(target) = self.skip_target() {
            self.video.seek_to(target.end);
            self.status.position = target.end;
        }
    }

    pub(super) fn play_next(&mut self, cx: &mut Context<Self>) {
        if self.up_next.is_counting() {
            // Leaving during the credits: this episode counts as watched.
            self.status.position = self.status.duration;
        }
        match self.playing.next().map(|e| e.key.clone()) {
            Some(key) => self.play_episode(key, cx),
            None => self.toast("This is the last episode"),
        }
    }

    /// Jumps to the next mark, or back to the previous one. False if
    /// there's none that way.
    pub(super) fn jump_to_mark(&mut self, direction: Direction) -> bool {
        let position = self.status.position;
        let target = self
            .markers
            .mark_from(position, self.status.duration, direction);
        let Some(mark) = target else {
            let side = match direction {
                Direction::Forward => "after",
                Direction::Back => "before",
            };
            eprintln!("[player] no mark {side} {position:.1} s");
            return false;
        };
        let (at, name) = (mark.at, mark.name);
        eprintln!("[player] mark: {position:.1} s -> {at:.1} s ({name})");
        self.video.seek_to(at);
        self.status.position = at;
        self.up_next.cancel();
        self.toast(format!("{name} · {}", format_time(at)));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_steps_stop_at_the_ends() {
        assert_eq!(next_speed(1.0, 1), 1.25);
        assert_eq!(next_speed(1.0, -1), 0.75);
        assert_eq!(next_speed(2.0, 1), 2.0);
        assert_eq!(next_speed(0.5, -1), 0.5);
    }

    #[test]
    fn an_odd_speed_steps_from_normal() {
        assert_eq!(next_speed(1.1, 1), 1.25);
        assert_eq!(next_speed(3.0, -1), 0.75);
    }
}
