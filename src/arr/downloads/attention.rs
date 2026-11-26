//! How closely the downloads are being followed, which sets how often the
//! apps are asked: every few seconds under someone's eyes, now and then for
//! the sidebar's count, not at all during a video.

use std::time::Duration;

/// How often the queue is asked while a page shows it.
pub(super) const LOOKING: Duration = Duration::from_secs(5);
/// And while none does.
const AWAY: Duration = Duration::from_secs(60);
/// A round this close to due is asked now, as the next look comes `LOOKING` later.
const SLACK: Duration = Duration::from_millis(200);

/// Who is following the downloads right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Attention {
    /// A page listing downloads is open.
    Looking,
    /// Only the sidebar's count shows.
    #[default]
    Away,
    /// A video is playing: nothing is asked.
    Watching,
}

impl Attention {
    /// How often the apps are asked; None for not at all.
    fn interval(self) -> Option<Duration> {
        match self {
            Attention::Looking => Some(LOOKING),
            Attention::Away => Some(AWAY),
            Attention::Watching => None,
        }
    }
}

/// Whether the apps should be asked now, last asked `since_fetched` ago.
pub(super) fn is_due(attention: Attention, since_fetched: Option<Duration>) -> bool {
    let Some(every) = attention.interval() else {
        return false;
    };
    since_fetched.is_none_or(|elapsed| elapsed + SLACK >= every)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_page_asks_at_every_look() {
        assert!(is_due(Attention::Looking, None));
        assert!(is_due(Attention::Looking, Some(Duration::from_secs(5))));
        // The timer can come a little early.
        assert!(is_due(
            Attention::Looking,
            Some(Duration::from_millis(4850))
        ));
        assert!(!is_due(Attention::Looking, Some(Duration::from_secs(2))));
    }

    #[test]
    fn the_sidebar_alone_asks_once_a_minute() {
        assert!(is_due(Attention::Away, None));
        assert!(!is_due(Attention::Away, Some(Duration::from_secs(5))));
        assert!(!is_due(Attention::Away, Some(Duration::from_secs(55))));
        assert!(is_due(Attention::Away, Some(Duration::from_millis(59_900))));
    }

    #[test]
    fn nothing_is_asked_during_a_video() {
        assert!(!is_due(Attention::Watching, None));
        assert!(!is_due(Attention::Watching, Some(Duration::from_secs(600))));
    }
}
