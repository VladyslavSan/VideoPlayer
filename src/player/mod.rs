//! Playback screen: the mpv video layer plus GPUI controls drawn above it.
//!
//! `Player` holds the video and one part per concern (tracks, markers, the
//! countdown, sliders, messages, fades, activity, the stream, reporting);
//! each part's module also has the player's methods about it.

mod actions;
mod activity;
pub mod controls;
mod fade;
mod keys;
mod markers;
mod messages;
mod mouse;
mod panels;
mod playing;
mod previews;
mod reporting;
mod seek_bar;
mod segments;
mod sound;
mod status;
mod stream;
mod track_choice;
mod tracks;
mod up_next;
mod view;

use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use gpui::{Context, EventEmitter, SharedString, Subscription, Task, Window};

use crate::jellyfin::Quality;
use crate::library::{SubtitlePref, SubtitleTuning, TrackPrefs, WATCHED_FRACTION};
use crate::media_session::{MediaSession, NowPlaying};
use crate::video::{ExternalSubtitle, Segment, SoundProfile, VideoSurface};
use activity::Activity;
use fade::Fades;
use keys::Input;
use markers::Markers;
use messages::Messages;
use panels::Panels;
use playing::Playing;
use reporting::Reporting;
use seek_bar::Sliders;
use status::Status;
use stream::Stream;
use track_choice::Tracks;
use up_next::UpNext;

pub use markers::SegmentSource;
pub use playing::Episode;
pub use previews::Previews;
pub use segments::merge as merge_segments;

/// How often mpv's state is read and the player's rules are run.
const TICK: Duration = Duration::from_millis(100);

pub enum PlayerEvent {
    /// Position report: every few seconds while playing, at each pause or resume, at the end.
    Progress {
        position: f64,
        duration: f64,
        paused: bool,
    },
    /// The user left the player, or the video ended.
    Exit,
    /// Switch to another item (an episode of the same show), by library key.
    Play(String),
    /// The viewer picked an audio track in this language, to remember.
    AudioChosen(String),
    /// The viewer picked subtitles (or turned them off), to remember.
    SubtitleChosen(SubtitlePref),
    /// The viewer changed the subtitle delay or size, to remember.
    SubtitleTuned(SubtitleTuning),
    /// The viewer picked a streaming quality (Jellyfin only).
    QualityChosen(Quality),
    /// The viewer picked a sound filter or turned it off, to remember.
    SoundChosen {
        on: bool,
        /// The filter picked (kept while it's off).
        profile: SoundProfile,
    },
}

/// What is being played, as shown in the player.
pub struct Playback {
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
    /// The show's episodes in watching order (empty for movies).
    pub episodes: Vec<Episode>,
    /// Index of the playing episode in `episodes`.
    pub current: Option<usize>,
    /// Remembered track choices to apply once the tracks are known.
    pub prefs: TrackPrefs,
    /// Subtitle delay and size saved for this show or movie.
    pub tuning: SubtitleTuning,
    /// Streaming quality, for Jellyfin videos (None hides the setting).
    pub quality: Option<Quality>,
    /// Artwork for the system's now-playing panel.
    pub thumb: Option<std::path::PathBuf>,
}

/// What runs for as long as the player lives.
struct KeptAlive {
    _poll: Task<()>,
    _window_bounds: Subscription,
}

pub struct Player {
    video: VideoSurface,
    input: Input,
    playing: Playing,
    /// What mpv said at the last tick.
    status: Status,
    panels: Panels,
    tracks: Tracks,
    markers: Markers,
    up_next: UpNext,
    sliders: Sliders,
    messages: Messages,
    /// Everything that fades in and out (see `view`'s `render`).
    fades: Fades,
    activity: Activity,
    stream: Stream,
    reporting: Reporting,
    _kept_alive: KeptAlive,
}

impl EventEmitter<PlayerEvent> for Player {}

