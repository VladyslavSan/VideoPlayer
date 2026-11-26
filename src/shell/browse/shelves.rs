//! How the library is laid out for browsing: its sources (each server,
//! local folders) and their libraries, the grid of one library, the shows
//! summarised from their episodes, the sort orders, and matching a search.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::SharedString;

use crate::library::{Catalog, Item, Section, Source};
use crate::shell::navigation::Screen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::shell) enum Sort {
    Title,
    RecentlyAdded,
    RecentlyWatched,
    Year,
}

impl Sort {
    pub(super) fn label(self) -> &'static str {
        match self {
            Sort::Title => "Title",
            Sort::RecentlyAdded => "Recently added",
            Sort::RecentlyWatched => "Recently watched",
            Sort::Year => "Year",
        }
    }
}

pub(in crate::shell) const MOVIE_SORTS: [Sort; 4] = [
    Sort::Title,
    Sort::RecentlyAdded,
    Sort::RecentlyWatched,
    Sort::Year,
];
pub(in crate::shell) const SHOW_SORTS: [Sort; 3] =
    [Sort::Title, Sort::RecentlyAdded, Sort::RecentlyWatched];

/// A TV show, summarised from its episodes.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::shell) struct ShowSummary {
    pub name: String,
    pub thumb: Option<PathBuf>,
    pub episodes: usize,
    pub unwatched: usize,
    /// Newest episode's added date.
    pub added: Option<i64>,
    pub last_played: Option<i64>,
}

impl ShowSummary {
    pub(super) fn subtitle(&self) -> String {
        match self.unwatched {
            0 => format!("{} episodes · all watched", self.episodes),
            n if n == self.episodes => format!("{n} episodes"),
            n => format!("{} episodes · {n} unwatched", self.episodes),
        }
    }
}

pub(in crate::shell) fn show_summaries<'a>(
    items: impl Iterator<Item = &'a Item>,
) -> Vec<ShowSummary> {
    let mut shows: BTreeMap<&str, ShowSummary> = BTreeMap::new();
    for item in items {
        let Some(name) = item.show.as_deref() else {
            continue;
        };
        let show = shows.entry(name).or_insert_with(|| ShowSummary {
            name: name.to_string(),
            thumb: None,
            episodes: 0,
            unwatched: 0,
            added: None,
            last_played: None,
        });
        show.episodes += 1;
        show.unwatched += usize::from(!item.watched);
        show.thumb = show.thumb.take().or_else(|| item.thumb.clone());
        show.added = show.added.max(item.added);
        show.last_played = show.last_played.max(item.last_played);
    }
    shows.into_values().collect()
}

/// Newest/most recent first for dates; items without one go last.
fn newest_first(value: Option<i64>) -> (bool, Reverse<i64>) {
    (value.is_none(), Reverse(value.unwrap_or(0)))
}

pub(in crate::shell) fn sort_movies(movies: &mut [&Item], sort: Sort) {
    movies.sort_by_key(|m| m.title.to_lowercase());
    match sort {
        Sort::Title => {}
        Sort::RecentlyAdded => movies.sort_by_key(|m| newest_first(m.added)),
        Sort::RecentlyWatched => movies.sort_by_key(|m| newest_first(m.last_played)),
        Sort::Year => movies.sort_by_key(|m| newest_first(m.year.map(i64::from))),
    }
}

pub(in crate::shell) fn sort_shows(shows: &mut [ShowSummary], sort: Sort) {
    shows.sort_by_key(|s| s.name.to_lowercase());
    match sort {
        Sort::Title | Sort::Year => {}
        Sort::RecentlyAdded => shows.sort_by_key(|s| newest_first(s.added)),
        Sort::RecentlyWatched => shows.sort_by_key(|s| newest_first(s.last_played)),
    }
}

/// Every word of `query` appears in `text` (case-insensitive).
pub(in crate::shell) fn matches(text: &str, query: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// Where an item comes from: a Jellyfin server (our row id), or None for
/// local folders.
pub(in crate::shell) type Origin = Option<i64>;

pub(in crate::shell) fn origin(item: &Item) -> Origin {
    match item.source {
        Source::Jellyfin { server, .. } => Some(server),
        Source::Local { .. } => None,
    }
}

/// One library of one source: a Jellyfin library by name, or None for
/// items in no library (local folders, or a server whose libraries
/// couldn't be listed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::shell) struct Shelf {
    pub origin: Origin,
    pub library: Option<String>,
}

