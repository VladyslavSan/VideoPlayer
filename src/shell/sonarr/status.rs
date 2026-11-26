//! What a season's row says of Sonarr: its state in words, their colour,
//! and the button that fits. Also how long a Download counts as searching.
//! Plain rules, apart from asking Sonarr and from drawing.

use std::time::{Duration, Instant};

use gpui::SharedString;

use crate::arr::SeasonSummary;
use crate::shell::{ERROR, MUTED, TEXT};

/// A Download asked for this recently still counts as searching.
const SEARCHING_FOR: Duration = Duration::from_secs(15 * 60);

/// A Download (or a chosen release) asked for on a season.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Request {
    Sending,
    /// Asked Sonarr to search for the season.
    Sent(Instant),
    /// Sent a chosen release to the download client.
    Grabbed(Instant),
    Failed(SharedString),
}

impl Request {
    /// Whether Sonarr was asked so recently (as of `now`) that it may
    /// still be searching or fetching.
    pub(super) fn is_recent(&self, now: Instant) -> bool {
        match self {
            Request::Sent(at) | Request::Grabbed(at) => {
                now.saturating_duration_since(*at) < SEARCHING_FOR
            }
            Request::Sending | Request::Failed(_) => false,
        }
    }
}

/// What a season's row says, and the button under it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SeasonStatus {
    pub text: String,
    pub color: u32,
    /// The Download button's label, when there is something to ask for.
    pub button: Option<&'static str>,
}

fn episodes(count: usize) -> String {
    if count == 1 {
        "1 episode".to_string()
    } else {
        format!("{count} episodes")
    }
}

/// The season's state as Sonarr has it, with what was asked here.
pub(super) fn season_status(
    season: &SeasonSummary,
    request: Option<&Request>,
    now: Instant,
) -> SeasonStatus {
    let missing = if season.downloaded == 0 {
        format!("{} not downloaded", episodes(season.missing))
    } else {
        format!("{} of {} missing", season.missing, episodes(season.aired))
    };
    let (text, color, button) = if season.downloading > 0 {
        let progress = season
            .progress
            .map(|p| format!(" · {:.0}%", p * 100.0))
            .unwrap_or_default();
        (
            format!("Downloading {}{progress}", episodes(season.downloading)),
            TEXT,
            None,
        )
    } else {
        match request {
            Some(Request::Sending) => ("Asking Sonarr…".to_string(), MUTED, None),
            Some(Request::Failed(e)) => (e.to_string(), ERROR, Some("Try Again")),
            Some(request @ Request::Sent(_)) if request.is_recent(now) && season.missing > 0 => (
                format!("{missing} · Sonarr is searching for it"),
                MUTED,
                None,
            ),
            Some(request @ Request::Grabbed(_)) if request.is_recent(now) && season.missing > 0 => {
                (
                    format!("{missing} · sent to the download client"),
                    MUTED,
                    None,
                )
            }
            _ if season.aired == 0 && season.monitored => (
                "Not aired yet · Sonarr will get it when it airs".to_string(),
                MUTED,
                None,
            ),
            _ if season.aired == 0 => ("Not aired yet".to_string(), MUTED, Some("Download")),
            _ if season.missing == 0 => ("Downloaded".to_string(), MUTED, None),
            _ if season.monitored => (
                format!("{missing} · Sonarr is watching for releases"),
                MUTED,
                Some("Search Again"),
            ),
            _ => (missing, MUTED, Some("Download")),
        }
    };
    SeasonStatus {
        text,
        color,
        button,
    }
}

