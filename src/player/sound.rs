//! The options picked from menus in the tracks panel: the sound filter
//! (`video/sound.rs`: night, dialogue, leveller…) and the streaming quality.

use anyhow::Result;
use gpui::Context;

use super::{Player, PlayerEvent};
use crate::jellyfin::Quality;
use crate::ui::menu;
use crate::video::{Downmix, SoundProfile, SoundSettings, VideoSurface};

/// The streaming quality of a Jellyfin video, and its menu.
pub(super) struct QualityChoice {
    /// None hides the setting (not a Jellyfin video).
    pub(super) current: Option<Quality>,
    /// Opened with the quality in use, which stays checked as it fades out.
    pub(super) menu: menu::Shown<Quality>,
}

impl QualityChoice {
    pub(super) fn new(current: Option<Quality>) -> Self {
        Self {
            current,
            menu: menu::Shown::default(),
        }
    }

    /// Opens the menu with `current` checked, or closes it if it's open.
    pub(super) fn toggle_menu(&mut self, current: Quality) {
        if !self.menu.close() {
            self.menu.open(current);
        }
    }
}

/// The sound filter: on or off, the one picked, and its menu.
pub(super) struct Sound {
    on: bool,
    /// The filter picked (kept while it's off), with Settings' custom tunings.
    settings: SoundSettings,
    /// The audio channels the filter was built for (it may fold surround to stereo).
    channels: Option<u32>,
    /// Opened with the choice in use (None: off).
    pub(super) menu: menu::Shown<Option<SoundProfile>>,
}

impl Sound {
    pub(super) fn new() -> Self {
        Self {
            on: false,
            settings: SoundSettings::default(),
            channels: None,
            menu: menu::Shown::default(),
        }
    }

    /// Opens the menu with `current` checked, or closes it if it's open.
    pub(super) fn toggle_menu(&mut self, current: Option<SoundProfile>) {
        if !self.menu.close() {
            self.menu.open(current);
        }
    }

    /// The filter in use: None when it's off.
    pub(super) fn choice(&self) -> Option<SoundProfile> {
        self.on.then_some(self.settings.profile)
    }

    /// What its key switches to: off, or back on as last picked.
    fn toggled(&self) -> Option<SoundProfile> {
        (!self.on).then_some(self.settings.profile)
    }

    /// The filter as saved, at the start.
    fn start(&mut self, video: &VideoSurface, on: bool, settings: SoundSettings) {
        self.settings = settings;
        if !on {
            return;
        }
        self.channels = video.audio_channels();
        match video.set_sound_filter(true, &self.settings.tuning(), self.channels) {
            Ok(()) => self.on = true,
            Err(e) => eprintln!("[player] {e:#}"),
        }
    }

    /// Builds the filter again when the audio's channels change (the file
    /// opened, another track picked): some tunings fold surround to stereo.
    fn follow_channels(&mut self, video: &VideoSurface) {
        let tuning = self.settings.tuning();
        if !self.on || tuning.downmix == Downmix::Off {
            return;
        }
        let channels = video.audio_channels();
        if channels.is_none() || channels == self.channels {
            return;
        }
        self.channels = channels;
        if let Err(e) = video.set_sound_filter(true, &tuning, channels) {
            eprintln!("[player] {e:#}");
        }
    }

    /// Switches the filter to `choice` (None: off); nothing changes if mpv refuses.
    fn switch_to(&mut self, video: &VideoSurface, choice: Option<SoundProfile>) -> Result<()> {
        let mut settings = self.settings;
        if let Some(profile) = choice {
            settings.profile = profile;
        }
        self.channels = video.audio_channels();
        video.set_sound_filter(choice.is_some(), &settings.tuning(), self.channels)?;
        self.settings = settings;
        self.on = choice.is_some();
        Ok(())
    }
}

impl Player {
    /// The sound filter as saved, at the start (no note: it's how it was).
    pub fn start_sound(&mut self, on: bool, sound: SoundSettings) {
        self.tracks.sound.start(&self.video, on, sound);
    }

    pub(super) fn follow_audio_channels(&mut self) {
        self.tracks.sound.follow_channels(&self.video);
    }

    /// Switches to `choice` (None: off), with a note, and has it remembered.
    pub(super) fn choose_sound(&mut self, choice: Option<SoundProfile>, cx: &mut Context<Self>) {
        self.tracks.sound.menu.close();
        if choice == self.tracks.sound.choice() {
            return;
        }
        if let Err(e) = self.tracks.sound.switch_to(&self.video, choice) {
            eprintln!("[player] {e:#}");
            self.toast("The sound filter isn't available here");
            return;
        }
        self.toast(match choice {
            Some(profile) => format!("Sound filter: {}", profile.label()),
            None => "Sound filter off".to_string(),
        });
        cx.emit(PlayerEvent::SoundChosen {
            on: self.tracks.sound.on,
            profile: self.tracks.sound.settings.profile,
        });
    }

    /// The sound filter off, or back on as last picked (its key).
    pub(super) fn toggle_sound(&mut self, cx: &mut Context<Self>) {
        self.choose_sound(self.tracks.sound.toggled(), cx);
    }

    /// Switches to `quality` (from the quality menu).
    pub(super) fn choose_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        self.tracks.quality.menu.close();
        if self.tracks.quality.current != Some(quality) {
            cx.emit(PlayerEvent::QualityChosen(quality));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_turns_the_filter_off_or_back_on_as_last_picked() {
        let mut sound = Sound::new();
        let picked = sound.settings.profile;
        assert_eq!(sound.choice(), None);
        assert_eq!(sound.toggled(), Some(picked));
        sound.on = true;
        assert_eq!(sound.choice(), Some(picked));
        assert_eq!(sound.toggled(), None);
    }
}
