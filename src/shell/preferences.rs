//! The settings kept in the library database: the player's keys, the sound
//! filter, the volume, each server's streaming quality, poster brightness.
//! Settings changes them; the shell reads them when a video starts.

use super::posters::{DIMMEST, Light};
use crate::jellyfin::Quality;
use crate::library::Db;
use crate::player::controls::Controls;
use crate::video::{SoundProfile, SoundSettings};

const CONTROLS: &str = "controls";
const VOLUME: &str = "volume";
/// Whether headphone and media-key next/previous switch episodes.
const MEDIA_SKIP_BUTTONS: &str = "media_skip_buttons";
/// Whether the player's sound filter is on. The key is night mode's, which
/// installs already have saved.
const SOUND_ON: &str = "night_mode";
/// Which sound filter is picked, and the custom tunings (`SoundSettings`, as JSON).
const SOUND_SETTINGS: &str = "night_mode_profiles";
const POSTER_REST: &str = "poster_brightness";
const POSTER_LIT: &str = "poster_hover_brightness";
/// Poster brightness at rest and lit, as fractions of the picture's own.
const DEFAULT_POSTER_REST: f32 = 0.8;
const DEFAULT_POSTER_LIT: f32 = 1.0;
/// The volume a first video starts at, in percent.
const DEFAULT_VOLUME: f64 = 100.0;
/// The quietest volume worth remembering: a video never starts silent.
const QUIETEST_REMEMBERED: f64 = 1.0;

/// The saved settings, read and written one at a time.
#[derive(Clone, Copy)]
pub(super) struct Preferences<'a> {
    db: &'a Db,
}

impl<'a> Preferences<'a> {
    pub(super) fn of(db: &'a Db) -> Self {
        Self { db }
    }

    /// The player's keys and how far they skip.
    pub(super) fn controls(self) -> Controls {
        match self.text(CONTROLS) {
            Some(json) => Controls::from_json(&json),
            None => Controls::default(),
        }
    }

    pub(super) fn set_controls(self, controls: &Controls) {
        if let Err(e) = self.db.set_setting(CONTROLS, &controls.to_json()) {
            eprintln!("[settings] saving the controls: {e:#}");
        }
    }

    /// The volume videos start at, in percent.
    pub(super) fn volume(self) -> f64 {
        self.text(VOLUME)
            .and_then(|volume| volume.parse().ok())
            .unwrap_or(DEFAULT_VOLUME)
    }

    /// Remembers the volume a video was left at, unless it was silent.
    pub(super) fn remember_volume(self, volume: f64) {
        if volume >= QUIETEST_REMEMBERED
            && let Err(e) = self.db.set_setting(VOLUME, &format!("{volume:.0}"))
        {
            eprintln!("[library] saving volume: {e}");
        }
    }

    /// The streaming quality chosen for a server (Original by default).
    pub(super) fn quality(self, server: i64) -> Quality {
        self.text(&quality_setting(server))
            .map_or(Quality::Original, |saved| Quality::from_setting(&saved))
    }

    pub(super) fn set_quality(self, server: i64, quality: Quality) {
        let saved = self
            .db
            .set_setting(&quality_setting(server), &quality.to_setting());
        if let Err(e) = saved {
            eprintln!("[library] saving quality: {e}");
        }
    }

    pub(super) fn media_skip_buttons(self) -> bool {
        self.flag(MEDIA_SKIP_BUTTONS).unwrap_or(true)
    }

    pub(super) fn set_media_skip_buttons(self, on: bool) {
        self.set_flag(MEDIA_SKIP_BUTTONS, on);
    }

    /// Whether videos start with the sound filter on, as the player left it.
    pub(super) fn sound_on(self) -> bool {
        self.flag(SOUND_ON).unwrap_or(false)
    }

    /// The player picked a sound filter or switched it off.
    pub(super) fn sound_chosen(self, on: bool, profile: SoundProfile) {
        self.set_flag(SOUND_ON, on);
        let mut settings = self.sound_settings();
        if settings.profile != profile {
            settings.profile = profile;
            self.set_sound_settings(settings);
        }
    }

    pub(super) fn sound_settings(self) -> SoundSettings {
        self.text(SOUND_SETTINGS)
            .map(|json| SoundSettings::from_json(&json))
            .unwrap_or_default()
    }

    pub(super) fn set_sound_settings(self, settings: SoundSettings) {
        if let Err(e) = self.db.set_setting(SOUND_SETTINGS, &settings.to_json()) {
            eprintln!("[settings] saving the sound filter: {e:#}");
        }
    }

    /// How bright posters are, at rest and under the mouse.
    pub(super) fn poster_light(self) -> Light {
        let read = |key: &str, default: f32| {
            self.text(key)
                .and_then(|percent| percent.parse::<u32>().ok())
                .map_or(default, |percent| percent as f32 / 100.0)
                .clamp(DIMMEST, 1.0)
        };
        Light {
            rest: read(POSTER_REST, DEFAULT_POSTER_REST),
            lit: read(POSTER_LIT, DEFAULT_POSTER_LIT),
        }
    }

