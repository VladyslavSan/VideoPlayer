//! The video's audio and subtitle tracks and the viewer's choices among them:
//! remembered languages, subtitle files, subtitle tuning. `tracks` has the
//! rules; the sound filter and the quality are in `sound`.

use std::time::{Duration, Instant};

use gpui::{Context, SharedString, Window};

use super::panels::Panel;
use super::segments::Chapter;
use super::sound::{QualityChoice, Sound};
use super::tracks::{self, Choice, SubtitlePlan};
use super::{Player, PlayerEvent, view};
use crate::jellyfin::Quality;
use crate::library::{SubtitlePref, SubtitleTuning, TrackPrefs};
use crate::video::{ExternalSubtitle, Track, TrackKind, VideoSurface};

/// How often the track list is re-read while the track panel is open.
const TRACKS_REFRESH: Duration = Duration::from_secs(1);
/// A subtitle position change smaller than this isn't sent to mpv, in percent.
const POSITION_STEP: f64 = 0.05;

/// Where subtitles sit, in percent of the window height, with the control
/// bar `raised` (0 to 1) in a window `height` pixels high.
fn subtitle_position(height: f32, raised: f32) -> f64 {
    if height > 0.0 {
        100.0 - (view::BAR_CLEARANCE / height * 100.0 * raised) as f64
    } else {
        100.0
    }
}

/// Tells the viewer subtitles start with a delay or size saved earlier.
pub(super) fn tuning_note(tuning: SubtitleTuning) -> String {
    let mut parts = Vec::new();
    if tuning.delay.abs() >= 0.05 {
        parts.push(format!("delay {}", view::format_delay(tuning.delay)));
    }
    if (tuning.scale - 1.0).abs() >= 0.05 {
        parts.push(format!("size {:.0}%", tuning.scale * 100.0));
    }
    format!("Subtitle {}, as last time", parts.join(", "))
}

/// The tracks mpv reports and what was chosen among them.
pub(super) struct Tracks {
    list: Vec<Track>,
    read_at: Instant,
    /// Remembered track choices, applied once the tracks are known.
    prefs: TrackPrefs,
    /// The remembered audio is applied, or the viewer picked one: left alone now.
    audio_settled: bool,
    subtitle_settled: bool,
    /// Subtitle files to add once the video has loaded.
    pending_subtitles: Vec<ExternalSubtitle>,
    /// Subtitle files added to the stream so far, to add again if it's reopened.
    added_subtitles: Vec<ExternalSubtitle>,
    /// Subtitle position last sent to mpv, in percent of the window height.
    subtitle_position: f64,
    /// Said once the video shows: subtitles start with a saved delay or size.
    tuning_note: Option<SharedString>,
    pub(super) sound: Sound,
    pub(super) quality: QualityChoice,
}

impl Tracks {
    pub(super) fn new(
        prefs: TrackPrefs,
        quality: Option<Quality>,
        tuning_note: Option<SharedString>,
    ) -> Self {
        Self {
            list: Vec::new(),
            read_at: Instant::now(),
            prefs,
            audio_settled: false,
            subtitle_settled: false,
            pending_subtitles: Vec::new(),
            added_subtitles: Vec::new(),
            subtitle_position: 100.0,
            tuning_note,
            sound: Sound::new(),
            quality: QualityChoice::new(quality),
        }
    }

    pub(super) fn of(&self, kind: TrackKind) -> impl Iterator<Item = &Track> {
        self.list.iter().filter(move |t| t.kind == kind)
    }

    pub(super) fn read(&mut self, video: &VideoSurface) {
        self.list = video.tracks();
        self.read_at = Instant::now();
    }

    /// Subtitle files that live next to the video, added once it has loaded.
    pub(super) fn queue_subtitles(&mut self, subtitles: Vec<ExternalSubtitle>) {
        self.pending_subtitles.extend(subtitles);
    }

    /// Another stream of the video is opening, with its own subtitle files.
    pub(super) fn reloaded(&mut self, subtitles: Vec<ExternalSubtitle>) {
        self.added_subtitles.clear();
        self.pending_subtitles = subtitles;
        self.forget_tracks();
    }

    /// The same stream is opening again: its subtitle files are added again.
    pub(super) fn reopened(&mut self) {
        self.pending_subtitles = std::mem::take(&mut self.added_subtitles);
        self.forget_tracks();
    }

    /// The tracks are another file's now: choices apply again once it loads.
    fn forget_tracks(&mut self) {
        self.list.clear();
        self.audio_settled = false;
        self.subtitle_settled = false;
    }

