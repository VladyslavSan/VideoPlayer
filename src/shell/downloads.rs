//! The Downloads page: what Sonarr and Radarr are downloading for each
//! Jellyfin server, how far along, what's stuck and why, and what they
//! recently added. Asked again every few seconds while the page is open,
//! and now and then otherwise (for the count in the sidebar).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, SharedString, div, prelude::*, px, relative, rgb,
    rgba,
};

use super::{ACCENT, ERROR, MUTED, Screen, Shell, TEXT};
use crate::arr::{self, Download, DownloadState, Event, Kind, Recent, Subject};
use crate::library::Source;

/// How often the queue is asked while the page is open.
const LOOKING: Duration = Duration::from_secs(5);
/// And while it isn't.
const AWAY: Duration = Duration::from_secs(60);
/// History is asked again this often, or when a download comes or goes.
const RECENT_EVERY: Duration = Duration::from_secs(60);
const RECENT_LIMIT: usize = 20;
const RECENT_SHOWN: usize = 15;
const ADDED: u32 = 0x7ad89a;

use super::finder::AppKey;

#[derive(Default)]
pub(super) struct Feed {
    downloads: Vec<Download>,
    recent: Vec<Recent>,
    recent_at: Option<Instant>,
    error: Option<SharedString>,
    loaded: bool,
}

#[derive(Default)]
pub(super) struct Downloads {
    feeds: HashMap<AppKey, Feed>,
    loading: HashSet<AppKey>,
    fetched: Option<Instant>,
    polling: bool,
}

impl Downloads {
    /// Downloads in progress, for the sidebar.
    pub(super) fn active(&self) -> usize {
        self.feeds.values().map(|f| f.downloads.len()).sum()
    }

    /// One app's downloads in progress.
    pub(super) fn active_for(&self, key: AppKey) -> usize {
        self.feeds.get(&key).map_or(0, |f| f.downloads.len())
    }
}

/// A title, as compared: letters and digits only, whole and without a
/// trailing `(…)`.
struct Title {
    whole: String,
    bare: String,
}

impl Title {
    fn new(text: &str) -> Self {
        Self {
            whole: norm(text),
            bare: norm(bare(text)),
        }
    }

    /// 2 for the same title, 1 for the same but for a trailing `(…)`.
    fn score(&self, other: &Title) -> u8 {
        if self.whole == other.whole {
            2
        } else if self.bare == other.bare {
            1
        } else {
            0
        }
    }
}

/// The library's show and movie titles by server, for finding what a
/// download is of: made once per change to the library, not for every
/// line on every frame (which made the Downloads page scroll slowly).
pub(super) struct TitleIndex {
    /// What it was made from: the item lists' lengths and addresses.
    from: [usize; 4],
    shows: HashMap<i64, Vec<(Title, String)>>,
    /// Title, year and item key.
    movies: HashMap<i64, Vec<(Title, Option<u16>, String)>>,
}

impl TitleIndex {
    fn new<'a>(from: [usize; 4], items: impl Iterator<Item = &'a crate::library::Item>) -> Self {
        let mut shows: HashMap<i64, Vec<(Title, String)>> = HashMap::new();
        let mut seen: HashSet<(i64, &str)> = HashSet::new();
        let mut movies: HashMap<i64, Vec<(Title, Option<u16>, String)>> = HashMap::new();
        for item in items {
            let Source::Jellyfin { server, .. } = item.source else {
                continue;
            };
            match item.show.as_deref() {
                Some(show) => {
                    if seen.insert((server, show)) {
                        shows
                            .entry(server)
                            .or_default()
                            .push((Title::new(show), show.to_string()));
                    }
                }
                None => movies.entry(server).or_default().push((
                    Title::new(&item.title),
                    item.year,
                    item.key(),
                )),
            }
        }
        Self {
            from,
            shows,
            movies,
        }
    }

    fn target(&self, server: i64, subject: &Subject) -> Option<Target> {
        match subject {
            Subject::Episodes { show, episodes, .. } => {
                let wanted = Title::new(show);
                let name = self
                    .shows
                    .get(&server)?
                    .iter()
                    .map(|(title, name)| (title.score(&wanted), name))
                    .filter(|(score, _)| *score > 0)
                    .max_by_key(|(score, _)| *score)?
                    .1
                    .clone();
                let first = episodes.first().map(|(s, ..)| *s);
                let season = first.filter(|s| episodes.iter().all(|(n, ..)| n == s));
                Some(Target::Show(name, season))
            }
            Subject::Movie { title, year } => {
                let wanted = Title::new(title);
                self.movies
                    .get(&server)?
                    .iter()
                    .filter(|(_, y, _)| match (y, year) {
                        (Some(a), Some(b)) => i32::from(*a) == *b,
                        _ => true,
                    })
                    .map(|(t, _, key)| (t.score(&wanted), key))
                    .filter(|(score, _)| *score > 0)
                    .max_by_key(|(score, _)| *score)
                    .map(|(_, key)| Target::Movie(key.clone()))
            }
            Subject::Unknown(_) => None,
        }
    }
}

