//! Playback screen: the mpv video layer plus GPUI controls drawn above it.

pub mod controls;
mod fade;
mod previews;
mod segments;
mod tracks;
mod view;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui::{
    Bounds, Context, EventEmitter, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Subscription, Task, Window,
};

use crate::jellyfin::Quality;
use crate::library::{SubtitlePref, SubtitleTuning, TrackPrefs, WATCHED_FRACTION};
use crate::media_session::{self, MediaCommand, MediaSession, NowPlaying};
use crate::motion::Fade;
use crate::ui::menu;
use crate::video::{ExternalSubtitle, Segment, SegmentKind, Track, TrackKind, VideoSurface};
use fade::{Notes, Swaps};

pub use previews::Previews;
pub use segments::merge as merge_segments;

/// Where intro/credits markers came from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentSource {
    /// The Jellyfin server's media segments.
    Server,
    /// TheIntroDB / IntroDB.
    Online,
    /// Chapter names in the file.
    Chapters,
}

const TICK: Duration = Duration::from_millis(100);
const CONTROLS_TIMEOUT: Duration = Duration::from_millis(2500);
/// While the controls are hidden, the mouse has to travel this far within
/// `REVEAL_WINDOW` to bring them back, so small nudges don't.
const REVEAL_DISTANCE: f32 = 40.0;
const REVEAL_WINDOW: Duration = Duration::from_millis(500);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
/// A network stream that "ends" this far before its end broke off: the
/// connection went and what was buffered ran out.
const BROKE_OFF_BEFORE_END: f64 = 20.0;
/// Waits between attempts to open a broken-off stream again.
const REOPEN_WAITS: [u64; 5] = [2, 3, 5, 8, 10];
/// How long a note like "Volume 80%" stays before it fades out.
const TOAST_DURATION: Duration = Duration::from_millis(1200);
/// How long mpv's video output must be up before the black backdrop is
/// lifted, so its first frame is on screen first.
const VIDEO_SETTLE: Duration = Duration::from_millis(150);
/// How often the track list is re-read while the track panel is open.
const TRACKS_REFRESH: Duration = Duration::from_secs(1);
pub const SPEEDS: [f64; 7] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

/// Seconds the "Up next" card counts down before playing the next episode.
const UP_NEXT_SECONDS: u64 = 8;
/// Credits that start after this fraction of the video are its end credits.
const END_CREDITS_FROM: f64 = 0.7;

pub enum PlayerEvent {
    /// Position report, for resume: every few seconds while playing, once
    /// when paused (a paused player has nothing new to say, and repeating
    /// itself would overwrite what other devices do meanwhile), once when
    /// playing again, and at the end.
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
}

/// An episode of the show being played, for the episode list and "next".
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    /// Library key, sent back in `PlayerEvent::Play`.
    pub key: String,
    pub season: u16,
    pub number: Option<u16>,
    pub title: String,
    pub watched: bool,
    pub progress: Option<f32>,
    pub thumb: Option<std::path::PathBuf>,
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

/// The panel open above the control bar, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Tracks,
    Episodes,
    Shortcuts,
}

impl Panel {
    const ALL: [Panel; 3] = [Panel::Tracks, Panel::Episodes, Panel::Shortcuts];

    fn index(self) -> usize {
        self as usize
    }
}

/// The player's overlays, each fading in and out.
struct Fades {
    /// The control bar and the title.
    controls: Fade,
    /// Each panel, by `Panel::index`.
    panels: [Fade; 3],
    up_next: Fade,
    skip: Fade,
    /// The seek bar's time preview (and the bar thickening under it).
    preview: Fade,
    /// The speed chip in the control bar, when not at normal speed.
    speed: Fade,
    /// The video, as the black backdrop lifts off it.
    video: Fade,
}

impl Fades {
    fn new() -> Self {
        Self {
            controls: Fade::new(true),
            panels: [Fade::new(false); 3],
            up_next: Fade::new(false),
            skip: Fade::new(false),
            preview: Fade::new(false),
            speed: Fade::new(false),
            video: Fade::new(false),
        }
    }

    fn moving(&self) -> bool {
        [
            &self.controls,
            &self.up_next,
            &self.skip,
            &self.preview,
            &self.speed,
            &self.video,
        ]
        .into_iter()
        .chain(&self.panels)
        .any(Fade::moving)
    }
}

/// What a held mouse button is dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Seek,
    Volume,
}

/// What became of a key's action.
enum Ran {
    /// Didn't apply now: try the key's next action.
    No,
    Yes,
    /// The player is leaving (for another episode, or back).
    Left,
}

/// A network stream that broke off.
struct Stall {
    /// Where it stopped; it carries on from here.
    at: f64,
    since: Instant,
    next_try: Instant,
    tries: usize,
    /// Play once it's back (false if the viewer paused meanwhile).
    play: bool,
}

/// Values mpv owns, re-read every tick so the controls never drift from it.
#[derive(Debug, Clone, Copy)]
struct Status {
    position: f64,
    duration: f64,
    buffered: Option<f64>,
    paused: bool,
    buffering: bool,
    ended: bool,
    /// The video is on screen. Until then the player paints black: the
    /// window has no backing of its own, so unpainted areas would show the
    /// desktop behind it.
    video_shown: bool,
    volume: f64,
    muted: bool,
    speed: f64,
    subtitle_scale: f64,
    subtitle_delay: f64,
}