    /// Saves the levels of `light` that differ from `previous` by a percent or more.
    pub(super) fn set_poster_light(self, light: Light, previous: Light) {
        let levels = [
            (POSTER_REST, light.rest, previous.rest),
            (POSTER_LIT, light.lit, previous.lit),
        ];
        for (key, level, previous) in levels {
            let percent = whole_percent(level);
            if percent != whole_percent(previous)
                && let Err(e) = self.db.set_setting(key, &percent.to_string())
            {
                eprintln!("[settings] saving {key}: {e:#}");
            }
        }
    }

    /// A setting's saved text; None when unset or unreadable.
    fn text(self, key: &str) -> Option<String> {
        self.db.setting(key).ok().flatten()
    }

    /// A yes/no setting; None when it was never set.
    fn flag(self, key: &str) -> Option<bool> {
        match self.text(key).as_deref() {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        }
    }

    fn set_flag(self, key: &str, on: bool) {
        if let Err(e) = self.db.set_setting(key, if on { "1" } else { "0" }) {
            eprintln!("[settings] saving {key}: {e:#}");
        }
    }
}

fn quality_setting(server: i64) -> String {
    format!("quality:{server}")
}

fn whole_percent(level: f32) -> u32 {
    (level * 100.0).round() as u32
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::*;

    #[test]
    fn nothing_saved_gives_the_defaults() -> Result<()> {
        let db = Db::open_in_memory()?;
        let preferences = Preferences::of(&db);
        assert_eq!(preferences.volume(), 100.0);
        assert_eq!(preferences.quality(1), Quality::Original);
        assert!(preferences.media_skip_buttons());
        assert!(!preferences.sound_on());
        assert_eq!(preferences.sound_settings(), SoundSettings::default());
        assert_eq!(
            preferences.controls().to_json(),
            Controls::default().to_json()
        );
        let light = preferences.poster_light();
        assert_eq!((light.rest, light.lit), (0.8, 1.0));
        Ok(())
    }

    #[test]
    fn a_silent_volume_is_not_remembered() -> Result<()> {
        let db = Db::open_in_memory()?;
        let preferences = Preferences::of(&db);
        preferences.remember_volume(42.4);
        assert_eq!(preferences.volume(), 42.0);
        preferences.remember_volume(0.0);
        assert_eq!(preferences.volume(), 42.0);
        Ok(())
    }

    #[test]
    fn each_server_keeps_its_own_quality() -> Result<()> {
        let db = Db::open_in_memory()?;
        let preferences = Preferences::of(&db);
        preferences.set_quality(2, Quality::Bitrate(8_000_000));
        assert_eq!(preferences.quality(2), Quality::Bitrate(8_000_000));
        assert_eq!(preferences.quality(3), Quality::Original);
        Ok(())
    }

    #[test]
    fn switches_stay_as_they_were_left() -> Result<()> {
        let db = Db::open_in_memory()?;
        let preferences = Preferences::of(&db);
        preferences.set_media_skip_buttons(false);
        assert!(!preferences.media_skip_buttons());
        preferences.set_media_skip_buttons(true);
        assert!(preferences.media_skip_buttons());
        Ok(())
    }

    #[test]
    fn the_player_s_sound_choice_keeps_the_custom_tunings() -> Result<()> {
        let db = Db::open_in_memory()?;
        let preferences = Preferences::of(&db);
        let mut settings = SoundSettings::default();
        settings.tune(|tuning| tuning.leveller = !tuning.leveller);
        preferences.set_sound_settings(settings);
        let other = SoundProfile::ALL
            .into_iter()
            .find(|profile| *profile != settings.profile)
            .expect("more than one sound profile");

        preferences.sound_chosen(true, other);

        assert!(preferences.sound_on());
        let saved = preferences.sound_settings();
        assert_eq!(saved.profile, other);
        let mut expected = settings;
        expected.profile = other;
        assert_eq!(saved, expected);
        Ok(())
    }

    #[test]
    fn poster_light_is_saved_in_whole_percents_and_kept_in_range() -> Result<()> {
        let db = Db::open_in_memory()?;
        let preferences = Preferences::of(&db);
        let previous = preferences.poster_light();
        let light = Light {
            rest: 0.604,
            lit: 1.0,
        };
        preferences.set_poster_light(light, previous);
        assert_eq!(db.setting(POSTER_REST)?.as_deref(), Some("60"));
        // The lit level didn't change, so it isn't written.
        assert_eq!(db.setting(POSTER_LIT)?, None);
        db.set_setting(POSTER_REST, "5")?;
        assert_eq!(preferences.poster_light().rest, DIMMEST);
        Ok(())
    }
}