/// Where a line on the page leads in the library.
pub(super) enum Target {
    Show(String, Option<u32>),
    Movie(String),
}

/// Letters and digits only, lower case: `Shameless (US)` → `shamelessus`.
fn norm(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Without a trailing `(US)` or `(2011)`.
fn bare(text: &str) -> &str {
    let text = text.trim();
    match text.rfind(" (") {
        Some(at) if text.ends_with(')') => text[..at].trim(),
        _ => text,
    }
}

/// `3 min ago`, `5 h ago`, `yesterday`.
fn ago(at: i64, now: i64) -> String {
    let secs = (now - at).max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..172_800 => "yesterday".into(),
        _ if secs < 30 * 86_400 => format!("{} days ago", secs / 86_400),
        _ => arr::format_date(at),
    }
}

/// `12 min left`, `2 h 5 min left`.
fn left(secs: i64) -> String {
    match secs {
        ..60 => "less than a minute left".into(),
        60..3600 => format!("{} min left", secs / 60),
        3600..86_400 => format!("{} h {} min left", secs / 3600, secs % 3600 / 60),
        _ => format!("{} days left", secs / 86_400),
    }
}

/// `12.3 GB`, `700 MB`.
fn size(bytes: f64) -> String {
    if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else {
        format!("{:.0} MB", bytes / 1e6)
    }
}

