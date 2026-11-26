//! The stream being played: waiting for mpv to open it, and opening it again
//! where it stopped when the connection to the server breaks off.

use std::time::{Duration, Instant};

use gpui::Context;

use super::Player;
use super::status::Status;
use crate::jellyfin::Quality;
use crate::video::{ExternalSubtitle, VideoSurface};

/// A network stream that "ends" this far before its end broke off: the
/// connection went and what was buffered ran out.
const BROKE_OFF_BEFORE_END: f64 = 20.0;
/// Seconds to wait between attempts to open a broken-off stream again.
const REOPEN_WAITS: [u64; 5] = [2, 3, 5, 8, 10];
/// How long mpv gets to open a stream at the very start before an idle
/// player counts as one that couldn't open it.
const OPENING_GRACE: Duration = Duration::from_secs(3);
/// How long mpv's video output must be up before the black backdrop is
/// lifted, so its first frame is on screen first.
const VIDEO_SETTLE: Duration = Duration::from_millis(150);

/// Whether `url` is streamed from a server (local files never break off).
fn is_network(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The wait before the next attempt, after `tries` attempts.
fn reopen_wait(tries: usize) -> Duration {
    Duration::from_secs(REOPEN_WAITS[tries.min(REOPEN_WAITS.len() - 1)])
}

/// What mpv says about the stream, for telling a break from an end.
#[derive(Debug, Clone, Copy)]
struct Signs {
    /// A file is loaded and mpv isn't idle.
    open: bool,
    idle: bool,
    ended: bool,
    position: f64,
    duration: f64,
}

impl Signs {
    fn read(video: &VideoSurface, status: &Status) -> Self {
        let idle = video.idle();
        Self {
            open: video.loaded() && !idle,
            idle,
            ended: video.ended(),
            position: status.position,
            duration: status.duration,
        }
    }

    /// The stream is open and playing: a stall is over.
    fn is_back(&self) -> bool {
        self.open && !self.ended && self.duration > 0.0
    }

    /// The stream "ended" well before its end: what was buffered ran out.
    fn ended_early(&self) -> bool {
        self.ended && self.duration > 0.0 && self.position < self.duration - BROKE_OFF_BEFORE_END
    }
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

impl Stall {
    fn begin(at: f64, play: bool, now: Instant) -> Self {
        Self {
            at,
            since: now,
            next_try: now + reopen_wait(0),
            tries: 0,
            play,
        }
    }

    /// Whether it's time to try again; if so, the next attempt is scheduled.
    fn take_turn(&mut self, now: Instant) -> bool {
        if now < self.next_try {
            return false;
        }
        self.next_try = now + reopen_wait(self.tries);
        self.tries += 1;
        true
    }
}

/// How far mpv is with the source it was last given.
pub(super) struct Stream {
    opened: Instant,
    /// The source mpv was last given, until it reports it (its state is the old one's).
    expected_path: Option<String>,
    /// When mpv's video output came up for the current source.
    video_ready_since: Option<Instant>,
    /// The video is on screen; until then the player paints black over the desktop behind.
    video_shown: bool,
    /// Where it last played fine.
    last_good: Option<f64>,
    /// The stream broke off; trying to open it again.
    stall: Option<Stall>,
    /// mpv's decoding pipeline as last logged.
    pipeline: String,
}

impl Stream {
    pub(super) fn new() -> Self {
        Self {
            opened: Instant::now(),
            expected_path: None,
            video_ready_since: None,
            video_shown: false,
            last_good: None,
            stall: None,
            pipeline: String::new(),
        }
    }

    /// The connection is lost and the stream is being opened again.
    pub(super) fn is_stalled(&self) -> bool {
        self.stall.is_some()
    }

    /// Flips play/pause for when a stalled stream is back; None if not stalled.
    pub(super) fn toggle_play_after_stall(&mut self) -> Option<bool> {
        let stall = self.stall.as_mut()?;
        stall.play = !stall.play;
        Some(stall.play)
    }

    /// Whether the stream broke off: it couldn't be opened, or ended early.
    fn broke_off(&self, signs: &Signs, now: Instant) -> bool {
        // While it opens, mpv isn't idle, but give it a moment at the very start.
        let failed_to_open = signs.idle
            && (self.last_good.is_some() || now.duration_since(self.opened) >= OPENING_GRACE);
        failed_to_open || signs.ended_early()
    }

    /// Remembers where it plays fine, to carry on from after a break.
    fn note_good(&mut self, signs: &Signs) {
        if signs.open && !signs.ended && signs.position > 0.0 {
            self.last_good = Some(signs.position);
        }
    }

    /// mpv was given `url`: until it reports that source, nothing is shown.
    fn expect(&mut self, url: &str) {
        self.expected_path = Some(url.to_string());
        self.video_ready_since = None;
        self.video_shown = false;
    }

    pub(super) fn video_shown(&self) -> bool {
        self.video_shown
    }

    /// Whether mpv has switched to the source it was last given.
    fn switched(&self, video: &VideoSurface) -> bool {
        match &self.expected_path {
            Some(expected) => video.path().as_deref() == Some(expected.as_str()),
            None => true,
        }
    }

    /// Shows the video once its output has been up long enough (`up`: mpv's
    /// video output is configured for the current source). Returns whether it shows.
    fn video_settled(&mut self, up: bool, now: Instant) -> bool {
        self.video_shown = if up {
            let since = *self.video_ready_since.get_or_insert(now);
            now.duration_since(since) >= VIDEO_SETTLE
        } else {
            self.video_ready_since = None;
            false
        };
        self.video_shown
    }

    /// Logs mpv's decoding pipeline when it changes.
    pub(super) fn log_pipeline(&mut self, video: &VideoSurface) {
        let pipeline = video.pipeline();
        if pipeline != self.pipeline {
            eprintln!("[video] {pipeline}");
            self.pipeline = pipeline;
        }
    }
}

impl Player {
    /// Switches to another stream of the same video (a different quality), at
    /// `start`, with its subtitle files. Remembered track choices apply again.
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
        self.stream.expect(url);
        self.stream.stall = None;
        self.tracks.reloaded(subtitles);
        self.up_next.cancel();
        self.tracks.quality.current = Some(quality);
        if quality.transcodes() {
            // Previews read the original file; not over a slow connection.
            self.sliders.seek.previews = None;
        }
        self.status.buffering = true;
        self.toast(format!("Quality: {}", quality.label()));
        cx.notify();
    }

    /// Notices a network stream breaking off and opens it again where it
    /// stopped, every few seconds. True while the video holds its place.
    pub(super) fn check_stall(&mut self, cx: &mut Context<Self>) -> bool {
        let url = self.video.source();
        if !is_network(&url) {
            return false;
        }
        let signs = Signs::read(&self.video, &self.status);
        if self.stream.is_stalled() {
            return self.hold_stalled(&url, &signs);
        }
        if !self.stream.broke_off(&signs, Instant::now()) {
            self.stream.note_good(&signs);
            return false;
        }
        let at = self
            .stream
            .last_good
            .unwrap_or_else(|| self.video.last_start());
        eprintln!("[player] the stream broke off at {at:.0} s; reconnecting");
        // Keep playing once it's back, unless the viewer had paused.
        let play = !self.reporting.said_paused();
        self.stream.stall = Some(Stall::begin(at, play, Instant::now()));
        self.up_next.cancel();
        self.status.position = at;
        self.status.ended = false;
        // Tells the server it's held up here (not finished).
        self.report(cx);
        true
    }

    /// Carries on if the stream is back; otherwise holds the place and tries
    /// to open it again when it's time. True while still stalled.
    fn hold_stalled(&mut self, url: &str, signs: &Signs) -> bool {
        let Some(stall) = &mut self.stream.stall else {
            return false;
        };
        let at = stall.at;
        if signs.is_back() {
            eprintln!(
                "[player] connection back after {:.0} s; carrying on at {at:.0} s",
                stall.since.elapsed().as_secs_f64()
            );
            let play = stall.play;
            self.stream.stall = None;
            self.video.set_pause(!play);
            self.toast("Connected again");
            return false;
        }
        self.status.position = at;
        self.status.paused = !stall.play;
        self.status.ended = false;
        self.status.buffering = false;
        if stall.take_turn(Instant::now()) {
            self.reopen(url, at);
        }
        true
    }

    fn reopen(&mut self, url: &str, at: f64) {
        if let Err(e) = self.video.load(url, at) {
            eprintln!("[player] reopening the stream: {e:#}");
            return;
        }
        self.stream.expect(url);
        self.tracks.reopened();
    }

    /// Follows mpv opening the source: lifts the black once the video is up,
    /// then adds subtitle files, reads tracks and chapters, applies choices.
    pub(super) fn follow_source(&mut self) {
        // After a reload, wait for mpv to switch to the new source.
        let switched = self.stream.switched(&self.video);
        let up = switched && self.video.video_configured();
        if self.stream.video_settled(up, Instant::now())
            && let Some(note) = self.tracks.take_tuning_note()
        {
            self.toast(note);
        }
        if switched && self.video.loaded() {
            self.stream.expected_path = None;
            self.source_loaded();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signs() -> Signs {
        Signs {
            open: true,
            idle: false,
            ended: false,
            position: 600.0,
            duration: 1400.0,
        }
    }

    fn seconds(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn only_server_addresses_are_network_streams() {
        assert!(is_network("http://server.local:8096/Videos/1/stream"));
        assert!(is_network("https://example.org/a.mkv"));
        assert!(!is_network("D:\\Films\\a.mkv"));
        assert!(!is_network("/home/me/a.mkv"));
    }

    #[test]
    fn an_end_well_before_the_end_is_a_break() {
        let stream = Stream::new();
        let now = stream.opened + seconds(60);
        let early = Signs {
            ended: true,
            ..signs()
        };
        assert!(stream.broke_off(&early, now));
        let at_the_end = Signs {
            ended: true,
            position: 1390.0,
            ..signs()
        };
        assert!(!stream.broke_off(&at_the_end, now));
        assert!(!stream.broke_off(&signs(), now));
    }

    #[test]
    fn an_end_with_no_known_length_is_not_a_break() {
        let stream = Stream::new();
        let unknown = Signs {
            ended: true,
            position: 0.0,
            duration: 0.0,
            ..signs()
        };
        assert!(!stream.broke_off(&unknown, stream.opened + seconds(60)));
    }

    #[test]
    fn an_idle_player_gets_a_moment_to_open_at_the_start() {
        let mut stream = Stream::new();
        let idle = Signs {
            open: false,
            idle: true,
            position: 0.0,
            duration: 0.0,
            ..signs()
        };
        assert!(!stream.broke_off(&idle, stream.opened + seconds(1)));
        assert!(stream.broke_off(&idle, stream.opened + seconds(3)));
        // Once it has played, idle means the stream is gone, at once.
        stream.note_good(&signs());
        assert!(stream.broke_off(&idle, stream.opened + seconds(1)));
    }

    #[test]
    fn the_last_good_place_is_only_taken_while_playing() {
        let mut stream = Stream::new();
        stream.note_good(&Signs {
            position: 0.0,
            ..signs()
        });
        assert_eq!(stream.last_good, None);
        stream.note_good(&signs());
        assert_eq!(stream.last_good, Some(600.0));
        stream.note_good(&Signs {
            ended: true,
            position: 700.0,
            ..signs()
        });
        assert_eq!(stream.last_good, Some(600.0));
    }

    #[test]
    fn a_stall_is_over_once_the_stream_plays_again() {
        assert!(signs().is_back());
        let closed = Signs {
            open: false,
            ..signs()
        };
        assert!(!closed.is_back());
        let no_length = Signs {
            duration: 0.0,
            ..signs()
        };
        assert!(!no_length.is_back());
    }

    #[test]
    fn reopening_waits_longer_each_time_up_to_ten_seconds() {
        let start = Instant::now();
        let mut stall = Stall::begin(600.0, true, start);
        let mut now = start;
        let mut waits = Vec::new();
        for _ in 0..8 {
            assert!(!stall.take_turn(now + seconds(1)));
            let wait = stall.next_try.duration_since(now);
            waits.push(wait.as_secs());
            now += wait;
            assert!(stall.take_turn(now));
        }
        assert_eq!(waits, [2, 2, 3, 5, 8, 10, 10, 10]);
    }

    #[test]
    fn the_video_shows_after_its_output_has_been_up_a_moment() {
        let mut stream = Stream::new();
        let start = Instant::now();
        assert!(!stream.video_settled(true, start));
        assert!(!stream.video_settled(true, start + Duration::from_millis(100)));
        assert!(stream.video_settled(true, start + Duration::from_millis(150)));
        // Going down starts the wait again.
        assert!(!stream.video_settled(false, start + Duration::from_millis(200)));
        assert!(!stream.video_settled(true, start + Duration::from_millis(250)));
        assert!(!stream.video_shown());

        // A new source hides it until its own output has settled.
        assert!(stream.video_settled(true, start + Duration::from_millis(400)));
        stream.expect("http://server/other");
        assert!(!stream.video_shown());
    }
}
