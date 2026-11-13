//! What Sonarr and Radarr are doing: the downloads in their queue, and what
//! they recently grabbed, added or failed at. A season pack is one download
//! with many episodes; it's shown once.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use super::parse_utc;

/// What a download or event is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    Episodes {
        show: String,
        year: Option<i32>,
        /// (season, episode, title), in order.
        episodes: Vec<(u32, u32, Option<String>)>,
    },
    Movie {
        title: String,
        year: Option<i32>,
    },
    /// Something the app couldn't match to a show or movie.
    Unknown(String),
}

impl Subject {
    /// "Shameless (US)", and "S11E12 · Father Frank" / "Season 11 · 12
    /// episodes" / "2021".
    pub fn label(&self) -> (String, Option<String>) {
        match self {
            Subject::Episodes { show, episodes, .. } => {
                let detail = match episodes.as_slice() {
                    [] => None,
                    [(s, e, title)] => Some(match title.as_deref().filter(|t| !t.is_empty()) {
                        Some(title) => format!("S{s}E{e} · {title}"),
                        None => format!("S{s}E{e}"),
                    }),
                    [(first, ..), rest @ ..] => {
                        let n = episodes.len();
                        Some(if rest.iter().all(|(s, ..)| s == first) {
                            format!("Season {first} · {n} episodes")
                        } else {
                            format!("{n} episodes")
                        })
                    }
                };
                (show.clone(), detail)
            }
            Subject::Movie { title, year } => (title.clone(), year.map(|y| y.to_string())),
            Subject::Unknown(title) => (title.clone(), None),
        }
    }

    fn add_episodes(&mut self, other: &Subject) {
        if let (Subject::Episodes { episodes, .. }, Subject::Episodes { episodes: more, .. }) =
            (self, other)
        {
            for episode in more {
                if !episodes.contains(episode) {
                    episodes.push(episode.clone());
                }
            }
            episodes.sort_by_key(|(s, e, _)| (*s, *e));
        }
    }
}

/// Where a download is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Queued,
    Paused,
    Downloading,
    /// Held back by a delay profile.
    Delayed,
    ImportPending,
    Importing,
    /// Downloaded, but the app can't import it (see the messages).
    ImportBlocked,
    Failed,
    Warning,
    ClientUnavailable,
    Done,
}

impl State {
    fn from(
        status: Option<&str>,
        tracked_state: Option<&str>,
        tracked_status: Option<&str>,
    ) -> Self {
        let lower = |s: Option<&str>| s.unwrap_or("").to_ascii_lowercase();
        match lower(tracked_state).as_str() {
            "importblocked" => return State::ImportBlocked,
            "importpending" => return State::ImportPending,
            "importing" => return State::Importing,
            "failed" | "failedpending" => return State::Failed,
            _ => {}
        }
        match lower(status).as_str() {
            "queued" => State::Queued,
            "paused" => State::Paused,
            "delay" => State::Delayed,
            "completed" => State::Done,
            "failed" => State::Failed,
            "warning" => State::Warning,
            "downloadclientunavailable" => State::ClientUnavailable,
            _ if lower(tracked_status) == "warning" => State::Warning,
            _ if lower(tracked_status) == "error" => State::Failed,
            _ => State::Downloading,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Queued => "Queued",
            State::Paused => "Paused",
            State::Downloading => "Downloading",
            State::Delayed => "Waiting (delay profile)",
            State::ImportPending => "Waiting to import",
            State::Importing => "Importing",
            State::ImportBlocked => "Can't import",
            State::Failed => "Failed",
            State::Warning => "Needs attention",
            State::ClientUnavailable => "Download client unreachable",
            State::Done => "Downloaded",
        }
    }

    /// Needs looking at in Sonarr or Radarr.
    pub fn is_problem(self) -> bool {
        matches!(
            self,
            State::ImportBlocked | State::Failed | State::Warning | State::ClientUnavailable
        )
    }
}

/// One download in the queue.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub id: String,
    pub subject: Subject,
    /// The release's name.
    pub release: String,
    pub quality: Option<String>,
    pub size: f64,
    pub sizeleft: f64,
    /// Seconds left, as the download client estimates.
    pub timeleft: Option<i64>,
    pub state: State,
    /// Why it's stuck, from the app.
    pub messages: Vec<String>,
    pub client: Option<String>,
}