impl Shell {
    /// Keeps the downloads current from now on, once any app is set up.
    pub(super) fn start_downloads_poll(&mut self, cx: &mut Context<Self>) {
        if self.downloads.polling || self.arr_apps().is_empty() {
            return;
        }
        self.downloads.polling = true;
        self.refresh_downloads(cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(LOOKING).await;
                let alive = this.update(cx, |this, cx| {
                    let every = if matches!(this.screen, Screen::Downloads | Screen::Arr(..)) {
                        LOOKING
                    } else {
                        AWAY
                    };
                    let due = this
                        .downloads
                        .fetched
                        .is_none_or(|at| at.elapsed() + Duration::from_millis(200) >= every);
                    // Not while watching: no reason to.
                    if due && this.player.is_none() {
                        this.refresh_downloads(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Asks every set-up app for its queue (and, now and then, history).
    pub(super) fn refresh_downloads(&mut self, cx: &mut Context<Self>) {
        let apps = self.arr_apps();
        self.downloads
            .feeds
            .retain(|key, _| apps.iter().any(|(s, k, _)| (*s, *k) == *key));
        self.downloads.fetched = Some(Instant::now());
        for (server, kind, client) in apps {
            let key = (server, kind);
            if !self.downloads.loading.insert(key) {
                continue;
            }
            let previous = self.downloads.feeds.get(&key).map(|feed| {
                let ids: HashSet<String> = feed.downloads.iter().map(|d| d.id.clone()).collect();
                (ids, feed.recent_at)
            });
            cx.spawn(async move |this, cx| {
                let (downloads, recent) = cx
                    .background_executor()
                    .spawn(async move {
                        let downloads = client.downloads()?;
                        let ids: HashSet<String> = downloads.iter().map(|d| d.id.clone()).collect();
                        let stale = previous.is_none_or(|(old, at)| {
                            old != ids || at.is_none_or(|at| at.elapsed() >= RECENT_EVERY)
                        });
                        let recent = stale.then(|| client.recent(&ids, RECENT_LIMIT));
                        anyhow::Ok((downloads, recent))
                    })
                    .await
                    .map_or_else(|e| (Err(e), None), |(d, r)| (Ok(d), r));
                this.update(cx, |this, cx| {
                    this.downloads.loading.remove(&key);
                    let feed = this.downloads.feeds.entry(key).or_default();
                    feed.loaded = true;
                    feed.error = None;
                    match downloads {
                        Ok(downloads) => feed.downloads = downloads,
                        Err(e) => {
                            eprintln!("[{}] queue: {e:#}", kind.name());
                            feed.error =
                                Some(format!("Couldn't ask {}: {e:#}", kind.name()).into());
                        }
                    }
                    match recent {
                        Some(Ok(recent)) => {
                            feed.recent = recent;
                            feed.recent_at = Some(Instant::now());
                        }
                        Some(Err(e)) => {
                            eprintln!("[{}] history: {e:#}", kind.name());
                            feed.error = Some(
                                format!("Couldn't read {}'s history: {e:#}", kind.name()).into(),
                            );
                        }
                        None => {}
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The show or movie in the server's library a line is about.
    pub(super) fn library_target(&self, server: i64, subject: &Subject) -> Option<Target> {
        let from = [
            self.local.len(),
            self.remote.len(),
            self.local.as_ptr() as usize,
            self.remote.as_ptr() as usize,
        ];
        let index = {
            let mut cached = self.title_index.borrow_mut();
            match &*cached {
                Some(index) if index.from == from => index.clone(),
                _ => {
                    let index = std::rc::Rc::new(TitleIndex::new(from, self.items()));
                    *cached = Some(index.clone());
                    index
                }
            }
        };
        index.target(server, subject)
    }

    pub(super) fn open_target(&mut self, target: &Target, cx: &mut Context<Self>) {
        match target {
            Target::Show(name, season) => {
                self.open_show(name.clone(), cx);
                if let Some(season) = season {
                    self.open_seasons
                        .entry(name.clone())
                        .or_default()
                        .insert(*season as u16);
                }
            }
            Target::Movie(key) => self.open_details(key.clone(), cx),
        }
    }

    /// A line's frame: clickable when it leads somewhere in the library.
    fn activity_row(
        &self,
        id: String,
        target: Option<Target>,
        body: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_col()
            .gap_1p5()
            .px_4()
            .py_3()
            .rounded_lg()
            .bg(rgba(0xffffff08))
            .child(body)
            .when_some(target, |row, target| {
                row.cursor_pointer()
                    .hover(|s| s.bg(rgba(0xffffff12)))
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.open_target(&target, cx)
                        }),
                    )
            })
            .into_any_element()
    }

    /// Title and detail on one line: "Shameless (US)  Season 11 · 12 episodes".
    fn subject_line(subject: &Subject) -> gpui::Div {
        let (title, detail) = subject.label();
        div()
            .flex()
            .items_baseline()
            .gap_2()
            .min_w(px(0.0))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .children(detail.map(|d| {
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(d)
            }))
    }

    fn download_row(
        &self,
        server: i64,
        kind: Kind,
        download: &Download,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = download.state;
        let color = if state.is_problem() {
            ERROR
        } else if state == DownloadState::Downloading {
            TEXT
        } else {
            MUTED
        };
        let mut facts: Vec<String> = Vec::new();
        facts.extend(download.quality.clone());
        if download.size > 0.0 {
            let done = (download.size - download.sizeleft).max(0.0);
            facts.push(if download.sizeleft > 0.0 && done > 0.0 {
                format!("{} of {}", size(done), size(download.size))
            } else {
                size(download.size)
            });
        }
        if state == DownloadState::Downloading {
            facts.extend(download.timeleft.filter(|s| *s > 0).map(left));
        }
        facts.extend(download.client.clone());
        let progress = download.progress().unwrap_or(0.0);
        let bar = div()
            .h(px(4.0))
            .w_full()
            .rounded_full()
            .bg(rgba(0xffffff14))
            .child(
                div()
                    .h_full()
                    .w(relative(progress))
                    .rounded_full()
                    .bg(rgb(if state.is_problem() { ERROR } else { ACCENT })),
            );
        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(Self::subject_line(&download.subject).flex_1())
                    .child(div().flex_none().text_sm().text_color(rgb(color)).child(
                        match download.progress() {
                            Some(p) if state == DownloadState::Downloading => {
                                format!("{} · {:.0}%", state.label(), p * 100.0)
                            }
                            _ => state.label().to_string(),
                        },
                    )),
            )
            .child(bar)
            // In a row: truncated text in a column is measured as nothing.
            .child(
                div().flex().child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .truncate()
                        .child(download.release.clone()),
                ),
            )
            .when(!facts.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(facts.join(" · ")),
                )
            })
            .when(!download.messages.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(ERROR))
                        .child(download.messages.join(" · ")),
                )
            });
        self.activity_row(
            format!("download-{server}-{}-{}", kind.name(), download.id),
            self.library_target(server, &download.subject),
            body,
            cx,
        )
    }

    fn recent_row(
        &self,
        server: i64,
        kind: Kind,
        recent: &Recent,
        now: i64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = match recent.event {
            Event::Imported => ADDED,
            Event::Failed => ERROR,
            Event::Grabbed => MUTED,
        };
        let mut facts: Vec<String> = Vec::new();
        facts.extend(recent.quality.clone());
        facts.push(ago(recent.at, now));
        let body = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_none()
                    .w(px(64.0))
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(color))
                    .child(recent.event.label()),
            )
            .child(Self::subject_line(&recent.subject).flex_1())
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(facts.join(" · ")),
            );
        self.activity_row(
            format!("recent-{server}-{}-{}", kind.name(), recent.id),
            self.library_target(server, &recent.subject),
            body,
            cx,
        )
    }

