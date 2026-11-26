//! Rules of playback that need no player: what the player is titled, the
//! episodes around the one playing, where track and subtitle choices are
//! remembered, and what is recorded when a video closes.

use gpui::SharedString;

use crate::library::{Item, Source, Stop};
use crate::player::Episode;

/// The scope of the track choices that apply when a show has none.
pub(super) const GENERAL_PREFS: &str = "general";

/// The scope of a show's own track choices.
pub(super) fn show_scope(show: &str) -> String {
    format!("show:{show}")
}

/// What the player is headed with.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Titles {
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
}

/// Episodes: the show on top, the episode beside it. Movies: title and year.
pub(super) fn titles(item: &Item) -> Titles {
    match &item.show {
        Some(show) => Titles {
            title: show.clone().into(),
            subtitle: Some(match item.episode_label() {
                Some(label) => format!("{label} · {}", item.title).into(),
                None => item.title.clone().into(),
            }),
        },
        None => Titles {
            title: item.title.clone().into(),
            subtitle: item.year.map(|year| year.to_string().into()),
        },
    }
}

/// The show's episodes in watching order (seasons in order, specials last)
/// and where `item` sits among them. Empty for movies.
pub(super) fn episodes_around<'a>(
    items: impl Iterator<Item = &'a Item>,
    item: &Item,
) -> (Vec<Episode>, Option<usize>) {
    let Some(show) = &item.show else {
        return (Vec::new(), None);
    };
    let mut items: Vec<&Item> = items.filter(|i| i.show.as_ref() == Some(show)).collect();
    items.sort_by_key(|i| i.watch_order());
    let key = item.key();
    let current = items.iter().position(|i| i.key() == key);
    let episodes = items
        .into_iter()
        .map(|i| Episode {
            key: i.key(),
            season: i.season.unwrap_or(0),
            number: i.episode,
            title: i.title.clone(),
            watched: i.watched,
            progress: i.progress(),
            thumb: i.thumb.clone(),
        })
        .collect();
    (episodes, current)
}

/// Where an item's subtitle delay and size are remembered: episodes share
/// their show's, a movie keeps its own.
pub(super) fn tuning_scope(item: &Item) -> String {
    match &item.show {
        Some(show) => show_scope(show),
        None => format!("item:{}", item.key()),
    }
}

/// Where a track choice made now is remembered: as the general preference,
/// and for the show being watched (if it's an episode).
pub(super) fn pref_scopes(show: Option<&str>) -> Vec<String> {
    let mut scopes = vec![GENERAL_PREFS.to_string()];
    scopes.extend(show.map(show_scope));
    scopes
}

/// How far a video was, by one account of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct End {
    pub position: f64,
    pub duration: f64,
    /// Played to the end (or far enough to count).
    pub finished: bool,
}

impl End {
    /// The player's own account once it knows the video's length (it is the
    /// freshest), else the last progress heard from it, never finished.
    pub(super) fn freshest(player: End, heard_position: f64, heard_duration: f64) -> End {
        if player.duration > 0.0 {
            player
        } else {
            End {
                position: heard_position,
                duration: heard_duration,
                finished: false,
            }
        }
    }

    /// The position to keep: a finished video counts as its full runtime,
    /// so it's watched even when left during the end credits.
    fn reported(&self) -> f64 {
        if self.finished {
            self.duration
        } else {
            self.position
        }
    }
}

/// What a closing video leaves behind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Record {
    /// A local file's progress, saved in the database.
    Local {
        id: i64,
        position: f64,
        duration: f64,
    },
    /// Where a Jellyfin item stopped, for its server.
    Server {
        position: f64,
        duration: f64,
        finished: bool,
    },
    Nothing,
}

impl Record {
    /// What to record for a video from `source` that closed at `end`.
    /// `settled_paused`: it was paused, and the server has heard the rest.
    pub(super) fn of(source: Option<&Source>, settled_paused: bool, end: End) -> Record {
        match source {
            Some(Source::Local { id, .. }) if end.duration > 0.0 => Record::Local {
                id: *id,
                position: end.reported(),
                duration: end.duration,
            },
            Some(Source::Jellyfin { .. }) if !settled_paused => Record::Server {
                position: end.position,
                duration: end.duration,
                finished: end.finished,
            },
            _ => Record::Nothing,
        }
    }

