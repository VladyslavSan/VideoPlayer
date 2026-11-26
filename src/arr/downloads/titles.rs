//! Which show or movie of a server's library a download is of, by title:
//! Sonarr and Radarr name things a little differently than Jellyfin does.
//! The titles are indexed once per change to the library, not for every
//! line on every frame (which made the Downloads page scroll slowly).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use crate::arr::Subject;
use crate::library::{Catalog, Item, Source};

/// A title, as compared: letters and digits only, whole and without a
/// trailing `(…)`.
#[derive(Debug)]
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

/// A show in a server's library.
#[derive(Debug)]
struct Show {
    title: Title,
    name: String,
}

/// A movie in a server's library.
#[derive(Debug)]
struct Movie {
    title: Title,
    year: Option<u16>,
    /// The item's key.
    key: String,
}

/// Where a download's line leads in the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A show's page, at `season` when the download is of one season.
    Show { name: String, season: Option<u32> },
    /// The page of the movie with this item key.
    Movie { key: String },
}

/// The library's show and movie titles, by server.
#[derive(Debug, Default)]
pub struct TitleIndex {
    shows: HashMap<i64, Vec<Show>>,
    movies: HashMap<i64, Vec<Movie>>,
}

impl TitleIndex {
    fn new<'a>(items: impl Iterator<Item = &'a Item>) -> Self {
        let mut shows: HashMap<i64, Vec<Show>> = HashMap::new();
        let mut seen: HashSet<(i64, &str)> = HashSet::new();
        let mut movies: HashMap<i64, Vec<Movie>> = HashMap::new();
        for item in items {
            let Source::Jellyfin { server, .. } = item.source else {
                continue;
            };
            match item.show.as_deref() {
                Some(show) => {
                    if seen.insert((server, show)) {
                        shows.entry(server).or_default().push(Show {
                            title: Title::new(show),
                            name: show.to_string(),
                        });
                    }
                }
                None => movies.entry(server).or_default().push(Movie {
                    title: Title::new(&item.title),
                    year: item.year,
                    key: item.key(),
                }),
            }
        }
        Self { shows, movies }
    }

    /// The show or movie in `server`'s library that `subject` is about.
    pub fn target(&self, server: i64, subject: &Subject) -> Option<Target> {
        match subject {
            Subject::Episodes { show, episodes, .. } => {
                let wanted = Title::new(show);
                let name = self
                    .shows
                    .get(&server)?
                    .iter()
                    .map(|show| (show.title.score(&wanted), &show.name))
                    .filter(|(score, _)| *score > 0)
                    .max_by_key(|(score, _)| *score)?
                    .1
                    .clone();
                let first = episodes.first().map(|(s, ..)| *s);
                let season = first.filter(|s| episodes.iter().all(|(n, ..)| n == s));
                Some(Target::Show { name, season })
            }
            Subject::Movie { title, year } => {
                let wanted = Title::new(title);
                self.movies
                    .get(&server)?
                    .iter()
                    .filter(|movie| match (movie.year, year) {
                        (Some(a), Some(b)) => i32::from(a) == *b,
                        _ => true,
                    })
                    .map(|movie| (movie.title.score(&wanted), &movie.key))
                    .filter(|(score, _)| *score > 0)
                    .max_by_key(|(score, _)| *score)
                    .map(|(_, key)| Target::Movie { key: key.clone() })
            }
            Subject::Unknown(_) => None,
        }
    }
}

/// The index and the catalog it was made from.
#[derive(Debug)]
struct Made {
    catalog: Arc<Catalog>,
    index: Rc<TitleIndex>,
}

/// The title index of the catalog last asked about, made when first needed.
#[derive(Debug, Default)]
pub(super) struct TitleCache {
    // A cell: lookups come while pages are drawn, through a shared borrow.
    made: RefCell<Option<Made>>,
}

impl TitleCache {
    /// The index of `catalog`'s titles; made again once the library changed.
    pub(super) fn index(&self, catalog: &Arc<Catalog>) -> Rc<TitleIndex> {
        let mut made = self.made.borrow_mut();
        match &*made {
            // The same snapshot: the library copies the catalog to change it.
            Some(made) if Arc::ptr_eq(&made.catalog, catalog) => made.index.clone(),
            _ => {
                let index = Rc::new(TitleIndex::new(catalog.items()));
                *made = Some(Made {
                    catalog: catalog.clone(),
                    index: index.clone(),
                });
                index
            }
        }
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

    fn item(id: &str, title: &str, show: Option<&str>, year: Option<u16>) -> Item {
        Item {
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
        }
    }

    #[test]
    fn downloads_lead_to_the_show_or_movie() {
        let items = [
            item("e1", "Pilot", Some("Shameless (US)"), None),
            item("e2", "Second", Some("Shameless (US)"), None),
            item("m1", "Dune", None, Some(1984)),
            item("m2", "Dune", None, Some(2021)),
        ];
        let index = TitleIndex::new(items.iter());
        let episodes = Subject::Episodes {
            show: "Shameless".into(),
            year: None,
            episodes: vec![(11, 1, None), (11, 2, None)],
        };
        assert_eq!(
            index.target(1, &episodes),
            Some(Target::Show {
                name: "Shameless (US)".into(),
                season: Some(11)
            })
        );
        // Another server's library doesn't count.
        assert!(index.target(2, &episodes).is_none());
        let dune = Subject::Movie {
            title: "Dune".into(),
            year: Some(2021),
        };
        assert_eq!(
            index.target(1, &dune),
            Some(Target::Movie {
                key: items[3].key()
            })
        );
    }

    #[test]
    fn episodes_of_several_seasons_lead_to_the_show_alone() {
        let items = [item("e1", "Pilot", Some("Shameless (US)"), None)];
        let index = TitleIndex::new(items.iter());
        let subject = |episodes: Vec<(u32, u32, Option<String>)>| Subject::Episodes {
            show: "Shameless (US)".into(),
            year: None,
            episodes,
        };
        let season_of = |subject: &Subject| match index.target(1, subject) {
            Some(Target::Show { season, .. }) => season,
            other => panic!("not a show: {other:?}"),
        };
        assert_eq!(season_of(&subject(vec![(1, 1, None), (2, 1, None)])), None);
        assert_eq!(season_of(&subject(Vec::new())), None);
        assert_eq!(season_of(&subject(vec![(3, 4, None)])), Some(3));
    }

    #[test]
    fn nothing_leads_from_what_the_app_could_not_match() {
        let items = [item("m1", "Dune", None, Some(1984))];
        let index = TitleIndex::new(items.iter());
        assert!(index.target(1, &Subject::Unknown("Dune".into())).is_none());
        let other_year = Subject::Movie {
            title: "Dune".into(),
            year: Some(2021),
        };
        assert!(index.target(1, &other_year).is_none());
        // A year only one side knows doesn't rule the movie out.
        let no_year = Subject::Movie {
            title: "Dune".into(),
            year: None,
        };
        assert!(index.target(1, &no_year).is_some());
    }

    #[test]
    fn the_index_is_kept_until_the_catalog_is_another() {
        let cache = TitleCache::default();
        let catalog = Arc::new(Catalog::default());
        let first = cache.index(&catalog);
        assert!(Rc::ptr_eq(&first, &cache.index(&catalog)));
        assert!(Rc::ptr_eq(&first, &cache.index(&catalog.clone())));
        // A change to the library gives another catalog, equal or not.
        let changed = Arc::new(Catalog::default());
        let second = cache.index(&changed);
        assert!(!Rc::ptr_eq(&first, &second));
        assert!(Rc::ptr_eq(&second, &cache.index(&changed)));
    }
}