    /// One app's Downloading and Recently, for its page.
    pub(super) fn app_activity(&self, key: AppKey, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (server, kind) = key;
        let note = |text: String, color: u32| {
            div()
                .text_sm()
                .text_color(rgb(color))
                .child(text)
                .into_any_element()
        };
        let feed = self.downloads.feeds.get(&key);
        let mut downloading: Vec<AnyElement> = feed
            .and_then(|f| f.error.as_ref())
            .map(|e| note(e.to_string(), ERROR))
            .into_iter()
            .collect();
        match feed {
            None => downloading.push(note(format!("Asking {}…", kind.name()), MUTED)),
            Some(feed) if feed.loaded && feed.downloads.is_empty() => {
                downloading.push(note("Nothing is downloading right now.".into(), MUTED));
            }
            Some(feed) => {
                for download in &feed.downloads {
                    downloading.push(self.download_row(server, kind, download, cx));
                }
            }
        }
        let mut sections = vec![Self::section(
            "Downloading",
            div().flex().flex_col().gap_2().children(downloading),
        )];
        let now = super::now();
        let recent: Vec<AnyElement> = feed
            .map(|f| f.recent.as_slice())
            .unwrap_or_default()
            .iter()
            .take(RECENT_SHOWN)
            .map(|r| self.recent_row(server, kind, r, now, cx))
            .collect();
        if !recent.is_empty() {
            sections.push(Self::section(
                "Recently",
                div().flex().flex_col().gap_2().children(recent),
            ));
        }
        sections
    }

