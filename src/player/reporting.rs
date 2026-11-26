//! What the player tells the outside: progress reports for resume and sync,
//! leaving, and the system's media controls.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::Context;

use super::status::Status;
use super::{Player, PlayerEvent};
use crate::media_session::{self, MediaCommand, MediaSession};

/// How often the position is reported while playing.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

/// Whether a progress report is due: pausing or playing again is said at
/// once, the position every `PROGRESS_INTERVAL`, but only while playing.
fn progress_due(paused: bool, said_paused: bool, since_last: Duration) -> bool {
    paused != said_paused || (!paused && since_last >= PROGRESS_INTERVAL)
}

/// What the system's now-playing panel was last told.
#[derive(Debug, Clone, Copy)]
struct MediaReport {
    playing: bool,
    position: f64,
    at: Instant,
}

/// The last reports sent, and whether the player has left.
pub(super) struct Reporting {
    last_report: Instant,
    /// Whether the last report said paused.
    said_paused: bool,
    /// The system's media controls (headphone buttons, media keys).
    media: Option<Rc<MediaSession>>,
    media_report: Option<MediaReport>,
    /// The player has said it's leaving: nothing more is reported.
    left: bool,
}

impl Reporting {
    pub(super) fn new(media: Option<Rc<MediaSession>>) -> Self {
        Self {
            last_report: Instant::now(),
            said_paused: false,
            media,
            media_report: None,
            left: false,
        }
    }

    pub(super) fn has_left(&self) -> bool {
        self.left
    }

    pub(super) fn said_paused(&self) -> bool {
        self.said_paused
    }

    pub(super) fn progress_due(&self, paused: bool) -> bool {
        progress_due(paused, self.said_paused, self.last_report.elapsed())
    }

    /// Marks the player as leaving. False if it already was.
    fn leave(&mut self) -> bool {
        !std::mem::replace(&mut self.left, true)
    }

    /// Keeps the system's now-playing panel in step with the player.
    pub(super) fn report_media(&mut self, status: &Status) {
        let Some(media) = &self.media else {
            return;
        };
        let playing = !status.paused && !status.ended;
        let last = self
            .media_report
            .map(|last| (last.playing, last.position, last.at.elapsed().as_secs_f64()));
        if media_session::needs_report(last, playing, status.position, status.speed) {
            media.update(playing, status.position, status.duration);
            self.media_report = Some(MediaReport {
                playing,
                position: status.position,
                at: Instant::now(),
            });
        }
    }
}

impl Player {
    /// Paused by the viewer, or held up by a lost connection. At the end mpv
    /// also pauses, but that's the video finishing, not someone stepping away.
    pub(super) fn reporting_paused(&self) -> bool {
        self.stream.is_stalled() || (self.status.paused && !self.status.ended)
    }

    pub(super) fn report(&mut self, cx: &mut Context<Self>) {
        self.reporting.last_report = Instant::now();
        if self.status.duration > 0.0 {
            let paused = self.reporting_paused();
            self.reporting.said_paused = paused;
            cx.emit(PlayerEvent::Progress {
                position: self.status.position,
                duration: self.status.duration,
                paused,
            });
        }
    }

    pub(super) fn exit(&mut self, cx: &mut Context<Self>) {
        if self.reporting.leave() {
            self.report(cx);
            cx.emit(PlayerEvent::Exit);
        }
    }

    /// Leaves this video for another episode.
    pub(super) fn play_episode(&mut self, key: String, cx: &mut Context<Self>) {
        if self.reporting.leave() {
            self.report(cx);
            cx.emit(PlayerEvent::Play(key));
        }
    }

    /// Carries out what the system's media controls asked for. True if the
    /// player is leaving (for another episode).
    pub(super) fn handle_media_commands(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(media) = self.reporting.media.clone() else {
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
                    if self.playing.next().is_some() {
                        self.play_next(cx);
                        return true;
                    }
                }
                MediaCommand::Previous => {
                    if let Some(key) = self.playing.previous().map(|e| e.key.clone()) {
                        self.play_episode(key, cx);
                        return true;
                    }
                }
            }
            self.poke(cx);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOON: Duration = Duration::from_secs(1);

    #[test]
    fn pausing_and_playing_again_are_reported_at_once() {
        assert!(progress_due(true, false, SOON));
        assert!(progress_due(false, true, SOON));
    }

    #[test]
    fn the_position_is_reported_every_few_seconds_while_playing() {
        assert!(!progress_due(false, false, SOON));
        assert!(!progress_due(false, false, Duration::from_millis(4900)));
        assert!(progress_due(false, false, PROGRESS_INTERVAL));
    }

    #[test]
    fn a_paused_player_reports_once() {
        assert!(!progress_due(true, true, PROGRESS_INTERVAL * 10));
    }

    #[test]
    fn the_player_leaves_only_once() {
        let mut reporting = Reporting::new(None);
        assert!(!reporting.has_left());
        assert!(reporting.leave());
        assert!(!reporting.leave());
        assert!(reporting.has_left());
    }
}
