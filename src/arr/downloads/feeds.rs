//! What each set-up app last said about its queue and history, and the
//! rules over that: what counts as in progress, when history is asked
//! again, which lines a page lists. No network here.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui::SharedString;

use crate::arr::connections::Id;
use crate::arr::{Download, Kind, Recent};

/// History is asked again this often, or when a download comes or goes.
const RECENT_EVERY: Duration = Duration::from_secs(60);
/// How many history lines a page lists.
const RECENT_SHOWN: usize = 15;

/// One app's queue and history, as last heard.
#[derive(Debug, Clone, Default)]
pub struct Feed {
    downloads: Vec<Download>,
    recent: Vec<Recent>,
    recent_at: Option<Instant>,
    error: Option<SharedString>,
    loaded: bool,
}

impl Feed {
    /// The downloads in the app's queue.
    pub fn downloads(&self) -> &[Download] {
        &self.downloads
    }

    /// What went wrong the last time the app was asked.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// The history lines the app's own page lists, as the app ordered them.
    pub fn recent_shown(&self) -> impl Iterator<Item = &Recent> {
        self.recent.iter().take(RECENT_SHOWN)
    }

    /// Takes an app's answer: its queue, and its history when that was asked.
    fn answered(&mut self, kind: Kind, answer: Answer, now: Instant) {
        self.loaded = true;
        self.error = None;
        match answer.downloads {
            Ok(downloads) => self.downloads = downloads,
            Err(e) => {
                eprintln!("[{}] queue: {e:#}", kind.name());
                self.error = Some(format!("Couldn't ask {}: {e:#}", kind.name()).into());
            }
        }
        match answer.recent {
            Some(Ok(recent)) => {
                self.recent = recent;
                self.recent_at = Some(now);
            }
            Some(Err(e)) => {
                eprintln!("[{}] history: {e:#}", kind.name());
                self.error = Some(format!("Couldn't read {}'s history: {e:#}", kind.name()).into());
            }
            None => {}
        }
    }
}

/// What an app said when asked: its queue, and its history if that was due.
#[derive(Debug)]
pub(super) struct Answer {
    pub(super) downloads: Result<Vec<Download>>,
    pub(super) recent: Option<Result<Vec<Recent>>>,
}

/// What an app's feed held before it was asked again.
#[derive(Debug)]
pub(super) struct Previous {
    ids: HashSet<String>,
    recent_at: Option<Instant>,
}

impl Previous {
    /// Whether the history should be asked too, the queue now holding `ids`.
    pub(super) fn history_is_stale(&self, ids: &HashSet<String>, now: Instant) -> bool {
        self.ids != *ids
            || self
                .recent_at
                .is_none_or(|at| now.duration_since(at) >= RECENT_EVERY)
    }
}

/// One app's feed, among a server's.
#[derive(Debug, Clone, Copy)]
pub struct AppFeed<'a> {
    pub kind: Kind,
    pub feed: &'a Feed,
}

/// A history line and the app it came from.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    pub kind: Kind,
    pub recent: &'a Recent,
}

/// What a "Downloading" list says when it has no download to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    /// No app has answered yet.
    Asking,
    /// The apps answered: nothing is downloading.
    Nothing,
}

/// What to say in place of the downloads of `feeds`, if they list none.
pub fn quiet(feeds: &[&Feed]) -> Option<Quiet> {
    if !feeds.iter().any(|feed| feed.loaded) {
        Some(Quiet::Asking)
    } else if feeds.iter().all(|feed| feed.downloads.is_empty()) {
        Some(Quiet::Nothing)
    } else {
        None
    }
}

/// The history lines a server's page lists: its apps' together, newest first.
pub fn latest<'a>(feeds: &[AppFeed<'a>]) -> Vec<Entry<'a>> {
    let mut entries: Vec<Entry<'a>> = feeds
        .iter()
        .flat_map(|app| {
            let kind = app.kind;
            app.feed
                .recent
                .iter()
                .map(move |recent| Entry { kind, recent })
        })
        .collect();
    entries.sort_by_key(|entry| Reverse(entry.recent.at));
    entries.truncate(RECENT_SHOWN);
    entries
}

/// Every set-up app's feed, and which are being asked right now.
#[derive(Debug, Clone, Default)]
pub struct Feeds {
    feeds: HashMap<Id, Feed>,
    loading: HashSet<Id>,
    fetched: Option<Instant>,
}

impl Feeds {
    /// Downloads in progress, for the sidebar.
    pub fn active(&self) -> usize {
        self.feeds.values().map(|feed| feed.downloads.len()).sum()
    }

    /// One app's downloads in progress.
    pub fn active_for(&self, id: Id) -> usize {
        self.feeds.get(&id).map_or(0, |feed| feed.downloads.len())
    }

    /// An app's feed, once it has answered.
    pub fn feed(&self, id: Id) -> Option<&Feed> {
        self.feeds.get(&id)
    }

