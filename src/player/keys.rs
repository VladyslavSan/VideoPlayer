//! The keyboard: where key presses go, the viewer's keys, and what each does.

use gpui::{Context, FocusHandle, KeyDownEvent, Window};

use super::Player;
use super::controls::{Action, Controls};
use super::markers::Direction;
use super::panels::Panel;
use crate::video::TrackKind;

/// The player's keyboard focus and the viewer's keys and skip lengths.
pub(super) struct Input {
    pub(super) focus: FocusHandle,
    pub(super) controls: Controls,
}

/// What became of a key's action.
enum Ran {
    /// Didn't apply now: try the key's next action.
    No,
    Yes,
    /// The player is leaving (for another episode, or back).
    Left,
}

impl Ran {
    /// `Yes` if `done`, else `No`.
    fn when(done: bool) -> Self {
        if done { Ran::Yes } else { Ran::No }
    }
}

impl Player {
    pub(super) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let question_mark = event.keystroke.key_char.as_deref() == Some("?");
        if question_mark {
            self.toggle_panel(Panel::Shortcuts);
            return self.poke(cx);
        }
        let key = event.keystroke.unparse();
        if key == "escape" {
            return self.escape(window, cx);
        }
        // Each action on the key in turn, until one applies (the skip only
        // does while offered).
        for action in self.input.controls.actions_for(&key) {
            match self.run(action, window, cx) {
                Ran::No => continue,
                Ran::Yes => return self.poke(cx),
                Ran::Left => return,
            }
        }
    }

    /// Escape closes the innermost thing open: a menu, a panel, fullscreen,
    /// then the player.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tracks.quality.menu.close() | self.tracks.sound.menu.close() {
            // Closed (fading out).
        } else if self.panels.open().is_some() {
            self.panels.close();
        } else if window.is_fullscreen() {
            window.toggle_fullscreen();
        } else {
            return self.exit(cx);
        }
        self.poke(cx);
    }

    fn run(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) -> Ran {
        let has_episodes = self.playing.has_episodes();
        match action {
            Action::Skip if self.up_next.is_counting() => return self.run_play_next(cx),
            Action::Skip if self.skip_target().is_some() => self.skip_segment(),
            Action::Skip => return Ran::No,
            Action::PlayPause => self.toggle_pause(),
            Action::Back => self.skip(-self.input.controls.back_seconds),
            Action::Forward => self.skip(self.input.controls.forward_seconds),
            Action::PreviousMark => return Ran::when(self.jump_to_mark(Direction::Back)),
            Action::NextMark => return Ran::when(self.jump_to_mark(Direction::Forward)),
            Action::VolumeUp => self.step_volume(self.input.controls.volume_step),
            Action::VolumeDown => self.step_volume(-self.input.controls.volume_step),
            Action::Mute => {
                self.toggle_mute();
                self.toast(if self.status.muted {
                    "Muted"
                } else {
                    "Sound on"
                });
            }
            Action::NextSubtitle => self.cycle(TrackKind::Subtitle, cx),
            Action::NextAudio => self.cycle(TrackKind::Audio, cx),
            Action::TracksPanel => self.toggle_panel(Panel::Tracks),
            Action::Slower => self.step_speed(-1),
            Action::Faster => self.step_speed(1),
            Action::SubtitlesEarlier => self.step_subtitle_delay(-0.1, cx),
            Action::SubtitlesLater => self.step_subtitle_delay(0.1, cx),
            Action::SoundFilter => self.toggle_sound(cx),
            Action::Fullscreen => window.toggle_fullscreen(),
            Action::NextEpisode if has_episodes => return self.run_play_next(cx),
            Action::EpisodesPanel if has_episodes => self.toggle_panel(Panel::Episodes),
            Action::NextEpisode | Action::EpisodesPanel => return Ran::No,
            Action::Shortcuts => self.toggle_panel(Panel::Shortcuts),
            Action::Leave => {
                self.exit(cx);
                return Ran::Left;
            }
        }
        Ran::Yes
    }

    fn run_play_next(&mut self, cx: &mut Context<Self>) -> Ran {
        self.play_next(cx);
        Ran::Left
    }

    /// Changes the volume by `step` (its keys), with a note.
    fn step_volume(&mut self, step: f64) {
        self.set_volume(self.status.volume + step);
        self.toast(format!("Volume {:.0}%", self.status.volume));
    }
}