    /// The stop to tell the server about, for a Jellyfin item.
    pub(super) fn stop(&self) -> Option<Stop> {
        match *self {
            Record::Server {
                position,
                duration,
                finished,
            } => Some(Stop {
                position,
                duration,
                finished,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(title: &str, year: Option<u16>) -> Item {
        Item {
            library: None,
            source: Source::Jellyfin {
                server: 1,
                id: title.into(),
            },
            title: title.into(),
            year,
            show: None,
            season: None,
            episode: None,
            duration: None,
            position: 0.0,
            watched: false,
            last_played: None,
            thumb: None,
            added: None,
        }
    }

    fn episode(show: &str, season: u16, number: u16, title: &str) -> Item {
        Item {
            show: Some(show.into()),
            season: Some(season),
            episode: Some(number),
            ..movie(title, None)
        }
    }

    fn end(position: f64, duration: f64, finished: bool) -> End {
        End {
            position,
            duration,
            finished,
        }
    }

    #[test]
    fn a_movie_is_titled_by_its_name_and_year() {
        let with_year = titles(&movie("Heat", Some(1995)));
        assert_eq!(with_year.title, SharedString::from("Heat"));
        assert_eq!(with_year.subtitle, Some(SharedString::from("1995")));
        assert_eq!(titles(&movie("Heat", None)).subtitle, None);
    }

    #[test]
    fn an_episode_is_titled_by_its_show_with_the_episode_beside_it() {
        let numbered = titles(&episode("Dark", 1, 4, "Double Lives"));
        assert_eq!(numbered.title, SharedString::from("Dark"));
        let beside = SharedString::from("S1 · E4 · Double Lives");
        assert_eq!(numbered.subtitle, Some(beside));
        let mut unnumbered = episode("Dark", 1, 4, "Extra");
        unnumbered.episode = None;
        let subtitle = titles(&unnumbered).subtitle;
        assert_eq!(subtitle, Some(SharedString::from("Extra")));
    }

    #[test]
    fn episodes_come_in_watching_order_with_specials_last() {
        let special = episode("Dark", 0, 1, "Special");
        let second = episode("Dark", 1, 2, "Two");
        let first = episode("Dark", 1, 1, "One");
        let later = episode("Dark", 2, 1, "Three");
        let other = episode("Other", 1, 1, "Pilot");
        let film = movie("Heat", None);
        let items = [&special, &second, &other, &film, &later, &first];
        let (episodes, current) = episodes_around(items.into_iter(), &second);
        let titles: Vec<&str> = episodes.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two", "Three", "Special"]);
        assert_eq!(current, Some(1));
    }

    #[test]
    fn a_movie_has_no_episodes_around_it() {
        let film = movie("Heat", None);
        let (episodes, current) = episodes_around([&film].into_iter(), &film);
        assert!(episodes.is_empty());
        assert_eq!(current, None);
    }

    #[test]
    fn subtitle_tuning_is_kept_per_show_or_per_movie() {
        assert_eq!(tuning_scope(&episode("Dark", 1, 1, "One")), "show:Dark");
        assert_eq!(tuning_scope(&movie("Heat", None)), "item:jf-1-Heat");
    }

    #[test]
    fn a_track_choice_is_kept_in_general_and_for_the_show() {
        assert_eq!(pref_scopes(None), ["general"]);
        assert_eq!(pref_scopes(Some("Dark")), ["general", "show:Dark"]);
    }

    #[test]
    fn the_player_is_believed_once_it_knows_the_length() {
        let player = end(50.0, 100.0, true);
        assert_eq!(End::freshest(player, 40.0, 100.0), player);
        let unknown = end(0.0, 0.0, true);
        assert_eq!(End::freshest(unknown, 40.0, 90.0), end(40.0, 90.0, false));
    }

    #[test]
    fn a_finished_local_file_is_saved_at_its_full_runtime() {
        let source = Source::Local {
            id: 7,
            path: "D:/film.mkv".into(),
        };
        let saved = |end| Record::of(Some(&source), false, end);
        let full = Record::Local {
            id: 7,
            position: 100.0,
            duration: 100.0,
        };
        assert_eq!(saved(end(95.0, 100.0, true)), full);
        let partway = Record::Local {
            id: 7,
            position: 30.0,
            duration: 100.0,
        };
        assert_eq!(saved(end(30.0, 100.0, false)), partway);
        assert_eq!(saved(end(30.0, 0.0, false)), Record::Nothing);
    }

    #[test]
    fn a_server_hears_where_its_item_stopped_unless_it_was_paused() {
        let source = Source::Jellyfin {
            server: 1,
            id: "a".into(),
        };
        let stopped = Record::of(Some(&source), false, end(95.0, 100.0, true));
        let stop = stopped.stop().expect("a stop for the server");
        assert_eq!((stop.position, stop.duration), (95.0, 100.0));
        assert!(stop.finished);
        let paused = Record::of(Some(&source), true, end(95.0, 100.0, true));
        assert_eq!(paused, Record::Nothing);
        assert!(paused.stop().is_none());
    }

    #[test]
    fn a_file_from_the_command_line_leaves_nothing_behind() {
        let record = Record::of(None, false, end(30.0, 100.0, false));
        assert_eq!(record, Record::Nothing);
    }
}