pub struct Player {
    video: VideoSurface,
    focus: FocusHandle,
    title: SharedString,
    subtitle: Option<SharedString>,
    episodes: Vec<Episode>,
    current: Option<usize>,
    /// Season shown in the episodes panel.
    episode_season: u16,
    /// When the video has ended and the next episode will start.
    up_next: Option<Instant>,
    prefs: TrackPrefs,
    quality: Option<Quality>,
    /// The streaming quality menu (in the tracks panel), with the quality
    /// when it opened (the one checked, even as it fades out after a pick).
    quality_menu: menu::Shown<Quality>,
    /// When mpv's video output came up for the current source.
    video_ready_since: Option<Instant>,
    /// After `reload`, the new source; until mpv reports it, track and
    /// load state still describe the old one.
    expected_path: Option<String>,
    /// Set once the remembered choice is applied, or the viewer picks one;
    /// after that, tracks are left alone.
    audio_settled: bool,
    subtitle_settled: bool,
    /// Intros, recaps and credits, combined from `segment_sources`.
    segments: Vec<Segment>,
    /// What each `SegmentSource` found, best first.
    segment_sources: [Vec<Segment>; 3],
    chapters_read: bool,
    /// Chapter titles and start times, for the seek bar.
    chapters: Vec<(String, f64)>,
    /// "Watch credits" was chosen: no countdown until the video ends.
    credits_dismissed: bool,
    status: Status,
    tracks: Vec<Track>,
    tracks_read: Instant,
    pending_subtitles: Vec<ExternalSubtitle>,
    /// Subtitle files added to the stream so far, to add again if it has
    /// to be opened again.
    added_subtitles: Vec<ExternalSubtitle>,
    /// Where it last played fine.
    last_good: Option<f64>,
    /// The viewer's keys and skip lengths.
    controls: controls::Controls,
    opened: Instant,
    /// The stream broke off; trying to open it again.
    stall: Option<Stall>,
    panel: Option<Panel>,
    drag: Option<Drag>,
    /// Where the seek bar is being dragged to, shown instead of the position.
    scrub: Option<f64>,
    /// Mouse x over the seek bar, for the time preview.
    seek_hover: Option<Pixels>,
    bar_hovered: bool,
    /// Where (and when) the mouse started moving while the controls were
    /// hidden; they come back once it has moved far enough from there.
    reveal_from: Option<(Point<Pixels>, Instant)>,
    /// Notes like "Volume 80%", and when the current one was shown.
    toasts: Notes,
    toast_at: Option<Instant>,
    /// "Loading…" or "Reconnecting…" in the middle.
    notice: Notes,
    /// Since when it's been loading ("Loading…" waits a moment).
    loading_since: Option<Instant>,
    /// Everything that fades in and out (see `view`'s `render`).
    fades: Fades,
    /// The up-next card's countdown and whether the video ended, as last
    /// shown (drawn so while it fades out).
    up_next_shown: (u64, bool),
    /// The skip button's label, as last shown.
    skip_shown: &'static str,
    /// Where the seek bar's time preview was last shown.
    preview_x: Pixels,
    /// The episodes panel's season before the last switch, and when.
    season_switch: Option<(u16, Instant)>,
    /// Icons and values that dissolve when they change.
    swaps: Swaps,
    /// The last speed other than normal, shown while its chip fades out.
    speed_shown: f64,
    /// Said once the video shows: subtitles start with a saved delay or size.
    tuning_note: Option<SharedString>,
    /// Subtitle position last sent to mpv, in percent of the window height.
    subtitle_position: f64,
    exited: bool,
    last_activity: Instant,
    last_report: Instant,
    /// Whether the last report said paused.
    reported_paused: bool,
    pipeline: String,
    /// Pictures for the seek bar's hover preview, when available.
    previews: Option<Previews>,
    // Laid-out bounds of the sliders, written during paint, read by mouse handlers.
    seek_bounds: Rc<Cell<Bounds<Pixels>>>,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The system's media controls (headphone buttons, media keys).
    media: Option<Rc<MediaSession>>,
    /// What was last reported to them: playing, position, and when.
    media_report: Option<(bool, f64, Instant)>,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
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

/// Fraction of `bounds`' width at horizontal position `x`, clamped to 0..=1.
/// Tells the viewer subtitles start with a delay or size saved earlier.
fn tuning_note(tuning: SubtitleTuning) -> String {
    let mut parts = Vec::new();
    if tuning.delay.abs() >= 0.05 {
        parts.push(format!("delay {}", view::format_delay(tuning.delay)));
    }
    if (tuning.scale - 1.0).abs() >= 0.05 {
        parts.push(format!("size {:.0}%", tuning.scale * 100.0));
    }
    format!("Subtitle {}, as last time", parts.join(", "))
}

fn fraction_at(bounds: Bounds<Pixels>, x: Pixels) -> f64 {
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return 0.0;
    }
    (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0) as f64
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
        let Playback {
            title,
            subtitle,
            episodes,
            current,
            prefs,
            tuning,
            quality,
            thumb,
        } = playback;
        if let Some(media) = &media {
            media.show(NowPlaying {
                title: &title,
                subtitle: subtitle.as_ref().map(|s| s.as_ref()),
                thumb: thumb.as_deref(),
                has_previous: current.is_some_and(|i| i > 0),
                has_next: current.is_some_and(|i| i + 1 < episodes.len()),
            });
        }
        let episode_season = current
            .and_then(|i| episodes.get(i))
            .map_or(0, |e| e.season);
        let focus = cx.focus_handle();
        focus.focus(window);
        video.sync_size();
        video.set_volume(volume);
        let tuning_note = (!tuning.is_default()).then(|| {
            video.set_subtitle_delay(tuning.delay);
            video.set_subtitle_scale(tuning.scale);
            tuning_note(tuning).into()
        });