/// Whether a show is worth asking about again soon: something downloads,
/// or one of the `requests` made on it was made recently.
pub(super) fn is_busy<'a>(
    seasons: &[SeasonSummary],
    mut requests: impl Iterator<Item = &'a Request>,
    now: Instant,
) -> bool {
    seasons.iter().any(|season| season.downloading > 0)
        || requests.any(|request| request.is_recent(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn season(aired: usize, downloaded: usize) -> SeasonSummary {
        SeasonSummary {
            number: 1,
            aired,
            downloaded,
            missing: aired - downloaded,
            upcoming: 0,
            episode_ids: Vec::new(),
            monitored: false,
            downloading: 0,
            progress: None,
        }
    }

    fn minutes(count: u64) -> Duration {
        Duration::from_secs(count * 60)
    }

    #[test]
    fn a_download_counts_as_searching_for_fifteen_minutes() {
        let asked = Instant::now();
        for request in [Request::Sent(asked), Request::Grabbed(asked)] {
            assert!(request.is_recent(asked));
            assert!(request.is_recent(asked + minutes(14)));
            assert!(!request.is_recent(asked + minutes(15)));
        }
        assert!(!Request::Sending.is_recent(asked));
        assert!(!Request::Failed("no".into()).is_recent(asked));
    }

    #[test]
    fn a_missing_season_offers_a_download() {
        let now = Instant::now();
        let status = season_status(&season(8, 0), None, now);
        assert_eq!(status.text, "8 episodes not downloaded");
        assert_eq!(status.button, Some("Download"));
        let status = season_status(&season(8, 5), None, now);
        assert_eq!(status.text, "3 of 8 episodes missing");
        let monitored = SeasonSummary {
            monitored: true,
            ..season(8, 5)
        };
        let status = season_status(&monitored, None, now);
        assert_eq!(
            status.text,
            "3 of 8 episodes missing · Sonarr is watching for releases"
        );
        assert_eq!(status.button, Some("Search Again"));
    }

    #[test]
    fn a_season_being_downloaded_says_how_far_along() {
        let downloading = SeasonSummary {
            downloading: 2,
            progress: Some(0.4),
            ..season(8, 5)
        };
        let failed = Request::Failed("Sonarr said no".into());
        let status = season_status(&downloading, Some(&failed), Instant::now());
        assert_eq!(status.text, "Downloading 2 episodes · 40%");
        assert_eq!((status.color, status.button), (TEXT, None));
    }

    #[test]
    fn a_request_shows_until_it_is_old_or_the_season_is_complete() {
        let asked = Instant::now();
        let sent = Request::Sent(asked);
        let status = season_status(&season(8, 0), Some(&sent), asked + minutes(1));
        assert_eq!(
            status.text,
            "8 episodes not downloaded · Sonarr is searching for it"
        );
        assert_eq!(status.button, None);
        let grabbed = Request::Grabbed(asked);
        let status = season_status(&season(8, 0), Some(&grabbed), asked + minutes(1));
        assert_eq!(
            status.text,
            "8 episodes not downloaded · sent to the download client"
        );
        // Old: the season is as if nothing was asked.
        let status = season_status(&season(8, 0), Some(&sent), asked + minutes(20));
        assert_eq!(status.button, Some("Download"));
        // Complete: nothing is being searched for.
        let status = season_status(&season(8, 8), Some(&sent), asked + minutes(1));
        assert_eq!(status.text, "Downloaded");
    }

    #[test]
    fn a_failed_or_pending_request_says_so() {
        let now = Instant::now();
        let status = season_status(&season(8, 0), Some(&Request::Sending), now);
        assert_eq!(
            (status.text.as_str(), status.button),
            ("Asking Sonarr…", None)
        );
        let failed = Request::Failed("Sonarr said no: timeout".into());
        let status = season_status(&season(8, 0), Some(&failed), now);
        assert_eq!(status.text, "Sonarr said no: timeout");
        assert_eq!((status.color, status.button), (ERROR, Some("Try Again")));
    }

    #[test]
    fn a_season_not_aired_waits_or_offers_a_download() {
        let now = Instant::now();
        let status = season_status(&season(0, 0), None, now);
        assert_eq!(
            (status.text.as_str(), status.button),
            ("Not aired yet", Some("Download"))
        );
        let monitored = SeasonSummary {
            monitored: true,
            ..season(0, 0)
        };
        let status = season_status(&monitored, None, now);
        assert_eq!(
            status.text,
            "Not aired yet · Sonarr will get it when it airs"
        );
        assert_eq!(status.button, None);
    }

    #[test]
    fn a_show_is_busy_while_it_downloads_or_was_just_asked_for() {
        let asked = Instant::now();
        let quiet = [season(8, 5)];
        assert!(!is_busy(&quiet, [].iter(), asked));
        let downloading = [SeasonSummary {
            downloading: 1,
            ..season(8, 5)
        }];
        assert!(is_busy(&downloading, [].iter(), asked));
        let requests = [Request::Failed("no".into()), Request::Sent(asked)];
        assert!(is_busy(&quiet, requests.iter(), asked + minutes(5)));
        assert!(!is_busy(&quiet, requests.iter(), asked + minutes(16)));
    }
}
