//! Local media library: scanning folders, parsing names, and persisting
//! items and watch progress.

pub mod db;
pub mod parse;
pub mod scan;
pub mod thumbs;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub use db::Db;

/// Saved positions shorter than this are treated as "not started".
pub const MIN_RESUME_SECONDS: f64 = 10.0;
/// Fraction of the runtime after which an item counts as watched (Jellyfin's
/// default too, so local and server state agree).
pub const WATCHED_FRACTION: f64 = 0.9;

/// Remembered audio and subtitle choices, as language codes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackPrefs {
    pub audio: Option<String>,
    pub subtitle: Option<SubtitlePref>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubtitlePref {
    Off,
    Language(String),
}

impl TrackPrefs {
    /// `self`, with gaps filled from `fallback` (for example a show's
    /// choices over the general ones).
    pub fn or(self, fallback: TrackPrefs) -> TrackPrefs {
        TrackPrefs {
            audio: self.audio.or(fallback.audio),
            subtitle: self.subtitle.or(fallback.subtitle),
        }
    }
}

/// Where an item lives and how to play it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// A file found by scanning a local folder; `id` is its database row.
    Local { id: i64, path: PathBuf },
    /// An item on a signed-in Jellyfin server (`server` is our row id).
    Jellyfin { server: i64, id: String },
}

/// A library on a Jellyfin server ("Movies", "Anime Series", ...), which
/// gets its own tab.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Library {
    pub name: String,
    /// Where the server lists it among the user's libraries.
    pub order: u16,
    /// Browsed by show (a TV library) rather than by title.
    pub shows: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The Jellyfin library it's in; None for local files (and for items
    /// cached before libraries were tracked).
    pub library: Option<Arc<Library>>,
    pub source: Source,
    /// Movie title, or episode title.
    pub title: String,
    pub year: Option<u16>,
    pub show: Option<String>,
    pub season: Option<u16>,
    pub episode: Option<u16>,
    pub duration: Option<f64>,
    pub position: f64,
    pub watched: bool,
    pub last_played: Option<i64>,
    pub thumb: Option<PathBuf>,
    /// When it joined the library (Unix seconds), for "recently added".
    pub added: Option<i64>,
}

impl Item {
    /// Stable identifier, unique across sources (used for UI element ids).
    pub fn key(&self) -> String {
        match &self.source {
            Source::Local { id, .. } => format!("local-{id}"),
            Source::Jellyfin { server, id } => format!("jf-{server}-{id}"),
        }
    }

    #[cfg(test)]
    pub fn local_id(&self) -> Option<i64> {
        match self.source {
            Source::Local { id, .. } => Some(id),
            Source::Jellyfin { .. } => None,
        }
    }

    pub fn is_episode(&self) -> bool {
        self.show.is_some()
    }

    /// Playback progress in 0..=1, if the item has been started.
    pub fn progress(&self) -> Option<f32> {
        match self.duration {
            Some(d) if d > 0.0 && self.position >= MIN_RESUME_SECONDS => {
                Some((self.position / d).clamp(0.0, 1.0) as f32)
            }
            _ => None,
        }
    }

    pub fn resume_position(&self) -> f64 {
        if self.position >= MIN_RESUME_SECONDS {
            self.position
        } else {
            0.0
        }
    }

    /// Short label such as `S1 · E4`.
    pub fn episode_label(&self) -> Option<String> {
        Some(format!("S{} · E{}", self.season?, self.episode?))
    }

    /// Sort key for watching a show in order: seasons in order, specials
    /// (season 0) last, then episode number.
    pub fn watch_order(&self) -> (bool, u16, Option<u16>, String) {
        let season = self.season.unwrap_or(0);
        (season == 0, season, self.episode, self.title.to_lowercase())
    }

    /// Title for headings: the movie title, or `Show · S1 · E4`.
    pub fn display_title(&self) -> String {
        match (&self.show, self.episode_label()) {
            (Some(show), Some(label)) => format!("{show} · {label}"),
            _ => self.title.clone(),
        }
    }
}

