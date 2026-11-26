//! The player's short messages: toasts like "Volume 80%" near the top, and
//! the "Loading…" / "Reconnecting…" notice in the middle.

use std::time::{Duration, Instant};

use gpui::SharedString;

use super::fade::Notes;
use crate::motion;

/// How long a toast like "Volume 80%" stays before it fades out.
const TOAST_DURATION: Duration = Duration::from_millis(1200);

/// What is holding the video up, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Holdup {
    Nothing,
    /// Buffering or opening, while it should be playing.
    Loading,
    /// The connection is lost and the stream is being opened again.
    Reconnecting,
}

/// The notice for `holdup`, having loaded for `loading_for`: "Loading…" only
/// once the wait is longer than `patience`, so a quick seek never flashes it.
fn notice_text(holdup: Holdup, loading_for: Duration, patience: Duration) -> Option<&'static str> {
    match holdup {
        Holdup::Reconnecting => Some("Lost the connection to the server. Reconnecting…"),
        Holdup::Loading if loading_for >= patience => Some("Loading…"),
        Holdup::Loading | Holdup::Nothing => None,
    }
}

/// The toast and the notice, each fading over the one before.
pub(super) struct Messages {
    pub(super) toasts: Notes,
    /// When the current toast was shown.
    toast_at: Option<Instant>,
    pub(super) notice: Notes,
    /// Since when it's been loading.
    loading_since: Option<Instant>,
}

impl Messages {
    pub(super) fn new() -> Self {
        Self {
            toasts: Notes::default(),
            toast_at: None,
            notice: Notes::default(),
            loading_since: None,
        }
    }

    pub(super) fn toast(&mut self, message: SharedString) {
        self.toasts.show(Some(message));
        self.toast_at = Some(Instant::now());
    }

    /// Lets the toast go once it has been up long enough.
    pub(super) fn expire_toast(&mut self) {
        if self
            .toast_at
            .is_some_and(|at| at.elapsed() > TOAST_DURATION)
        {
            self.toast_at = None;
            self.toasts.show(None);
        }
    }

    /// Shows the notice that fits `holdup` (or lets it go).
    pub(super) fn update_notice(&mut self, holdup: Holdup) {
        if holdup != Holdup::Loading {
            self.loading_since = None;
        } else if self.loading_since.is_none() {
            self.loading_since = Some(Instant::now());
        }
        let loading_for = self
            .loading_since
            .map_or(Duration::ZERO, |since| since.elapsed());
        let notice = notice_text(holdup, loading_for, motion::WAIT.time());
        self.notice.show(notice.map(SharedString::from));
    }

    pub(super) fn moving(&self) -> bool {
        self.toasts.moving() || self.notice.moving()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATIENCE: Duration = Duration::from_millis(350);

    #[test]
    fn loading_is_only_said_after_a_moment() {
        let after = |ms| notice_text(Holdup::Loading, Duration::from_millis(ms), PATIENCE);
        assert_eq!(after(0), None);
        assert_eq!(after(349), None);
        assert_eq!(after(350), Some("Loading…"));
    }

    #[test]
    fn a_lost_connection_is_said_at_once() {
        let notice = notice_text(Holdup::Reconnecting, Duration::ZERO, PATIENCE);
        assert_eq!(
            notice,
            Some("Lost the connection to the server. Reconnecting…")
        );
        assert_eq!(notice_text(Holdup::Nothing, PATIENCE * 2, PATIENCE), None);
    }

    #[test]
    fn a_toast_replaces_the_one_before() {
        let mut messages = Messages::new();
        messages.toast("Volume 80%".into());
        messages.toast("Volume 85%".into());
        assert_eq!(
            messages.toasts.current().map(|text| text.as_ref()),
            Some("Volume 85%")
        );
        messages.expire_toast();
        assert!(messages.toasts.current().is_some());
    }
}