impl Download {
    /// 0..1, when the size is known.
    pub fn progress(&self) -> Option<f32> {
        (self.size > 0.0).then(|| (1.0 - self.sizeleft / self.size).clamp(0.0, 1.0) as f32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Sent to the download client.
    Grabbed,
    /// Downloaded and added to the library.
    Imported,
    Failed,
}

impl Event {
    fn from(event_type: &str) -> Option<Self> {
        match event_type {
            "grabbed" => Some(Event::Grabbed),
            "downloadFolderImported" | "seriesFolderImported" | "movieFolderImported" => {
                Some(Event::Imported)
            }
            "downloadFailed" => Some(Event::Failed),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Event::Grabbed => "Grabbed",
            Event::Imported => "Added",
            Event::Failed => "Failed",
        }
    }
}

/// Something that happened to a download.
#[derive(Debug, Clone, PartialEq)]
pub struct Recent {
    pub id: String,
    pub subject: Subject,
    pub event: Event,
    /// Unix seconds.
    pub at: i64,
    pub release: String,
    pub quality: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Page<T> {
    #[serde(default = "Vec::new")]
    pub records: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesRef {
    title: String,
    year: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeRef {
    season_number: u32,
    episode_number: u32,
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MovieRef {
    title: String,
    year: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusMessage {
    title: Option<String>,
    #[serde(default)]
    messages: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QueueRecord {
    id: i64,
    download_id: Option<String>,
    title: Option<String>,
    #[serde(default)]
    size: f64,
    #[serde(default)]
    sizeleft: f64,
    timeleft: Option<String>,
    status: Option<String>,
    tracked_download_status: Option<String>,
    tracked_download_state: Option<String>,
    #[serde(default)]
    status_messages: Vec<StatusMessage>,
    error_message: Option<String>,
    download_client: Option<String>,
    quality: Option<Value>,
    series: Option<SeriesRef>,
    episode: Option<EpisodeRef>,
    movie: Option<MovieRef>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HistoryRecord {
    id: i64,
    event_type: String,
    date: String,
    download_id: Option<String>,
    source_title: Option<String>,
    quality: Option<Value>,
    series: Option<SeriesRef>,
    episode: Option<EpisodeRef>,
    movie: Option<MovieRef>,
}

/// "WEBDL-1080p", from a `quality` object (or its inner one).
pub(super) fn quality_name(quality: &Value) -> Option<String> {
    quality["quality"]["name"]
        .as_str()
        .or_else(|| quality["name"].as_str())
        .map(String::from)
}

/// `00:12:34` or `1.02:03:04` (days first) → seconds.
pub fn parse_timeleft(text: &str) -> Option<i64> {
    let (days, clock) = match text.split_once('.') {
        // `00:00:05.1234567` has fractions of a second, not days.
        Some((days, clock)) if !days.contains(':') => (days.parse::<i64>().ok()?, clock),
        _ => (0, text),
    };
    let clock = clock.split('.').next()?;
    let mut parts = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, s) = (parts.next()??, parts.next()??, parts.next()??);
    Some(days * 86_400 + h * 3600 + m * 60 + s)
}

fn subject(
    series: Option<SeriesRef>,
    episode: Option<EpisodeRef>,
    movie: Option<MovieRef>,
    fallback: &str,
) -> Subject {
    match (series, movie) {
        (Some(series), _) => Subject::Episodes {
            show: series.title,
            year: series.year,
            episodes: episode
                .map(|e| vec![(e.season_number, e.episode_number, e.title)])
                .unwrap_or_default(),
        },
        (None, Some(movie)) => Subject::Movie {
            title: movie.title,
            year: movie.year,
        },
        _ => Subject::Unknown(fallback.to_string()),
    }
}

/// The queue's records as downloads: one per download, its episodes
/// together.
pub(super) fn downloads(records: Vec<QueueRecord>) -> Vec<Download> {
    let mut out: Vec<Download> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for record in records {
        let id = record
            .download_id
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("queue-{}", record.id));
        let release = record.title.clone().unwrap_or_default();
        let subject = subject(record.series, record.episode, record.movie, &release);
        if let Some(&i) = index.get(&id) {
            out[i].subject.add_episodes(&subject);
            continue;
        }
        let mut messages: Vec<String> = record
            .status_messages
            .iter()
            .flat_map(|m| {
                if m.messages.is_empty() {
                    m.title.clone().into_iter().collect()
                } else {
                    m.messages.clone()
                }
            })
            .collect();
        messages.extend(record.error_message.filter(|e| !e.is_empty()));
        messages.dedup();
        index.insert(id.clone(), out.len());
        out.push(Download {
            id,
            subject,
            release,
            quality: record.quality.as_ref().and_then(quality_name),
            size: record.size,
            sizeleft: record.sizeleft,
            timeleft: record.timeleft.as_deref().and_then(parse_timeleft),
            state: State::from(
                record.status.as_deref(),
                record.tracked_download_state.as_deref(),
                record.tracked_download_status.as_deref(),
            ),
            messages,
            client: record.download_client.filter(|c| !c.is_empty()),
        });
    }
    out
}

/// History (newest first) as one line per download: what last happened to
/// it. Downloads still in the queue (`queued`) are left out; the queue
/// shows them.
pub(super) fn recent(
    records: Vec<HistoryRecord>,
    queued: &HashSet<String>,
    limit: usize,
) -> Vec<Recent> {
    let mut out: Vec<Recent> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for record in records {
        let Some(event) = Event::from(&record.event_type) else {
            continue;
        };
        let id = record
            .download_id
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("history-{}", record.id));
        let release = record.source_title.clone().unwrap_or_default();
        let subject = subject(record.series, record.episode, record.movie, &release);
        if let Some(&i) = index.get(&id) {
            // An older event of the same download: its other episodes
            // count, its earlier steps don't.
            if out[i].event == event {
                out[i].subject.add_episodes(&subject);
            }
            continue;
        }
        if queued.contains(&id) || out.len() >= limit {
            index.insert(id, usize::MAX);
            continue;
        }
        let Some(at) = parse_utc(&record.date) else {
            continue;
        };
        index.insert(id.clone(), out.len());
        out.push(Recent {
            id,
            subject,
            event,
            at,
            release,
            quality: record.quality.as_ref().and_then(quality_name),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_time_left() {
        assert_eq!(parse_timeleft("00:12:34"), Some(754));
        assert_eq!(parse_timeleft("1.02:03:04"), Some(86_400 + 7384));
        assert_eq!(parse_timeleft("00:00:05.1234567"), Some(5));
        assert_eq!(parse_timeleft("soon"), None);
    }

    #[test]
    fn a_season_pack_is_one_download() {
        let records: Page<QueueRecord> = serde_json::from_value(json!({"records": [
            {"id": 1, "downloadId": "ABC", "title": "Show.S11.1080p", "size": 1000.0, "sizeleft": 250.0,
             "timeleft": "00:10:00", "status": "downloading", "trackedDownloadStatus": "ok",
             "trackedDownloadState": "downloading", "quality": {"quality": {"name": "WEBDL-1080p"}},
             "series": {"title": "Shameless (US)", "year": 2011},
             "episode": {"seasonNumber": 11, "episodeNumber": 2, "title": "Two"}},
            {"id": 2, "downloadId": "ABC", "title": "Show.S11.1080p", "size": 1000.0, "sizeleft": 250.0,
             "series": {"title": "Shameless (US)", "year": 2011},
             "episode": {"seasonNumber": 11, "episodeNumber": 1, "title": "One"}},
            {"id": 3, "downloadId": "DEF", "title": "Luca.2021.2160p", "size": 50.0, "sizeleft": 0.0,
             "status": "completed", "trackedDownloadStatus": "warning",
             "trackedDownloadState": "importBlocked",
             "statusMessages": [{"title": "Luca.2021.2160p", "messages": ["No files found are eligible for import"]}],
             "movie": {"title": "Luca", "year": 2021}}
        ]}))
        .unwrap();
        let downloads = downloads(records.records);
        assert_eq!(downloads.len(), 2);
        let pack = &downloads[0];
        assert_eq!(
            pack.subject.label(),
            (
                "Shameless (US)".into(),
                Some("Season 11 · 2 episodes".into())
            )
        );
        assert_eq!(pack.progress(), Some(0.75));
        assert_eq!(pack.timeleft, Some(600));
        assert_eq!(pack.state, State::Downloading);
        assert_eq!(pack.quality.as_deref(), Some("WEBDL-1080p"));
        let movie = &downloads[1];
        assert_eq!(movie.subject.label(), ("Luca".into(), Some("2021".into())));
        assert_eq!(movie.state, State::ImportBlocked);
        assert!(movie.state.is_problem());
        assert_eq!(movie.messages, ["No files found are eligible for import"]);
    }

    #[test]
    fn history_keeps_the_last_step_of_each_download() {
        let records: Page<HistoryRecord> = serde_json::from_value(json!({"records": [
            {"id": 9, "eventType": "downloadFolderImported", "date": "2026-11-13T19:17:01Z",
             "downloadId": "ABC", "sourceTitle": "Show S11E12",
             "series": {"title": "Shameless (US)"}, "episode": {"seasonNumber": 11, "episodeNumber": 12, "title": "Last"}},
            {"id": 8, "eventType": "downloadFolderImported", "date": "2026-11-13T19:17:01Z",
             "downloadId": "ABC", "sourceTitle": "Show S11E11",
             "series": {"title": "Shameless (US)"}, "episode": {"seasonNumber": 11, "episodeNumber": 11}},
            {"id": 7, "eventType": "episodeFileRenamed", "date": "2026-11-13T19:00:00Z",
             "downloadId": "", "sourceTitle": "x", "series": {"title": "Other"}},
            {"id": 6, "eventType": "grabbed", "date": "2026-11-13T18:00:00Z",
             "downloadId": "QUEUED", "sourceTitle": "Still going", "movie": {"title": "Polar", "year": 2019}},
            {"id": 5, "eventType": "grabbed", "date": "2026-11-13T17:00:00Z",
             "downloadId": "ABC", "sourceTitle": "Show.S11.1080p",
             "series": {"title": "Shameless (US)"}, "episode": {"seasonNumber": 11, "episodeNumber": 1}},
            {"id": 4, "eventType": "downloadFailed", "date": "2026-11-12T10:00:00Z",
             "downloadId": "OLD", "sourceTitle": "Luca.2021", "movie": {"title": "Luca", "year": 2021}}
        ]}))
        .unwrap();
        let queued = HashSet::from(["QUEUED".to_string()]);
        let recent = recent(records.records, &queued, 10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].event, Event::Imported);
        assert_eq!(
            recent[0].subject.label().1.as_deref(),
            Some("Season 11 · 2 episodes")
        );
        assert_eq!(recent[1].event, Event::Failed);
        assert_eq!(recent[1].subject.label().0, "Luca");
    }
}