/// The next episode of each show being watched: the most recently played
/// episode if it wasn't finished (and has no resume point, or it would be
/// continued instead), else the first unwatched one after it. Paired
/// with when the show was last watched, for sorting.
pub fn next_up<'a>(items: impl Iterator<Item = &'a Item>) -> Vec<(Option<i64>, &'a Item)> {
    let mut shows: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
    for item in items {
        if let Some(show) = &item.show {
            shows.entry(show.as_str()).or_default().push(item);
        }
    }
    shows
        .into_values()
        .filter_map(|mut episodes| {
            episodes.sort_by_key(|e| e.watch_order());
            let (last_index, last) = episodes
                .iter()
                .enumerate()
                .filter(|(_, e)| e.last_played.is_some())
                .max_by_key(|(_, e)| e.last_played)?;
            if last.progress().is_some() {
                return None; // still in progress: it's continued, not next
            }
            // Starting from the last one itself: one that was left in its
            // first minutes has no resume point (Jellyfin drops positions
            // under 5%), but it isn't finished either.
            let next = episodes[last_index..].iter().find(|e| !e.watched)?;
            Some((last.last_played, *next))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(show: &str, season: u16, number: u16) -> Item {
        Item {
            library: None,
            source: Source::Jellyfin {
                server: 1,
                id: format!("{show}-{season}-{number}"),
            },
            title: format!("Episode {number}"),
            year: None,
            show: Some(show.into()),
            season: Some(season),
            episode: Some(number),
            duration: Some(1000.0),
            position: 0.0,
            watched: false,
            last_played: None,
            thumb: None,
            added: None,
        }
    }

    fn keys(list: &[(Option<i64>, &Item)]) -> Vec<String> {
        list.iter().map(|(_, i)| i.key()).collect()
    }

    #[test]
    fn next_up_follows_the_last_finished_episode() {
        let mut items = [
            episode("A", 1, 1),
            episode("A", 1, 2),
            episode("A", 2, 1),
            episode("A", 0, 1), // special: watched last
            episode("B", 1, 1),
            episode("B", 1, 2),
            episode("C", 1, 1),
        ];
        // A: finished S1E2 most recently -> S2E1 is next.
        items[0].watched = true;
        items[0].last_played = Some(100);
        items[1].watched = true;
        items[1].last_played = Some(200);
        // B: E1 half-watched -> continued, not "next".
        items[4].position = 500.0;
        items[4].last_played = Some(300);
        // C: never played -> nothing.
        let next = next_up(items.iter());
        assert_eq!(keys(&next), vec!["jf-1-A-2-1"]);
        assert_eq!(next[0].0, Some(200));

        // Finishing the last regular episode leads to the specials.
        items[2].watched = true;
        items[2].last_played = Some(400);
        assert_eq!(keys(&next_up(items.iter())), vec!["jf-1-A-0-1"]);

        // Everything watched: nothing next.
        items[3].watched = true;
        items[3].last_played = Some(500);
        assert!(next_up(items.iter()).is_empty());
    }

    #[test]
    fn rewatching_an_early_episode_skips_watched_ones() {
        let mut items = [episode("A", 1, 1), episode("A", 1, 2), episode("A", 1, 3)];
        for item in &mut items {
            item.watched = true;
        }
        items[2].watched = false;
        items[0].last_played = Some(900); // rewatched E1 last
        items[1].last_played = Some(100);
        assert_eq!(keys(&next_up(items.iter())), vec!["jf-1-A-1-3"]);
    }

    #[test]
    fn an_episode_left_early_is_still_next() {
        let mut items = [episode("A", 1, 1), episode("A", 1, 2), episode("A", 1, 3)];
        items[0].watched = true;
        items[0].last_played = Some(100);
        // Opened E2 and closed it after a few seconds: no resume point, not
        // watched. It's still the one to watch, not E3.
        items[1].last_played = Some(200);
        assert_eq!(keys(&next_up(items.iter())), vec!["jf-1-A-1-2"]);
    }
}