    /// The feeds of a server's apps that have answered, in the order of `kinds`.
    pub fn of_server(&self, server: i64, kinds: &[Kind]) -> Vec<AppFeed<'_>> {
        kinds
            .iter()
            .filter_map(|&kind| {
                let feed = self.feeds.get(&Id { server, kind })?;
                Some(AppFeed { kind, feed })
            })
            .collect()
    }

    /// How long ago the apps were last asked.
    pub(super) fn since_fetched(&self, now: Instant) -> Option<Duration> {
        self.fetched.map(|at| now.duration_since(at))
    }

    /// A round of asking starts: only the feeds of `connected` apps stay.
    pub(super) fn start_round(&mut self, connected: &[Id], now: Instant) {
        self.feeds.retain(|id, _| connected.contains(id));
        self.fetched = Some(now);
    }

    /// Notes that `id` is being asked; false when it already is.
    pub(super) fn start_loading(&mut self, id: Id) -> bool {
        self.loading.insert(id)
    }

    /// What `id`'s feed holds, to tell whether its history is still current.
    pub(super) fn previous(&self, id: Id) -> Option<Previous> {
        self.feeds.get(&id).map(|feed| Previous {
            ids: feed.downloads.iter().map(|d| d.id.clone()).collect(),
            recent_at: feed.recent_at,
        })
    }

    /// Takes the answer of `id`, which is no longer being asked.
    pub(super) fn answered(&mut self, id: Id, answer: Answer, now: Instant) {
        self.loading.remove(&id);
        self.feeds
            .entry(id)
            .or_default()
            .answered(id.kind, answer, now);
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;
    use crate::arr::{DownloadState, Event, Subject};

    const SONARR: Id = Id {
        server: 1,
        kind: Kind::Sonarr,
    };
    const RADARR: Id = Id {
        server: 1,
        kind: Kind::Radarr,
    };

    fn download(id: &str) -> Download {
        Download {
            id: id.into(),
            subject: Subject::Unknown(id.into()),
            release: String::new(),
            quality: None,
            size: 0.0,
            sizeleft: 0.0,
            timeleft: None,
            state: DownloadState::Downloading,
            messages: Vec::new(),
            client: None,
        }
    }

    fn recent(id: &str, at: i64) -> Recent {
        Recent {
            id: id.into(),
            subject: Subject::Unknown(id.into()),
            event: Event::Imported,
            at,
            release: String::new(),
            quality: None,
        }
    }

    fn queue(ids: &[&str]) -> Answer {
        Answer {
            downloads: Ok(ids.iter().map(|id| download(id)).collect()),
            recent: None,
        }
    }

    fn history(lines: Vec<Recent>) -> Answer {
        Answer {
            downloads: Ok(Vec::new()),
            recent: Some(Ok(lines)),
        }
    }

    fn ids(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn downloads_are_counted_per_app_and_together() {
        let now = Instant::now();
        let mut feeds = Feeds::default();
        assert_eq!(feeds.active(), 0);
        feeds.answered(SONARR, queue(&["a", "b"]), now);
        feeds.answered(RADARR, queue(&["c"]), now);
        assert_eq!(feeds.active(), 3);
        assert_eq!(feeds.active_for(SONARR), 2);
        assert_eq!(feeds.active_for(RADARR), 1);
        let other = Id {
            server: 2,
            kind: Kind::Sonarr,
        };
        assert_eq!(feeds.active_for(other), 0);
    }

    #[test]
    fn a_round_drops_the_feeds_of_apps_no_longer_set_up() {
        let now = Instant::now();
        let mut feeds = Feeds::default();
        feeds.answered(SONARR, queue(&["a"]), now);
        feeds.answered(RADARR, queue(&["c"]), now);
        assert_eq!(feeds.since_fetched(now), None);
        feeds.start_round(&[RADARR], now);
        assert!(feeds.feed(SONARR).is_none());
        assert_eq!(feeds.active(), 1);
        let later = now + Duration::from_secs(7);
        assert_eq!(feeds.since_fetched(later), Some(Duration::from_secs(7)));
    }

    #[test]
    fn an_app_is_asked_once_at_a_time() {
        let mut feeds = Feeds::default();
        assert!(feeds.start_loading(SONARR));
        assert!(!feeds.start_loading(SONARR));
        assert!(feeds.start_loading(RADARR));
        feeds.answered(SONARR, queue(&[]), Instant::now());
        assert!(feeds.start_loading(SONARR));
    }

    #[test]
    fn a_failed_answer_keeps_the_downloads_and_says_why() {
        let now = Instant::now();
        let mut feeds = Feeds::default();
        feeds.answered(SONARR, queue(&["a"]), now);
        let failed = Answer {
            downloads: Err(anyhow!("timed out")),
            recent: None,
        };
        feeds.answered(SONARR, failed, now);
        let feed = feeds.feed(SONARR).expect("the feed stays");
        assert_eq!(feed.downloads().len(), 1);
        let error = feed.error().expect("an error to show");
        assert_eq!(error.as_ref(), "Couldn't ask Sonarr: timed out");
        feeds.answered(SONARR, queue(&["a"]), now);
        assert!(
            feeds
                .feed(SONARR)
                .expect("the feed stays")
                .error()
                .is_none()
        );
    }

    #[test]
    fn a_failed_history_keeps_the_lines_and_says_why() {
        let now = Instant::now();
        let mut feeds = Feeds::default();
        feeds.answered(RADARR, history(vec![recent("x", 10)]), now);
        let failed = Answer {
            downloads: Ok(Vec::new()),
            recent: Some(Err(anyhow!("no route"))),
        };
        feeds.answered(RADARR, failed, now);
        let feed = feeds.feed(RADARR).expect("the feed stays");
        assert_eq!(feed.recent_shown().count(), 1);
        let error = feed.error().expect("an error to show");
        assert_eq!(error.as_ref(), "Couldn't read Radarr's history: no route");
    }

    #[test]
    fn history_is_asked_when_the_queue_changes_or_a_minute_passed() {
        let now = Instant::now();
        let mut feeds = Feeds::default();
        assert!(feeds.previous(SONARR).is_none());
        // A queue alone: the history was never read.
        feeds.answered(SONARR, queue(&["a"]), now);
        let unread = feeds.previous(SONARR).expect("a feed");
        assert!(unread.history_is_stale(&ids(&["a"]), now));
        let both = Answer {
            downloads: Ok(vec![download("a")]),
            recent: Some(Ok(Vec::new())),
        };
        feeds.answered(SONARR, both, now);
        let previous = feeds.previous(SONARR).expect("a feed");
        let soon = now + Duration::from_secs(59);
        assert!(!previous.history_is_stale(&ids(&["a"]), soon));
        assert!(previous.history_is_stale(&ids(&["a", "b"]), soon));
        assert!(previous.history_is_stale(&ids(&[]), soon));
        assert!(previous.history_is_stale(&ids(&["a"]), now + RECENT_EVERY));
    }

    #[test]
    fn an_empty_list_says_asking_until_an_app_answers() {
        let now = Instant::now();
        let mut feeds = Feeds::default();
        assert_eq!(quiet(&[]), Some(Quiet::Asking));
        feeds.answered(SONARR, queue(&[]), now);
        feeds.answered(RADARR, queue(&[]), now);
        let sonarr = feeds.feed(SONARR).expect("a feed");
        let radarr = feeds.feed(RADARR).expect("a feed");
        assert_eq!(quiet(&[sonarr]), Some(Quiet::Nothing));
        assert_eq!(quiet(&[sonarr, radarr]), Some(Quiet::Nothing));
        feeds.answered(RADARR, queue(&["c"]), now);
        let sonarr = feeds.feed(SONARR).expect("a feed");
        let radarr = feeds.feed(RADARR).expect("a feed");
        assert_eq!(quiet(&[sonarr, radarr]), None);
        assert_eq!(quiet(&[sonarr]), Some(Quiet::Nothing));
    }

    #[test]
    fn a_servers_history_is_its_apps_together_newest_first() {
        let now = Instant::now();
        let mut feeds = Feeds::default();
        feeds.answered(
            SONARR,
            history(vec![recent("s3", 30), recent("s1", 10)]),
            now,
        );
        feeds.answered(RADARR, history(vec![recent("r2", 20)]), now);
        let kinds = [Kind::Sonarr, Kind::Radarr];
        let server = feeds.of_server(1, &kinds);
        assert_eq!(server.len(), 2);
        let shown: Vec<(Kind, &str)> = latest(&server)
            .iter()
            .map(|entry| (entry.kind, entry.recent.id.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                (Kind::Sonarr, "s3"),
                (Kind::Radarr, "r2"),
                (Kind::Sonarr, "s1")
            ]
        );
        assert!(feeds.of_server(2, &kinds).is_empty());
    }

    #[test]
    fn a_page_lists_fifteen_history_lines_at_most() {
        let now = Instant::now();
        let mut feeds = Feeds::default();
        let lines = |prefix: &str| -> Vec<Recent> {
            (0..12)
                .map(|n| recent(&format!("{prefix}{n}"), n))
                .collect()
        };
        feeds.answered(SONARR, history(lines("s")), now);
        feeds.answered(RADARR, history(lines("r")), now);
        let server = feeds.of_server(1, &[Kind::Sonarr, Kind::Radarr]);
        assert_eq!(latest(&server).len(), RECENT_SHOWN);
        feeds.answered(
            SONARR,
            history((0..40).map(|n| recent("s", n)).collect()),
            now,
        );
        let feed = feeds.feed(SONARR).expect("a feed");
        assert_eq!(feed.recent_shown().count(), RECENT_SHOWN);
        // As the app ordered them, not by time.
        assert_eq!(feed.recent_shown().next().map(|recent| recent.at), Some(0));
    }
}
