//! What a season's row says of Sonarr: its state in words, their colour,
//! and the button that fits. Also how long a Download counts as searching.
//! Plain rules, apart from asking Sonarr and from drawing.

use std::time::{Duration, Instant};

use gpui::SharedString;

use crate::arr::{self, SeasonSummary};
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
    let missing = missing_text(season);
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
            _ => {
                let (text, button) = settled_status(season, missing);
                (text, MUTED, button)
            }
        }
    };
    SeasonStatus {
        text,
        color,
        button,
    }
}

/// "8 episodes not downloaded", "3 of 8 episodes missing".
fn missing_text(season: &SeasonSummary) -> String {
    if season.downloaded == 0 {
        format!("{} not downloaded", episodes(season.missing))
    } else {
        format!("{} of {} missing", season.missing, episodes(season.aired))
    }
}

/// What a season says when nothing is downloading or was just asked for.
fn settled_status(season: &SeasonSummary, missing: String) -> (String, Option<&'static str>) {
    let next_airs = season.next_airs.map(arr::format_date);
    if season.aired == 0 {
        let not_aired = next_airs.map_or_else(
            || "Not aired yet".to_string(),
            |date| format!("Airs {date}"),
        );
        return if season.monitored {
            (
                format!("{not_aired} · Sonarr will get it when it airs"),
                None,
            )
        } else {
            (not_aired, Some("Download"))
        };
    }
    if !season.is_airing() {
        return match (season.missing, season.monitored) {
            (0, _) => ("Downloaded".to_string(), None),
            (_, true) => (
                format!("{missing} · Sonarr is watching for releases"),
                Some("Search Again"),
            ),
            (_, false) => (missing, Some("Download")),
        };
    }
    airing_status(season, missing, next_airs)
}

/// A season with episodes still to air has no whole-season release yet,
/// so it says why its episodes come one at a time.
fn airing_status(
    season: &SeasonSummary,
    missing: String,
    next_airs: Option<String>,
) -> (String, Option<&'static str>) {
    let next = next_airs
        .map(|date| format!(" · next airs {date}"))
        .unwrap_or_default();
    let out = format!(
        "{} of {} out, all downloaded{next}",
        season.aired,
        season.total()
    );
    match (season.missing, season.monitored) {
        (0, true) => (
            format!("{out} · Sonarr will get the rest as they air"),
            None,
        ),
        (0, false) => (
            format!("{out} · Sonarr isn't watching for the rest"),
            Some("Get the Rest"),
        ),
        (_, monitored) => (
            format!("{missing} · still airing, so releases come one episode at a time{next}"),
            Some(if monitored {
                "Search Again"
            } else {
                "Download"
            }),
        ),
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
            next_airs: None,
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

    /// 6 of 10 episodes aired, `downloaded` of them on disk, the next one
    /// airing on 28 Dec 2020.
    fn airing(downloaded: usize, monitored: bool) -> SeasonSummary {
        SeasonSummary {
            upcoming: 4,
            next_airs: Some(1_609_120_800),
            episode_ids: (1..=10).collect(),
            monitored,
            ..season(6, downloaded)
        }
    }

    #[test]
    fn a_season_still_airing_says_why_it_has_no_whole_season_release() {
        let now = Instant::now();
        let status = season_status(&airing(6, true), None, now);
        assert_eq!(
            status.text,
            "6 of 10 out, all downloaded · next airs 28 Dec 2020 · Sonarr will get the rest as they air"
        );
        assert_eq!(status.button, None);
        let status = season_status(&airing(6, false), None, now);
        assert_eq!(
            status.text,
            "6 of 10 out, all downloaded · next airs 28 Dec 2020 · Sonarr isn't watching for the rest"
        );
        assert_eq!(status.button, Some("Get the Rest"));
        let status = season_status(&airing(4, true), None, now);
        assert_eq!(
            status.text,
            "2 of 6 episodes missing · still airing, so releases come one episode at a time · next airs 28 Dec 2020"
        );
        assert_eq!(status.button, Some("Search Again"));
        assert_eq!(
            season_status(&airing(4, false), None, now).button,
            Some("Download")
        );
    }

    #[test]
    fn a_season_not_aired_says_when_it_airs() {
        let announced = SeasonSummary {
            upcoming: 8,
            next_airs: Some(1_609_120_800),
            monitored: true,
            ..season(0, 0)
        };
        let status = season_status(&announced, None, Instant::now());
        assert_eq!(
            status.text,
            "Airs 28 Dec 2020 · Sonarr will get it when it airs"
        );
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
