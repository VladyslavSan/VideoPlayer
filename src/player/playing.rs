//! What is being played, as the player shows it: its titles and, for an
//! episode, the show's other episodes.

use gpui::SharedString;

/// An episode of the show being played, for the episode list and "next".
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    /// Library key, sent back in `PlayerEvent::Play`.
    pub key: String,
    pub season: u16,
    pub number: Option<u16>,
    pub title: String,
    pub watched: bool,
    pub progress: Option<f32>,
    pub thumb: Option<std::path::PathBuf>,
}

/// The video's titles and its place among the show's episodes.
pub(super) struct Playing {
    pub(super) title: SharedString,
    pub(super) subtitle: Option<SharedString>,
    /// The show's episodes in watching order (empty for movies).
    pub(super) episodes: Vec<Episode>,
    /// Index of the playing episode in `episodes`.
    pub(super) current: Option<usize>,
}

impl Playing {
    pub(super) fn has_episodes(&self) -> bool {
        !self.episodes.is_empty()
    }

    pub(super) fn next(&self) -> Option<&Episode> {
        self.episodes.get(self.current? + 1)
    }

    pub(super) fn previous(&self) -> Option<&Episode> {
        self.episodes.get(self.current?.checked_sub(1)?)
    }

    /// The season of the playing episode.
    pub(super) fn current_season(&self) -> Option<u16> {
        Some(self.episodes.get(self.current?)?.season)
    }

    /// The show's seasons, in watching order.
    pub(super) fn seasons(&self) -> Vec<u16> {
        let mut seasons: Vec<u16> = self.episodes.iter().map(|e| e.season).collect();
        seasons.dedup();
        seasons
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(season: u16, number: u16) -> Episode {
        Episode {
            key: format!("s{season}e{number}"),
            season,
            number: Some(number),
            title: String::new(),
            watched: false,
            progress: None,
            thumb: None,
        }
    }

    fn show(current: Option<usize>) -> Playing {
        Playing {
            title: "Show".into(),
            subtitle: None,
            episodes: vec![episode(1, 1), episode(1, 2), episode(2, 1)],
            current,
        }
    }

    #[test]
    fn neighbours_stop_at_the_ends_of_the_show() {
        let first = show(Some(0));
        assert_eq!(first.previous(), None);
        assert_eq!(first.next().map(|e| e.key.as_str()), Some("s1e2"));
        let last = show(Some(2));
        assert_eq!(last.previous().map(|e| e.key.as_str()), Some("s1e2"));
        assert_eq!(last.next(), None);
    }

    #[test]
    fn a_movie_has_no_neighbours_or_seasons() {
        let movie = Playing {
            title: "Movie".into(),
            subtitle: None,
            episodes: Vec::new(),
            current: None,
        };
        assert_eq!(movie.next(), None);
        assert_eq!(movie.previous(), None);
        assert_eq!(movie.current_season(), None);
        assert!(movie.seasons().is_empty());
        assert!(!movie.has_episodes());
    }

    #[test]
    fn seasons_are_listed_once_each() {
        assert_eq!(show(Some(2)).seasons(), [1, 2]);
        assert_eq!(show(Some(2)).current_season(), Some(2));
    }
}