    /// Adds the waiting subtitle files to the video. True if there were any.
    fn add_pending_subtitles(&mut self, video: &VideoSurface) -> bool {
        if self.pending_subtitles.is_empty() {
            return false;
        }
        for subtitle in std::mem::take(&mut self.pending_subtitles) {
            if let Err(e) = video.add_subtitle(&subtitle) {
                eprintln!("[video] {e:#}");
            }
            self.added_subtitles.push(subtitle);
        }
        true
    }

    /// Whether the list should be read again: never read, or the panel
    /// showing it is open and it's a moment old.
    fn is_stale(&self, panel_open: bool) -> bool {
        self.list.is_empty() || (panel_open && self.read_at.elapsed() >= TRACKS_REFRESH)
    }

    /// Selects the remembered languages, if the file has them. A subtitle
    /// language that isn't there yet may still arrive as an external file.
    fn apply_prefs(&mut self, video: &VideoSurface) {
        let mut changed = false;
        if !self.audio_settled {
            if let Some(id) = tracks::audio_to_select(&self.list, self.prefs.audio.as_deref()) {
                video.select_audio(id);
                changed = true;
            }
            self.audio_settled = true;
        }
        if !self.subtitle_settled {
            let plan = tracks::subtitle_plan(&self.list, self.prefs.subtitle.as_ref());
            match plan {
                SubtitlePlan::Leave | SubtitlePlan::Wait => {}
                SubtitlePlan::Off => video.select_subtitle(None),
                SubtitlePlan::Select(id) => video.select_subtitle(Some(id)),
            }
            self.subtitle_settled = plan != SubtitlePlan::Wait;
            changed |= matches!(plan, SubtitlePlan::Off | SubtitlePlan::Select(_));
        }
        if changed {
            self.read(video);
        }
    }

    /// The viewer's audio pick. Returns its language, to remember.
    fn pick_audio(&mut self, video: &VideoSurface, id: i64) -> Option<String> {
        video.select_audio(id);
        self.audio_settled = true;
        self.read(video);
        let language = self.language_of(id)?;
        self.prefs.audio = Some(language.clone());
        Some(language)
    }

    /// The viewer's subtitle pick (`None`: off). Returns what to remember.
    fn pick_subtitle(&mut self, video: &VideoSurface, id: Option<i64>) -> Option<SubtitlePref> {
        video.select_subtitle(id);
        self.subtitle_settled = true;
        self.read(video);
        let pref = match id {
            None => SubtitlePref::Off,
            Some(id) => SubtitlePref::Language(self.language_of(id)?),
        };
        self.prefs.subtitle = Some(pref.clone());
        Some(pref)
    }

    fn language_of(&self, id: i64) -> Option<String> {
        self.list.iter().find(|t| t.id == id)?.lang.clone()
    }

    /// The note about saved subtitle tuning, the first time it's asked for.
    pub(super) fn take_tuning_note(&mut self) -> Option<SharedString> {
        self.tuning_note.take()
    }

    /// Keeps subtitles above the control bar, `raised` (0 to 1) as it fades
    /// in and out, so they glide rather than jump.
    pub(super) fn place_subtitles(&mut self, video: &VideoSurface, height: f32, raised: f32) {
        let target = subtitle_position(height, raised);
        if (target - self.subtitle_position).abs() >= POSITION_STEP {
            video.set_subtitle_position(target);
            self.subtitle_position = target;
        }
    }
}

impl Player {
    /// The source has loaded: adds its subtitle files, reads its tracks and
    /// chapters, and applies the remembered choices.
    pub(super) fn source_loaded(&mut self) {
        let added = self.tracks.add_pending_subtitles(&self.video);
        if added || self.tracks.is_stale(self.panels.is_open(Panel::Tracks)) {
            self.tracks.read(&self.video);
        }
        if self.markers.wants_chapters(self.status.duration) {
            let chapters = Chapter::list(self.video.chapters());
            self.markers.set_chapters(chapters, self.status.duration);
        }
        // Once when the file opens, and again when subtitle files arrive.
        if added || !self.tracks.audio_settled {
            self.tracks.apply_prefs(&self.video);
        }
    }

    /// The viewer's pick; remembered for next time.
    pub(super) fn select_audio(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(language) = self.tracks.pick_audio(&self.video, id) {
            cx.emit(PlayerEvent::AudioChosen(language));
        }
    }

    /// The viewer's pick (`None` = off); remembered for next time.
    pub(super) fn select_subtitle(&mut self, id: Option<i64>, cx: &mut Context<Self>) {
        if let Some(pref) = self.tracks.pick_subtitle(&self.video, id) {
            cx.emit(PlayerEvent::SubtitleChosen(pref));
        }
    }