/// Creates the native video layer for `window`. Kept separate from
/// `Player::new` so failures surface before the view exists.
pub fn open_video(
    window: &Window,
    source: &str,
    start: f64,
    http_header: Option<&str>,
) -> Result<VideoSurface> {
    VideoSurface::new(window, source, start, http_header)
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

impl Player {
    pub fn new(
        video: VideoSurface,
        playback: Playback,
        volume: f64,
        media: Option<Rc<MediaSession>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tuning = playback.tuning;
        let playing = Playing {
            title: playback.title,
            subtitle: playback.subtitle,
            episodes: playback.episodes,
            current: playback.current,
        };
        if let Some(media) = &media {
            media.show(NowPlaying {
                title: &playing.title,
                subtitle: playing.subtitle.as_ref().map(|s| s.as_ref()),
                thumb: playback.thumb.as_deref(),
                has_previous: playing.current.is_some_and(|i| i > 0),
                has_next: playing.next().is_some(),
            });
        }
        let focus = cx.focus_handle();
        focus.focus(window);
        video.sync_size();
        video.set_volume(volume);
        let tuning_note = (!tuning.is_default()).then(|| {
            video.set_subtitle_delay(tuning.delay);
            video.set_subtitle_scale(tuning.scale);
            track_choice::tuning_note(tuning).into()
        });

        let window_bounds = cx.observe_window_bounds(window, |this, _, _| this.video.sync_size());
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });

        Self {
            video,
            input: Input {
                focus,
                controls: controls::Controls::default(),
            },
            status: Status::starting(volume, tuning),
            panels: Panels::new(playing.current_season().unwrap_or(0)),
            playing,
            tracks: Tracks::new(playback.prefs, playback.quality, tuning_note),
            markers: Markers::new(),
            up_next: UpNext::new(),
            sliders: Sliders::new(),
            messages: Messages::new(),
            fades: Fades::new(),
            activity: Activity::new(),
            stream: Stream::new(),
            reporting: Reporting::new(media),
            _kept_alive: KeptAlive {
                _poll: poll,
                _window_bounds: window_bounds,
            },
        }
    }

    /// Intro, recap and credits markers from `source`. For each kind, the
    /// best source that has it is used.
    pub fn add_segments(&mut self, source: SegmentSource, segments: Vec<Segment>) {
        self.markers.add(source, segments);
    }

    pub fn set_controls(&mut self, controls: controls::Controls) {
        self.input.controls = controls;
    }

    /// Subtitle files that live next to the video (for example on a Jellyfin
    /// server). They're added as soon as the video has loaded.
    pub fn add_external_subtitles(&mut self, subtitles: Vec<ExternalSubtitle>) {
        self.tracks.queue_subtitles(subtitles);
    }

    pub fn volume(&self) -> f64 {
        self.status.volume
    }

    pub fn position(&self) -> f64 {
        self.status.position
    }

    pub fn duration(&self) -> f64 {
        self.status.duration
    }

    /// Whether the video counts as watched: it ended, it's nearly over, or
    /// the end credits have started.
    pub fn finished(&self) -> bool {
        let (position, duration) = (self.status.position, self.status.duration);
        duration > 0.0
            && (self.status.ended
                || position >= duration * WATCHED_FRACTION
                || self.markers.end_credits_started(position, duration))
    }

    /// Saves the video as shown to `path` (tests).
    pub fn screenshot(&self, path: &std::path::Path) {
        if let Err(e) = self.video.screenshot(path) {
            eprintln!("[test] {e:#}");
        }
    }

    /// Shows pictures from `previews` when hovering the seek bar.
    pub fn set_previews(&mut self, previews: Previews) {
        self.sliders.seek.previews = Some(previews);
    }

    /// Reads mpv's state and runs the player's rules on it, every `TICK`.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.reporting.has_left() {
            return;
        }
        self.follow_audio_channels();
        self.status = Status::read(&self.video);
        if self.handle_media_commands(cx) {
            return;
        }
        if self.check_stall(cx) {
            cx.notify();
            return;
        }
        self.reporting.report_media(&self.status);
        self.stream.log_pipeline(&self.video);
        self.follow_source();
        if self.video.ended() {
            return self.follow_end(cx);
        }
        if self.follow_credits(cx) {
            return;
        }
        if self.reporting.progress_due(self.reporting_paused()) {
            self.report(cx);
        }
        cx.notify();
    }

    fn poke(&mut self, cx: &mut Context<Self>) {
        self.activity.poke();
        cx.notify();
    }

    fn toast(&mut self, message: impl Into<SharedString>) {
        self.messages.toast(message.into());
    }

    fn controls_visible(&self) -> bool {
        self.status.paused
            || self.panels.open().is_some()
            || self.up_next.is_counting()
            || self.sliders.is_dragging()
            || self.activity.keeps_controls()
    }
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn times_show_hours_only_when_there_are_some() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(75.9), "1:15");
        assert_eq!(format_time(3725.0), "1:02:05");
        assert_eq!(format_time(-4.0), "0:00");
    }
}