/// A grid: the movies, or the shows, of one shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::shell) struct Grid {
    pub shows: bool,
    pub shelf: Shelf,
}

impl Grid {
    pub(in crate::shell) fn of(screen: &Screen) -> Option<Grid> {
        match screen {
            Screen::Movies(shelf) => Some(Grid {
                shows: false,
                shelf: shelf.clone(),
            }),
            Screen::Shows(shelf) => Some(Grid {
                shows: true,
                shelf: shelf.clone(),
            }),
            _ => None,
        }
    }

    pub(super) fn contains(&self, item: &Item) -> bool {
        item.is_episode() == self.shows
            && origin(item) == self.shelf.origin
            && item.library.as_ref().map(|l| l.name.as_str()) == self.shelf.library.as_deref()
    }

    pub(in crate::shell) fn id(&self) -> SharedString {
        format!(
            "{}-grid-{}-{}",
            if self.shows { "shows" } else { "movies" },
            self.shelf
                .origin
                .map_or("local".to_string(), |s| s.to_string()),
            self.shelf.library.as_deref().unwrap_or("")
        )
        .into()
    }
}

/// A source in the sidebar: its name, its page, and its libraries.
pub(in crate::shell) struct SourceEntry {
    pub origin: Origin,
    pub name: String,
    pub libraries: Vec<(String, Screen)>,
}

/// The sources: each signed-in Jellyfin server with its libraries in the
/// server's order, then local folders (Movies, TV Shows) if there are any.
pub(in crate::shell) fn sources(catalog: &Catalog) -> Vec<SourceEntry> {
    struct Found<'a> {
        libraries: Vec<&'a Section>,
        /// Items in no library: movies, episodes.
        loose: (bool, bool),
    }
    let mut found: BTreeMap<Origin, Found> = BTreeMap::new();
    for item in catalog.items() {
        let entry = found.entry(origin(item)).or_insert(Found {
            libraries: Vec::new(),
            loose: (false, false),
        });
        match &item.library {
            Some(l) if !entry.libraries.iter().any(|known| known.name == l.name) => {
                entry.libraries.push(l)
            }
            Some(_) => {}
            None if item.is_episode() => entry.loose.1 = true,
            None => entry.loose.0 = true,
        }
    }
    let entry = |origin: Origin, name: String, found: Option<&Found>| {
        let mut libraries: Vec<(String, Screen)> = Vec::new();
        if let Some(found) = found {
            let mut named = found.libraries.clone();
            named.sort_by_key(|l| l.order);
            for l in named {
                let shelf = Shelf {
                    origin,
                    library: Some(l.name.clone()),
                };
                let screen = if l.shows {
                    Screen::Shows(shelf)
                } else {
                    Screen::Movies(shelf)
                };
                libraries.push((l.name.clone(), screen));
            }
            let shelf = Shelf {
                origin,
                library: None,
            };
            if found.loose.0 {
                libraries.push(("Movies".into(), Screen::Movies(shelf.clone())));
            }
            if found.loose.1 {
                libraries.push(("TV Shows".into(), Screen::Shows(shelf)));
            }
        }
        SourceEntry {
            origin,
            name,
            libraries,
        }
    };
    let mut sources: Vec<SourceEntry> = catalog
        .servers()
        .iter()
        .map(|server| {
            // One server signed in as two users needs telling apart.
            let twin = catalog
                .servers()
                .iter()
                .any(|s| s.id != server.id && s.name == server.name);
            let name = if twin {
                format!("{} ({})", server.name, server.user_name)
            } else {
                server.name.clone()
            };
            entry(Some(server.id), name, found.get(&Some(server.id)))
        })
        .collect();
    if found.contains_key(&None) || !catalog.roots().is_empty() {
        sources.push(entry(None, "Local folders".into(), found.get(&None)));
    }
    sources
}

/// Every library screen in the sidebar.
pub(in crate::shell) fn library_screens(catalog: &Catalog) -> Vec<Screen> {
    sources(catalog)
        .into_iter()
        .flat_map(|s| s.libraries.into_iter().map(|(_, screen)| screen))
        .collect()
}

