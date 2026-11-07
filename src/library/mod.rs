//! Local media library: scanning folders, parsing names, and persisting
//! items and watch progress.

pub mod db;
pub mod parse;
pub mod scan;
pub mod thumbs;

use std::path::PathBuf;

pub use db::Db;

/// Saved positions shorter than this are treated as "not started".
pub const MIN_RESUME_SECONDS: f64 = 10.0;
/// Fraction of the runtime after which an item counts as watched.
pub const WATCHED_FRACTION: f64 = 0.92;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: i64,
    pub path: PathBuf,
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
}

impl Item {
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

    /// Title for headings: the movie title, or `Show · S1 · E4`.
    pub fn display_title(&self) -> String {
        match (&self.show, self.episode_label()) {
            (Some(show), Some(label)) => format!("{show} · {label}"),
            _ => self.title.clone(),
        }
    }
}