    /// Steps to the next track of `kind`; subtitles include "off".
    pub(super) fn cycle(&mut self, kind: TrackKind, cx: &mut Context<Self>) {
        self.tracks.read(&self.video);
        let Some(choice) = tracks::next_choice(&self.tracks.list, kind) else {
            self.toast(match kind {
                TrackKind::Audio => "No audio tracks",
                TrackKind::Subtitle => "No subtitles",
            });
            return;
        };
        match (kind, choice) {
            (TrackKind::Audio, Choice::Track(id)) => self.select_audio(id, cx),
            (TrackKind::Audio, Choice::Off) => {}
            (TrackKind::Subtitle, Choice::Track(id)) => self.select_subtitle(Some(id), cx),
            (TrackKind::Subtitle, Choice::Off) => self.select_subtitle(None, cx),
        }
        let name = tracks::choice_name(&self.tracks.list, kind, choice);
        self.toast(match kind {
            TrackKind::Audio => format!("Audio: {name}"),
            TrackKind::Subtitle => format!("Subtitles: {name}"),
        });
    }

    pub(super) fn step_subtitle_scale(&mut self, delta: f64, cx: &mut Context<Self>) {
        let scale = ((self.status.subtitle_scale + delta) * 10.0).round() / 10.0;
        let scale = scale.clamp(0.5, 2.5);
        self.video.set_subtitle_scale(scale);
        self.status.subtitle_scale = scale;
        self.tuned(cx);
    }

    pub(super) fn step_subtitle_delay(&mut self, delta: f64, cx: &mut Context<Self>) {
        let delay = ((self.status.subtitle_delay + delta) * 10.0).round() / 10.0;
        self.video.set_subtitle_delay(delay);
        self.status.subtitle_delay = delay;
        self.toast(format!("Subtitle delay {}", view::format_delay(delay)));
        self.tuned(cx);
    }

    /// The delay or size changed: remembered for the show's next episodes.
    fn tuned(&mut self, cx: &mut Context<Self>) {
        // A change made before the video shows makes the note out of date.
        self.tracks.tuning_note = None;
        cx.emit(PlayerEvent::SubtitleTuned(SubtitleTuning {
            delay: self.status.subtitle_delay,
            scale: self.status.subtitle_scale,
        }));
    }

    /// Keeps subtitles above the control bar while it's showing.
    pub(super) fn place_subtitles(&mut self, window: &Window, raised: f32) {
        let height = f32::from(window.viewport_size().height);
        self.tracks.place_subtitles(&self.video, height, raised);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitles_rise_with_the_control_bar() {
        assert_eq!(subtitle_position(920.0, 0.0), 100.0);
        let raised = subtitle_position(920.0, 1.0);
        assert!((raised - 90.0).abs() < 1e-4, "{raised}");
        let halfway = subtitle_position(920.0, 0.5);
        assert!((halfway - 95.0).abs() < 1e-4, "{halfway}");
    }

    #[test]
    fn subtitles_stay_put_in_a_window_with_no_height() {
        assert_eq!(subtitle_position(0.0, 1.0), 100.0);
    }

    #[test]
    fn the_tuning_note_names_what_was_saved() {
        let note = |delay, scale| tuning_note(SubtitleTuning { delay, scale });
        assert_eq!(note(0.5, 1.0), "Subtitle delay +0.5 s, as last time");
        assert_eq!(note(0.0, 1.2), "Subtitle size 120%, as last time");
        assert_eq!(
            note(-0.3, 0.8),
            "Subtitle delay -0.3 s, size 80%, as last time"
        );
    }

    #[test]
    fn a_reopened_stream_gets_its_subtitle_files_again() {
        let mut tracks = Tracks::new(TrackPrefs::default(), None, None);
        let file = ExternalSubtitle {
            url: "http://server/sub.srt".into(),
            title: None,
            lang: Some("eng".into()),
        };
        tracks.added_subtitles.push(file.clone());
        tracks.audio_settled = true;
        tracks.reopened();
        assert_eq!(tracks.pending_subtitles, [file]);
        assert!(tracks.added_subtitles.is_empty());
        assert!(!tracks.audio_settled);
    }

    #[test]
    fn another_quality_brings_its_own_subtitle_files() {
        let mut tracks = Tracks::new(TrackPrefs::default(), None, None);
        let file = |name: &str| ExternalSubtitle {
            url: name.into(),
            title: None,
            lang: None,
        };
        tracks.added_subtitles.push(file("old"));
        tracks.reloaded(vec![file("new")]);
        assert_eq!(tracks.pending_subtitles, [file("new")]);
        assert!(tracks.added_subtitles.is_empty());
    }
}