    pub(super) fn downloads_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let note = |text: String, color: u32| {
            div()
                .text_sm()
                .text_color(rgb(color))
                .child(text)
                .into_any_element()
        };
        let mut page = vec![
            div()
                .pt_6()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .child("Downloads")
                .into_any_element(),
        ];
        let apps = self.arr_apps();
        let mut servers: Vec<i64> = apps.iter().map(|(s, ..)| *s).collect();
        servers.dedup();
        let now = super::now();
        for &server in &servers {
            let kinds: Vec<Kind> = apps
                .iter()
                .filter(|(s, ..)| *s == server)
                .map(|(_, k, _)| *k)
                .collect();
            if servers.len() > 1 {
                page.push(
                    div()
                        .pt_8()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.source_name(Some(server)))
                        .into_any_element(),
                );
            }
            let feeds: Vec<(Kind, &Feed)> = kinds
                .iter()
                .filter_map(|k| Some((*k, self.downloads.feeds.get(&(server, *k))?)))
                .collect();
            let names = kinds
                .iter()
                .map(|k| k.name())
                .collect::<Vec<_>>()
                .join(" and ");
            let mut downloading: Vec<AnyElement> = feeds
                .iter()
                .filter_map(|(k, f)| f.error.as_ref().map(|e| (k, e)))
                .map(|(_, e)| note(e.to_string(), ERROR))
                .collect();
            for (kind, feed) in &feeds {
                for download in &feed.downloads {
                    downloading.push(self.download_row(server, *kind, download, cx));
                }
            }
            let loaded = feeds.iter().any(|(_, f)| f.loaded);
            if !loaded {
                downloading.push(note(format!("Asking {names}…"), MUTED));
            } else if feeds.iter().all(|(_, f)| f.downloads.is_empty()) {
                downloading.push(note("Nothing is downloading right now.".into(), MUTED));
            }
            page.push(Self::section(
                "Downloading",
                div().flex().flex_col().gap_2().children(downloading),
            ));
            let mut recent: Vec<(Kind, &Recent)> = feeds
                .iter()
                .flat_map(|(k, f)| f.recent.iter().map(move |r| (*k, r)))
                .collect();
            recent.sort_by_key(|(_, r)| std::cmp::Reverse(r.at));
            if !recent.is_empty() {
                let rows: Vec<AnyElement> = recent
                    .into_iter()
                    .take(RECENT_SHOWN)
                    .map(|(kind, r)| self.recent_row(server, kind, r, now, cx))
                    .collect();
                page.push(Self::section(
                    "Recently",
                    div().flex().flex_col().gap_2().children(rows),
                ));
            }
        }
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_match_with_or_without_a_country() {
        let score = |a: &str, b: &str| Title::new(a).score(&Title::new(b));
        assert_eq!(score("Shameless (US)", "Shameless (US)"), 2);
        assert_eq!(score("Shameless", "Shameless (US)"), 1);
        assert_eq!(score("Grey's Anatomy", "Greys Anatomy"), 2);
        assert_eq!(score("Shameless", "Shadows"), 0);
    }

    #[test]
    fn downloads_lead_to_the_show_or_movie() {
        use crate::library::Item;
        let item = |id: &str, title: &str, show: Option<&str>, year: Option<u16>| Item {
            library: None,
            source: Source::Jellyfin {
                server: 1,
                id: id.into(),
            },
            title: title.into(),
            year,
            show: show.map(String::from),
            season: show.map(|_| 1),
            episode: show.map(|_| 1),
            duration: None,
            position: 0.0,
            watched: false,
            last_played: None,
            thumb: None,
            added: None,
        };
        let items = [
            item("e1", "Pilot", Some("Shameless (US)"), None),
            item("e2", "Second", Some("Shameless (US)"), None),
            item("m1", "Dune", None, Some(1984)),
            item("m2", "Dune", None, Some(2021)),
        ];
        let index = TitleIndex::new([0; 4], items.iter());
        let episodes = Subject::Episodes {
            show: "Shameless".into(),
            year: None,
            episodes: vec![(11, 1, None), (11, 2, None)],
        };
        assert!(matches!(
            index.target(1, &episodes),
            Some(Target::Show(name, Some(11))) if name == "Shameless (US)"
        ));
        // Another server's library doesn't count.
        assert!(index.target(2, &episodes).is_none());
        let dune = Subject::Movie {
            title: "Dune".into(),
            year: Some(2021),
        };
        assert!(matches!(
            index.target(1, &dune),
            Some(Target::Movie(key)) if key == items[3].key()
        ));
    }

    #[test]
    fn times() {
        assert_eq!(ago(1000, 1030), "just now");
        assert_eq!(ago(0, 7200), "2 h ago");
        assert_eq!(ago(0, 100_000), "yesterday");
        assert_eq!(left(754), "12 min left");
        assert_eq!(left(7384), "2 h 3 min left");
        assert_eq!(size(24_300_000_000.0), "24.3 GB");
    }
}
