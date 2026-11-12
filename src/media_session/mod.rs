//! The system's media controls: headphone buttons, keyboard media keys, and
//! the now-playing panel (the Windows media flyout, the macOS Now Playing
//! menu). The OS sends commands from any thread; they queue up here until
//! the player collects them.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

use std::cell::Cell;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::Result;
use gpui::Window;

#[cfg(target_os = "macos")]
use mac::Controls;
#[cfg(windows)]
use win::Controls;

/// A request from the system's media controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaCommand {
    Play,
    Pause,
    #[cfg_attr(windows, allow(dead_code))] // Only macOS sends a bare toggle.
    Toggle,
    Next,
    Previous,
    /// Relative seek in seconds (negative goes back).
    SeekBy(f64),
    /// Absolute position in seconds.
    SeekTo(f64),
}

/// What the now-playing panel shows.
#[derive(Debug, Clone, Copy)]
pub struct NowPlaying<'a> {
    pub title: &'a str,
    pub subtitle: Option<&'a str>,
    pub thumb: Option<&'a Path>,
    pub has_previous: bool,
    pub has_next: bool,
}

/// The app's media session, one per window. Hidden while nothing plays.
pub struct MediaSession {
    controls: Controls,
    commands: Receiver<MediaCommand>,
    /// Whether next/previous are offered (a setting).
    skip_buttons: Cell<bool>,
    /// Previous and next episode exist, while something is shown.
    shown: Cell<Option<(bool, bool)>>,
}

type CommandSender = Sender<MediaCommand>;

impl MediaSession {
    pub fn new(window: &Window) -> Result<Self> {
        let (sender, commands) = channel();
        Ok(Self {
            controls: Controls::new(window, sender)?,
            commands,
            skip_buttons: Cell::new(true),
            shown: Cell::new(None),
        })
    }

    /// Turns the next/previous buttons (headphones, media keys, the panel)
    /// on or off; play and pause always work.
    pub fn set_skip_buttons(&self, on: bool) {
        self.skip_buttons.set(on);
        if let Some((previous, next)) = self.shown.get()
            && let Err(e) = self.controls.set_skip(previous && on, next && on)
        {
            eprintln!("[media] updating skip buttons: {e:#}");
        }
    }

    /// Shows `now` in the panel and starts taking commands. Commands that
    /// arrived while nothing was playing are dropped.
    pub fn show(&self, now: NowPlaying) {
        while self.commands.try_recv().is_ok() {}
        self.shown.set(Some((now.has_previous, now.has_next)));
        let skip = self.skip_buttons.get();
        let now = NowPlaying {
            has_previous: now.has_previous && skip,
            has_next: now.has_next && skip,
            ..now
        };
        if let Err(e) = self.controls.show(now) {
            eprintln!("[media] showing now playing: {e:#}");
        }
    }

    /// Reports the playback state, so the panel's timeline is right and
    /// a play/pause button press is sent as the right one.
    pub fn update(&self, playing: bool, position: f64, duration: f64) {
        if let Err(e) = self
            .controls
            .update(playing, position.max(0.0), duration.max(0.0))
        {
            eprintln!("[media] updating playback state: {e:#}");
        }
    }

    /// Removes the app from the panel; media keys go to other apps again.
    pub fn hide(&self) {
        self.shown.set(None);
        if let Err(e) = self.controls.hide() {
            eprintln!("[media] hiding: {e:#}");
        }
    }

    /// Commands received since the last call, oldest first.
    pub fn commands(&self) -> Vec<MediaCommand> {
        let skip = self.skip_buttons.get();
        self.commands
            .try_iter()
            .filter(|c| skip || !matches!(c, MediaCommand::Next | MediaCommand::Previous))
            .collect()
    }
}

/// How long to wait between timeline reports while playing normally. The
/// panel advances the position by itself in between.
pub const REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether the panel needs a new report: the state changed, the position
/// jumped (a seek), or it has been `REPORT_INTERVAL` since the last one.
/// `last` is the previous report: playing, position, seconds since then.
pub fn needs_report(
    last: Option<(bool, f64, f64)>,
    playing: bool,
    position: f64,
    speed: f64,
) -> bool {
    let Some((was_playing, was_at, elapsed)) = last else {
        return true;
    };
    if was_playing != playing || elapsed >= REPORT_INTERVAL.as_secs_f64() {
        return true;
    }
    let expected = if playing {
        was_at + elapsed * speed
    } else {
        was_at
    };
    (position - expected).abs() > 1.5
}

#[cfg(test)]
mod tests {
    use super::needs_report;

    #[test]
    fn first_report_is_always_sent() {
        assert!(needs_report(None, true, 0.0, 1.0));
    }

    #[test]
    fn steady_playback_waits_for_the_interval() {
        assert!(!needs_report(Some((true, 10.0, 1.0)), true, 11.0, 1.0));
        assert!(!needs_report(Some((true, 10.0, 1.0)), true, 12.0, 2.0));
        assert!(needs_report(Some((true, 10.0, 5.0)), true, 15.0, 1.0));
    }

    #[test]
    fn pausing_and_seeking_are_reported_at_once() {
        assert!(needs_report(Some((true, 10.0, 0.5)), false, 10.5, 1.0));
        assert!(needs_report(Some((true, 10.0, 0.5)), true, 40.0, 1.0));
        assert!(needs_report(Some((false, 10.0, 0.5)), false, 0.0, 1.0));
        assert!(!needs_report(Some((false, 10.0, 3.0)), false, 10.0, 1.0));
    }
}