/// A source's name in the sidebar.
pub(super) fn source_name(catalog: &Catalog, origin: Origin) -> String {
    match origin {
        Some(id) => catalog
            .servers()
            .iter()
            .find(|s| s.id == id)
            .map_or_else(|| "Jellyfin".into(), |s| s.name.clone()),
        None => "Local folders".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, show: Option<&str>) -> Item {
        Item {
            library: None,
            source: Source::Jellyfin {
                server: 1,
                id: title.into(),
            },
            title: title.into(),
            year: None,
            show: show.map(String::from),
            season: show.map(|_| 1),
            episode: show.map(|_| 1),
            duration: None,
            position: 0.0,
            watched: false,
            last_played: None,
            thumb: None,
            added: None,
        }
    }

    #[test]
    fn grids_hold_one_library() {
        let library = |name: &str, shows| {
            Some(std::sync::Arc::new(Section {
                name: name.into(),
                order: 0,
                shows,
            }))
        };
        let mut movie = item("Movie", None);
        movie.library = library("Movies", false);
        let mut anime = item("Anime", None);
        anime.library = library("Anime Movies", false);
        let mut episode = item("Ep", Some("Show"));
        episode.library = library("Shows", true);
        let mut local = item("Home Video", None);
        local.source = Source::Local {
            id: 1,
            path: "D:/Home Video.mkv".into(),
        };
        // Another server's library of the same name.
        let mut other = item("Other Movie", None);
        other.library = library("Movies", false);
        other.source = Source::Jellyfin {
            server: 2,
            id: "x".into(),
        };
        let shelf = |origin, library: Option<&str>| Shelf {
            origin,
            library: library.map(String::from),
        };

        let grid = Grid::of(&Screen::Movies(shelf(Some(1), Some("Movies")))).unwrap();
        assert!(grid.contains(&movie));
        assert!(!grid.contains(&anime) && !grid.contains(&episode) && !grid.contains(&local));
        assert!(!grid.contains(&other));
        let local_grid = Grid::of(&Screen::Movies(shelf(None, None))).unwrap();
        assert!(local_grid.contains(&local) && !local_grid.contains(&movie));
        let shows = Grid::of(&Screen::Shows(shelf(Some(1), Some("Shows")))).unwrap();
        assert!(shows.contains(&episode) && !shows.contains(&movie));
        assert_eq!(Grid::of(&Screen::Home), None);
    }

    #[test]
    fn search_matches_all_words() {
        assert!(matches("The Lord of the Rings", "lord rings"));
        assert!(matches("Amélie", "amélie"));
        assert!(!matches("The Lord of the Rings", "lord hobbit"));
        assert!(matches("Anything", ""));
    }

    #[test]
    fn sorting_movies() {
        let mut a = item("Alpha", None);
        let mut b = item("beta", None);
        let c = item("Gamma", None);
        a.added = Some(10);
        b.added = Some(20);
        b.last_played = Some(5);
        a.year = Some(1999);
        b.year = Some(2010);
        let titles = |list: &[&Item]| list.iter().map(|i| i.title.clone()).collect::<Vec<_>>();

        let mut list = vec![&c, &b, &a];
        sort_movies(&mut list, Sort::Title);
        assert_eq!(titles(&list), ["Alpha", "beta", "Gamma"]);
        sort_movies(&mut list, Sort::RecentlyAdded);
        assert_eq!(titles(&list), ["beta", "Alpha", "Gamma"]);
        sort_movies(&mut list, Sort::RecentlyWatched);
        assert_eq!(titles(&list), ["beta", "Alpha", "Gamma"]);
        sort_movies(&mut list, Sort::Year);
        assert_eq!(titles(&list), ["beta", "Alpha", "Gamma"]);
    }

    #[test]
    fn shows_are_summarised_from_episodes() {
        let mut e1 = item("One", Some("Show"));
        let mut e2 = item("Two", Some("Show"));
        let e3 = item("Pilot", Some("Other"));
        e1.watched = true;
        e1.added = Some(100);
        e2.added = Some(300);
        e2.last_played = Some(50);
        let movie = item("Film", None);
        let mut shows = show_summaries([&e1, &e2, &e3, &movie].into_iter());
        assert_eq!(shows.len(), 2);
        let show = shows.iter().find(|s| s.name == "Show").unwrap();
        assert_eq!((show.episodes, show.unwatched), (2, 1));
        assert_eq!((show.added, show.last_played), (Some(300), Some(50)));
        assert_eq!(show.subtitle(), "2 episodes · 1 unwatched");
        sort_shows(&mut shows, Sort::RecentlyAdded);
        assert_eq!(shows[0].name, "Show");
        sort_shows(&mut shows, Sort::Title);
        assert_eq!(shows[0].name, "Other");
    }
}