        let bounds = cx.observe_window_bounds(window, |this, _, _| this.video.sync_size());
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
            focus,
            title,
            subtitle,
            episodes,
            current,
            episode_season,
            up_next: None,
            prefs,
            quality,
            video_ready_since: None,
            expected_path: None,
            audio_settled: false,
            subtitle_settled: false,
            segments: Vec::new(),
            segment_sources: Default::default(),
            chapters_read: false,
            chapters: Vec::new(),
            credits_dismissed: false,
            status: Status {
                position: 0.0,
                duration: 0.0,
                buffered: None,
                paused: false,
                buffering: true,
                ended: false,
                video_shown: false,
                volume,
                muted: false,
                speed: 1.0,
                subtitle_scale: tuning.scale,
                subtitle_delay: tuning.delay,
            },
            tracks: Vec::new(),
            tracks_read: Instant::now(),
            pending_subtitles: Vec::new(),
            added_subtitles: Vec::new(),
            opened: Instant::now(),
            controls: controls::Controls::default(),
            last_good: None,
            stall: None,
            panel: None,
            quality_menu: menu::Shown::default(),
            drag: None,
            scrub: None,
            seek_hover: None,
            bar_hovered: false,
            reveal_from: None,
            toasts: Notes::default(),
            toast_at: None,
            notice: Notes::default(),
            loading_since: None,
            fades: Fades::new(),
            up_next_shown: (0, false),
            skip_shown: "",
            preview_x: Pixels::ZERO,
            season_switch: None,
            swaps: Swaps::default(),
            speed_shown: 1.0,
            tuning_note,
            subtitle_position: 100.0,
            exited: false,
            last_activity: Instant::now(),
            last_report: Instant::now(),
            reported_paused: false,
            pipeline: String::new(),
            previews: None,
            seek_bounds: Rc::default(),
            volume_bounds: Rc::default(),
            media,
            media_report: None,
            _poll: poll,
            _subscriptions: vec![bounds],
        }
    }

    /// Subtitle files that live next to the video (for example on a
    /// Jellyfin server). They're added as soon as the video has loaded.
    /// Intro, recap and credits markers from `source`. For each kind, the
    /// best source that has it is used.
    pub fn add_segments(&mut self, source: SegmentSource, segments: Vec<Segment>) {
        self.segment_sources[source as usize] = segments;
        let sources: Vec<&[Segment]> = self.segment_sources.iter().map(Vec::as_slice).collect();
        let merged = segments::merge(&sources);
        if merged != self.segments {
            eprintln!("[player] segments: {merged:?}");
            self.segments = merged;
        }
    }

    pub fn set_controls(&mut self, controls: controls::Controls) {
        self.controls = controls;
    }

    pub fn add_external_subtitles(&mut self, subtitles: Vec<ExternalSubtitle>) {
        self.pending_subtitles.extend(subtitles);
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
                || self.segments.iter().any(|s| {
                    // Only closing credits: some files mark opening ones too.
                    s.kind == SegmentKind::Credits
                        && s.start >= duration * END_CREDITS_FROM
                        && position >= s.start
                }))
    }

    /// Switches to another stream of the same video (a different quality),
    /// at `start`, with the subtitle files that stream needs. Remembered
    /// track choices are applied again once it loads.
    /// Saves the video as shown to `path` (tests).
    pub fn screenshot(&self, path: &std::path::Path) {
        if let Err(e) = self.video.screenshot(path) {
            eprintln!("[test] {e:#}");
        }
    }

    /// Shows pictures from `previews` when hovering the seek bar.
    pub fn set_previews(&mut self, previews: Previews) {
        self.previews = Some(previews);
    }

    pub fn reload(
        &mut self,
        url: &str,
        start: f64,
        subtitles: Vec<ExternalSubtitle>,
        quality: Quality,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.video.load(url, start) {
            self.toast(format!("Couldn't switch quality: {e:#}"));
            return cx.notify();
        }
        self.expected_path = Some(url.to_string());
        self.stall = None;
        self.added_subtitles.clear();
        self.video_ready_since = None;
        self.status.video_shown = false;
        self.tracks.clear();
        self.pending_subtitles = subtitles;
        self.audio_settled = false;
        self.subtitle_settled = false;
        self.up_next = None;
        self.quality = Some(quality);
        if quality.transcodes() {
            // Previews read the original file; not over a slow connection.
            self.previews = None;
        }
        self.status.buffering = true;
        self.toast(format!("Quality: {}", quality.label()));
        cx.notify();
    }

    /// Switches to `quality` (from the quality menu).
    fn choose_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        self.quality_menu.close();
        if self.quality != Some(quality) {
            cx.emit(PlayerEvent::QualityChosen(quality));
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let v = &self.video;
        self.status = Status {
            position: v.position(),
            duration: v.duration(),
            buffered: v.buffered_until(),
            paused: v.paused(),
            buffering: v.buffering() || !v.loaded(),
            ended: v.ended(),
            video_shown: self.status.video_shown,
            volume: v.volume(),
            muted: v.muted(),
            speed: v.speed(),
            subtitle_scale: v.subtitle_scale(),
            subtitle_delay: v.subtitle_delay(),
        };
        if self.handle_media_commands(cx) {
            return;
        }
        if self.check_stall(cx) {
            cx.notify();
            return;
        }
        self.report_media();
        let pipeline = self.video.pipeline();
        if pipeline != self.pipeline {
            eprintln!("[video] {pipeline}");
            self.pipeline = pipeline;
        }
        // After a reload, wait for mpv to switch to the new source.
        let switched = match &self.expected_path {
            Some(expected) => self.video.path().as_deref() == Some(expected.as_str()),
            None => true,
        };
        if switched && self.video.video_configured() {
            let since = *self.video_ready_since.get_or_insert_with(Instant::now);
            self.status.video_shown = since.elapsed() >= VIDEO_SETTLE;
            if self.status.video_shown
                && let Some(note) = self.tuning_note.take()
            {
                self.toast(note);
            }
        } else {
            self.video_ready_since = None;
            self.status.video_shown = false;
        }
        if switched && self.video.loaded() {
            self.expected_path = None;
            let mut added = false;
            if !self.pending_subtitles.is_empty() {
                for subtitle in std::mem::take(&mut self.pending_subtitles) {
                    if let Err(e) = self.video.add_subtitle(&subtitle) {
                        eprintln!("[video] {e:#}");
                    }
                    self.added_subtitles.push(subtitle);
                }
                added = true;
            }
            if added
                || self.tracks.is_empty()
                || (self.panel == Some(Panel::Tracks)
                    && self.tracks_read.elapsed() >= TRACKS_REFRESH)
            {
                self.read_tracks();
            }
            if !self.chapters_read && self.status.duration > 0.0 {
                self.chapters_read = true;
                self.chapters = self.video.chapters();
                let found = segments::from_chapters(&self.chapters, self.status.duration);
                self.add_segments(SegmentSource::Chapters, found);
            }
            // Once when the file opens, and again when subtitle files arrive.
            if added || !self.audio_settled {
                self.apply_prefs();
            }
        }
        if self.video.ended() {
            // Report the full runtime so the item is marked watched.
            self.status.position = self.status.duration;
            match (self.next_episode().map(|e| e.key.clone()), self.up_next) {
                (Some(key), None) => {
                    eprintln!("[player] ended; up next: {key} in {UP_NEXT_SECONDS} s");
                    self.report(cx);
                    self.up_next = Some(Instant::now() + Duration::from_secs(UP_NEXT_SECONDS));
                }
                (Some(key), Some(at)) if Instant::now() >= at => {
                    return self.play_episode(key, cx);
                }
                (Some(_), Some(_)) => {}
                (None, _) => return self.exit(cx),
            }
            cx.notify();
            return;
        }
        if self.status.paused
            && let Some(at) = &mut self.up_next
        {
            // The countdown waits while paused.
            *at += TICK;
        }
        let in_credits = self
            .segment_at(self.status.position)
            .is_some_and(|s| s.kind == SegmentKind::Credits);
        let next = self.next_episode().map(|e| e.key.clone());
        match (in_credits, next, self.up_next) {
            (true, Some(key), None) if !self.credits_dismissed => {
                eprintln!("[player] credits; up next: {key} in {UP_NEXT_SECONDS} s");
                self.up_next = Some(Instant::now() + Duration::from_secs(UP_NEXT_SECONDS));
            }
            (true, Some(_), Some(at)) if Instant::now() >= at => return self.play_next(cx),
            (true, _, _) => {}
            (false, _, _) => {
                // Seeking back out of the credits (or the end) cancels it.
                self.up_next = None;
                self.credits_dismissed = false;
            }
        }
        let paused = self.reporting_paused();
        if paused != self.reported_paused
            || (!paused && self.last_report.elapsed() >= PROGRESS_INTERVAL)
        {
            self.report(cx);
        }
        cx.notify();
    }

    /// Carries out what the system's media controls asked for. True if
    /// the player is leaving (for another episode).
    fn handle_media_commands(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(media) = self.media.clone() else {
            return false;
        };
        for command in media.commands() {
            eprintln!("[media] {command:?}");
            match command {
                MediaCommand::Play if self.status.paused => self.toggle_pause(),
                MediaCommand::Pause if !self.status.paused => self.toggle_pause(),
                MediaCommand::Play | MediaCommand::Pause => {}
                MediaCommand::Toggle => self.toggle_pause(),
                MediaCommand::SeekBy(seconds) => self.skip(seconds),
                MediaCommand::SeekTo(seconds) => {
                    self.video.seek_to(seconds);
                    self.status.position = seconds;
                }
                MediaCommand::Next => {
                    if self.next_episode().is_some() {
                        self.play_next(cx);
                        return true;
                    }
                }
                MediaCommand::Previous => {
                    if let Some(key) = self.previous_episode().map(|e| e.key.clone()) {
                        self.play_episode(key, cx);
                        return true;
                    }
                }
            }
            self.poke(cx);
        }
        false
    }

    /// Keeps the system's now-playing panel in step with the player.
    fn report_media(&mut self) {
        let Some(media) = &self.media else {
            return;
        };
        let playing = !self.status.paused && !self.status.ended;
        let last = self
            .media_report
            .map(|(was_playing, at, when)| (was_playing, at, when.elapsed().as_secs_f64()));
        if media_session::needs_report(last, playing, self.status.position, self.status.speed) {
            media.update(playing, self.status.position, self.status.duration);
            self.media_report = Some((playing, self.status.position, Instant::now()));
        }
    }

    fn segment_at(&self, position: f64) -> Option<Segment> {
        self.segments
            .iter()
            .find(|s| s.start <= position && position < s.end)
            .copied()
    }

    /// The skip button's label and where it jumps to, while one applies.
    fn skip_target(&self) -> Option<(&'static str, f64)> {
        let segment = self.segment_at(self.status.position)?;
        let label = match segment.kind {
            SegmentKind::Intro => "Skip Intro",
            SegmentKind::Recap => "Skip Recap",
            SegmentKind::Preview => "Skip Preview",
            SegmentKind::Credits => return None,
        };
        // Not in the last second, where skipping would barely move.
        (self.status.position < segment.end - 1.0).then_some((label, segment.end))
    }

    fn skip_segment(&mut self) {
        if let Some((_, end)) = self.skip_target() {
            self.video.seek_to(end);
            self.status.position = end;
        }
    }

    /// Dismisses the credits countdown and keeps watching.
    fn watch_credits(&mut self) {
        self.credits_dismissed = true;
        self.up_next = None;
    }

    fn next_episode(&self) -> Option<&Episode> {
        self.episodes.get(self.current? + 1)
    }

    fn previous_episode(&self) -> Option<&Episode> {
        self.episodes.get(self.current?.checked_sub(1)?)
    }

    /// Leaves this video for another episode.
    fn play_episode(&mut self, key: String, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.report(cx);
        cx.emit(PlayerEvent::Play(key));
    }

    /// Seconds left on the "Up next" countdown, while it runs.
    fn up_next_remaining(&self) -> Option<u64> {
        let at = self.up_next?;
        Some(
            at.saturating_duration_since(Instant::now())
                .as_secs_f64()
                .ceil() as u64,
        )
    }

    fn seasons(&self) -> Vec<u16> {
        let mut seasons: Vec<u16> = self.episodes.iter().map(|e| e.season).collect();
        seasons.dedup();
        seasons
    }

    fn read_tracks(&mut self) {
        self.tracks = self.video.tracks();
        self.tracks_read = Instant::now();
    }

    fn tracks_of(&self, kind: TrackKind) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(move |t| t.kind == kind)
    }

    /// Paused by the viewer, or held up by a lost connection. At the end
    /// mpv also pauses, but that's the video finishing, not someone
    /// stepping away.
    fn reporting_paused(&self) -> bool {
        self.stall.is_some() || (self.status.paused && !self.status.ended)
    }

    /// Notices a network stream breaking off (an "end" well before the
    /// end, or a stream that couldn't be opened) and opens it again where
    /// it stopped, every few seconds, until the server answers. Meanwhile
    /// the video holds its place: it isn't finished, nothing comes up
    /// next. True while that goes on.
    fn check_stall(&mut self, cx: &mut Context<Self>) -> bool {
        let url = self.video.source();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return false; // a local file
        }
        let duration = self.status.duration;
        let open = self.video.loaded() && !self.video.idle();
        if let Some(stall) = &mut self.stall {
            let at = stall.at;
            if open && !self.video.ended() && duration > 0.0 {
                eprintln!(
                    "[player] connection back after {:.0} s; carrying on at {at:.0} s",
                    stall.since.elapsed().as_secs_f64()
                );
                let play = stall.play;
                self.stall = None;
                self.video.set_pause(!play);
                self.toast("Connected again");
                return false;
            }
            self.status.position = at;
            self.status.paused = !stall.play;
            self.status.ended = false;
            self.status.buffering = false;
            if Instant::now() >= stall.next_try {
                let wait = REOPEN_WAITS[stall.tries.min(REOPEN_WAITS.len() - 1)];
                stall.tries += 1;
                stall.next_try = Instant::now() + Duration::from_secs(wait);
                self.reopen(&url, at);
            }
            return true;
        }
        let position = self.status.position;
        // Idle: opening failed (while it opens, mpv isn't idle, but give it
        // a moment at the very start).
        let failed_to_open = self.video.idle()
            && (self.last_good.is_some() || self.opened.elapsed() >= Duration::from_secs(3));
        let broke_off = failed_to_open
            || (self.video.ended() && duration > 0.0 && position < duration - BROKE_OFF_BEFORE_END);
        if !broke_off {
            if open && !self.video.ended() && position > 0.0 {
                self.last_good = Some(position);
            }
            return false;
        }
        let at = self.last_good.unwrap_or_else(|| self.video.last_start());
        eprintln!("[player] the stream broke off at {at:.0} s; reconnecting");
        self.stall = Some(Stall {
            at,
            since: Instant::now(),
            next_try: Instant::now() + Duration::from_secs(REOPEN_WAITS[0]),
            tries: 0,
            // Keep playing once it's back, unless the viewer had paused.
            play: !self.reported_paused,
        });
        self.up_next = None;
        self.status.position = at;
        self.status.ended = false;
        // Tells the server it's held up here (not finished).
        self.report(cx);
        true
    }

    fn reopen(&mut self, url: &str, at: f64) {
        if let Err(e) = self.video.load(url, at) {
            eprintln!("[player] reopening the stream: {e:#}");
            return;
        }
        self.expected_path = Some(url.to_string());
        self.video_ready_since = None;
        self.status.video_shown = false;
        self.tracks.clear();
        self.pending_subtitles = std::mem::take(&mut self.added_subtitles);
        self.audio_settled = false;
        self.subtitle_settled = false;
    }

    fn report(&mut self, cx: &mut Context<Self>) {
        self.last_report = Instant::now();
        if self.status.duration > 0.0 {
            let paused = self.reporting_paused();
            self.reported_paused = paused;
            cx.emit(PlayerEvent::Progress {
                position: self.status.position,
                duration: self.status.duration,
                paused,
            });
        }
    }

    fn exit(&mut self, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.report(cx);
        cx.emit(PlayerEvent::Exit);
    }

    fn poke(&mut self, cx: &mut Context<Self>) {
        self.last_activity = Instant::now();
        cx.notify();
    }

    fn toast(&mut self, message: impl Into<SharedString>) {
        self.toasts.show(Some(message.into()));
        self.toast_at = Some(Instant::now());
    }

    fn controls_visible(&self) -> bool {
        self.status.paused
            || self.panel.is_some()
            || self.up_next.is_some()
            || self.drag.is_some()
            || self.bar_hovered
            || self.last_activity.elapsed() < CONTROLS_TIMEOUT
    }

    // ---- actions shared by buttons and keys ----

    fn toggle_pause(&mut self) {
        if let Some(stall) = &mut self.stall {
            // Applied once the stream is back.
            stall.play = !stall.play;
            self.status.paused = !stall.play;
            return;
        }
        self.video.toggle_pause();
        self.status.paused = !self.status.paused;
    }

    fn skip(&mut self, seconds: f64) {
        self.video.seek_by(seconds);
    }

    fn set_volume(&mut self, volume: f64) {
        let volume = volume.clamp(0.0, 100.0);
        self.video.set_volume(volume);
        self.status.volume = volume;
        if self.status.muted && volume > 0.0 {
            self.video.set_muted(false);
            self.status.muted = false;
        }
    }

    fn toggle_mute(&mut self) {
        self.status.muted = !self.status.muted;
        self.video.set_muted(self.status.muted);
    }

    /// Selects the remembered languages, if the file has them. A subtitle
    /// language that isn't there yet may still arrive as an external file.
    fn apply_prefs(&mut self) {
        let mut changed = false;
        if !self.audio_settled {
            if let Some(id) = self
                .prefs
                .audio
                .as_deref()
                .and_then(|lang| tracks::pick_audio(&self.tracks, lang))
                && !self.tracks.iter().any(|t| t.id == id && t.selected)
            {
                self.video.select_audio(id);
                changed = true;
            }
            self.audio_settled = true;
        }
        if !self.subtitle_settled {
            match &self.prefs.subtitle {
                None => self.subtitle_settled = true,
                Some(SubtitlePref::Off) => {
                    self.video.select_subtitle(None);
                    self.subtitle_settled = true;
                    changed = true;
                }
                Some(SubtitlePref::Language(lang)) => {
                    if let Some(id) = tracks::pick_subtitle(&self.tracks, lang) {
                        self.video.select_subtitle(Some(id));
                        self.subtitle_settled = true;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.read_tracks();
        }
    }

    /// The viewer's pick; remembered for next time.
    fn select_audio(&mut self, id: i64, cx: &mut Context<Self>) {
        self.video.select_audio(id);
        self.audio_settled = true;
        self.read_tracks();
        if let Some(lang) = self
            .tracks
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.lang.clone())
        {
            self.prefs.audio = Some(lang.clone());
            cx.emit(PlayerEvent::AudioChosen(lang));
        }
    }

    /// The viewer's pick (`None` = off); remembered for next time.
    fn select_subtitle(&mut self, id: Option<i64>, cx: &mut Context<Self>) {
        self.video.select_subtitle(id);
        self.subtitle_settled = true;
        self.read_tracks();
        let pref = match id {
            None => Some(SubtitlePref::Off),
            Some(id) => self
                .tracks
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| t.lang.clone())
                .map(SubtitlePref::Language),
        };
        if let Some(pref) = pref {
            self.prefs.subtitle = Some(pref.clone());
            cx.emit(PlayerEvent::SubtitleChosen(pref));
        }
    }

    /// Steps to the next track of `kind`; subtitles include "off".
    fn cycle(&mut self, kind: TrackKind, cx: &mut Context<Self>) {
        self.read_tracks();
        let ids: Vec<i64> = self.tracks_of(kind).map(|t| t.id).collect();
        let current = self.tracks_of(kind).find(|t| t.selected).map(|t| t.id);
        let mut choices: Vec<Option<i64>> = ids.into_iter().map(Some).collect();
        if kind == TrackKind::Subtitle {
            choices.push(None);
        }
        if choices.is_empty() {
            self.toast(match kind {
                TrackKind::Audio => "No audio tracks",
                TrackKind::Subtitle => "No subtitles",
            });
            return;
        }
        let next = choices
            .iter()
            .position(|c| *c == current)
            .map_or(0, |i| (i + 1) % choices.len());
        let choice = choices[next];
        match kind {
            TrackKind::Audio => self.select_audio(choice.unwrap_or_default(), cx),
            TrackKind::Subtitle => self.select_subtitle(choice, cx),
        }
        let name = choice
            .and_then(|id| {
                let (number, track) = self.tracks_of(kind).enumerate().find(|(_, t)| t.id == id)?;
                Some(tracks::label(track, number + 1).primary)
            })
            .unwrap_or_else(|| "Off".into());
        self.toast(match kind {
            TrackKind::Audio => format!("Audio: {name}"),
            TrackKind::Subtitle => format!("Subtitles: {name}"),
        });
    }

    fn step_speed(&mut self, direction: i32) {
        let current = SPEEDS
            .iter()
            .position(|s| (s - self.status.speed).abs() < 0.01)
            .unwrap_or(2);
        let next = (current as i32 + direction).clamp(0, SPEEDS.len() as i32 - 1) as usize;
        self.video.set_speed(SPEEDS[next]);
        self.status.speed = SPEEDS[next];
        self.toast(format!("Speed {}", view::format_speed(SPEEDS[next])));
    }

    fn step_subtitle_scale(&mut self, delta: f64, cx: &mut Context<Self>) {
        let scale = ((self.status.subtitle_scale + delta) * 10.0).round() / 10.0;
        let scale = scale.clamp(0.5, 2.5);
        self.video.set_subtitle_scale(scale);
        self.status.subtitle_scale = scale;
        self.tuned(cx);
    }

    fn step_subtitle_delay(&mut self, delta: f64, cx: &mut Context<Self>) {
        let delay = ((self.status.subtitle_delay + delta) * 10.0).round() / 10.0;
        self.video.set_subtitle_delay(delay);
        self.status.subtitle_delay = delay;
        self.toast(format!("Subtitle delay {}", view::format_delay(delay)));
        self.tuned(cx);
    }

    /// The delay or size changed: remembered for the show's next episodes.
    fn tuned(&mut self, cx: &mut Context<Self>) {
        // A change made before the video shows makes the note out of date.
        self.tuning_note = None;
        cx.emit(PlayerEvent::SubtitleTuned(SubtitleTuning {
            delay: self.status.subtitle_delay,
            scale: self.status.subtitle_scale,
        }));
    }

    /// Opens `panel`, or closes it if it's already open.
    fn toggle_panel(&mut self, panel: Panel) {
        self.panel = if self.panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
        match self.panel {
            Some(Panel::Tracks) => self.read_tracks(),
            Some(Panel::Episodes) => {
                if let Some(e) = self.current.and_then(|i| self.episodes.get(i)) {
                    self.episode_season = e.season;
                }
            }
            _ => {}
        }
    }

    fn play_next(&mut self, cx: &mut Context<Self>) {
        if self.up_next.is_some() {
            // Leaving during the credits: this episode counts as watched.
            self.status.position = self.status.duration;
        }
        match self.next_episode().map(|e| e.key.clone()) {
            Some(key) => self.play_episode(key, cx),
            None => self.toast("This is the last episode"),
        }
    }

    /// Keeps subtitles above the control bar while it's showing.
    /// `raised` is how far (0 to 1): it follows the bar fading in and out,
    /// so the subtitles glide rather than jump.
    fn place_subtitles(&mut self, window: &Window, raised: f32) {
        let height = f32::from(window.viewport_size().height);
        let target = if height > 0.0 {
            100.0 - (view::BAR_CLEARANCE / height * 100.0 * raised) as f64
        } else {
            100.0
        };
        if (target - self.subtitle_position).abs() >= 0.05 {
            self.video.set_subtitle_position(target);
            self.subtitle_position = target;
        }
    }

    // ---- input ----

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let question_mark = event.keystroke.key_char.as_deref() == Some("?");
        if question_mark {
            self.toggle_panel(Panel::Shortcuts);
            return self.poke(cx);
        }
        let key = event.keystroke.unparse();
        if key == "escape" {
            // The innermost thing open closes first.
            if self.quality_menu.close() {
                // Closed (fading out).
            } else if self.panel.is_some() {
                self.panel = None;
            } else if window.is_fullscreen() {
                window.toggle_fullscreen();
            } else {
                return self.exit(cx);
            }
            return self.poke(cx);
        }
        // Each action on the key in turn, until one applies (the skip only
        // does while offered).
        for action in self.controls.actions_for(&key) {
            match self.run(action, window, cx) {
                Ran::No => continue,
                Ran::Yes => return self.poke(cx),
                Ran::Left => return,
            }
        }
    }

    fn run(
        &mut self,
        action: controls::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Ran {
        use controls::Action;
        let has_episodes = !self.episodes.is_empty();
        match action {
            Action::Skip if self.up_next.is_some() => {
                self.play_next(cx);
                return Ran::Left;
            }
            Action::Skip if self.skip_target().is_some() => self.skip_segment(),
            Action::Skip => return Ran::No,
            Action::PlayPause => self.toggle_pause(),
            Action::Back => self.skip(-self.controls.back_seconds),
            Action::Forward => self.skip(self.controls.forward_seconds),
            Action::PreviousMark => {
                if !self.jump_to_mark(false) {
                    return Ran::No;
                }
            }
            Action::NextMark => {
                if !self.jump_to_mark(true) {
                    return Ran::No;
                }
            }
            Action::VolumeUp => {
                self.set_volume(self.status.volume + self.controls.volume_step);
                self.toast(format!("Volume {:.0}%", self.status.volume));
            }
            Action::VolumeDown => {
                self.set_volume(self.status.volume - self.controls.volume_step);
                self.toast(format!("Volume {:.0}%", self.status.volume));
            }
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
            Action::Fullscreen => window.toggle_fullscreen(),
            Action::NextEpisode if has_episodes => {
                self.play_next(cx);
                return Ran::Left;
            }
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

    /// Where the previous / next intro, recap, credits or chapter starts
    /// (or a skipped part ends), with what it is.
    fn marks(&self) -> Vec<(f64, String)> {
        let mut marks: Vec<(f64, String)> = vec![(0.0, "The beginning".to_string())];
        for segment in &self.segments {
            let name = match segment.kind {
                SegmentKind::Intro => "intro",
                SegmentKind::Recap => "recap",
                SegmentKind::Preview => "preview",
                SegmentKind::Credits => "credits",
            };
            let mut capital = name.to_string();
            capital[..1].make_ascii_uppercase();
            marks.push((segment.start, capital));
            if segment.kind != SegmentKind::Credits {
                marks.push((segment.end, format!("After the {name}")));
            }
        }
        for (title, start) in &self.chapters {
            let title = if title.trim().is_empty() {
                "Chapter".to_string()
            } else {
                title.clone()
            };
            marks.push((*start, title));
        }
        marks.retain(|(at, _)| self.status.duration <= 0.0 || *at < self.status.duration - 1.0);
        marks.sort_by(|a, b| a.0.total_cmp(&b.0));
        marks.dedup_by(|a, b| (a.0 - b.0).abs() < 1.0);
        marks
    }

    /// Jumps to the next mark, or back to the previous one (a moment into
    /// a part, back goes to its start; right at it, to the one before).
    /// False if there's none that way.
    fn jump_to_mark(&mut self, forward: bool) -> bool {
        let position = self.status.position;
        let marks = self.marks();
        let target = if forward {
            marks.into_iter().find(|(at, _)| *at > position + 1.0)
        } else {
            marks.into_iter().rev().find(|(at, _)| *at < position - 3.0)
        };
        let Some((at, name)) = target else {
            eprintln!(
                "[player] no mark {} {position:.1} s",
                if forward { "after" } else { "before" }
            );
            return false;
        };
        eprintln!("[player] mark: {position:.1} s -> {at:.1} s ({name})");
        self.video.seek_to(at);
        self.status.position = at;
        self.up_next = None;
        self.toast(format!("{name} · {}", format_time(at)));
        true
    }

    fn on_stage_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Clicking the video never pauses it (too easy to do by accident):
        // a click shows or hides the controls, a double-click is fullscreen.
        if self.panel.is_some() {
            // A click outside the panel just closes it.
            self.panel = None;
        } else if event.click_count == 2 {
            window.toggle_fullscreen();
        } else if self.controls_visible() {
            self.hide_controls();
            return cx.notify();
        }
        self.poke(cx);
    }

    /// Hides the controls now, as if the mouse had been idle. (They stay up
    /// while paused or while a panel or countdown is showing.)
    fn hide_controls(&mut self) {
        self.bar_hovered = false;
        self.last_activity = Instant::now()
            .checked_sub(CONTROLS_TIMEOUT)
            .unwrap_or_else(Instant::now);
    }

    fn on_seek_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.drag = Some(Drag::Seek);
        self.scrub_to(event.position);
        self.poke(cx);
    }

    fn on_volume_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.drag = Some(Drag::Volume);
        self.set_volume(fraction_at(self.volume_bounds.get(), event.position.x) * 100.0);
        self.poke(cx);
    }

    fn scrub_to(&mut self, position: Point<Pixels>) {
        if self.status.duration > 0.0 {
            let time = fraction_at(self.seek_bounds.get(), position.x) * self.status.duration;
            self.scrub = Some(time);
            self.video.seek_to_keyframe(time);
        }
    }

    /// Whether a mouse move should bring hidden controls back: only a
    /// deliberate one, not a nudge or jitter.
    fn reveals(&mut self, position: Point<Pixels>) -> bool {
        let now = Instant::now();
        match self.reveal_from {
            Some((origin, since)) if now.duration_since(since) < REVEAL_WINDOW => {
                let (dx, dy) = (
                    f32::from(position.x - origin.x),
                    f32::from(position.y - origin.y),
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
                self.reveal_from = Some((position, now));
                false
            }
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.controls_visible() && !self.reveals(event.position) {
            return;
        }
        match self.drag {
            Some(_) if event.pressed_button != Some(MouseButton::Left) => {
                // The button was released outside the window.
                self.finish_drag();
            }
            Some(Drag::Seek) => self.scrub_to(event.position),
            Some(Drag::Volume) => {
                self.set_volume(fraction_at(self.volume_bounds.get(), event.position.x) * 100.0)
            }
            None => {}
        }
        let seek = self.seek_bounds.get();
        self.seek_hover = (seek.contains(&event.position) || self.drag == Some(Drag::Seek))
            .then_some(event.position.x);
        self.poke(cx);
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Only the end of a slider drag counts as activity; otherwise the
        // release of a click that hid the controls would bring them back.
        if self.drag.is_some() {
            self.finish_drag();
            self.poke(cx);
        }
    }

    fn finish_drag(&mut self) {
        if self.drag.take() == Some(Drag::Seek)
            && let Some(time) = self.scrub.take()
        {
            // Land exactly where the bar was released.
            self.video.seek_to(time);
            self.status.position = time;
        }
    }
}
