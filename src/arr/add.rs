//! Adding a show to Sonarr or a movie to Radarr: finding it, and filing it
//! the way the library already does. Each root folder's habits (how much
//! of it is anime, its usual quality profile, tags, series type) come from
//! what's in it, so an anime show goes where the other anime shows are,
//! tagged like them.

use std::collections::HashMap;
use std::hash::Hash;

use serde::Deserialize;
use serde_json::{Value, json};

use super::Kind;

/// A show or movie found by name.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub title: String,
    pub year: Option<i32>,
    /// TVDB id (shows) or TMDB id (movies).
    pub key: i64,
    pub overview: Option<String>,
    pub genres: Vec<String>,
    /// Original language: "Japanese".
    pub language: Option<String>,
    /// Network (shows) or studio (movies).
    pub network: Option<String>,
    pub seasons: usize,
    /// "continuing", "ended", "released", "announced".
    pub status: Option<String>,
    pub poster: Option<String>,
    /// Its id in Sonarr/Radarr, when it's already there.
    pub existing: Option<i64>,
    /// The lookup's answer, sent back (with choices) to add it.
    raw: Value,
}

/// Anime by its metadata: TVDB's "Anime" genre, or animated and Japanese
/// (TMDB has no anime genre).
pub fn is_anime(genres: &[String], language: Option<&str>) -> bool {
    let has = |g: &str| genres.iter().any(|x| x.eq_ignore_ascii_case(g));
    has("Anime") || (has("Animation") && language == Some("Japanese"))
}

fn language(raw: &Value) -> Option<String> {
    raw["originalLanguage"]["name"].as_str().map(String::from)
}

fn genres(raw: &Value) -> Vec<String> {
    raw["genres"]
        .as_array()
        .map(|g| {
            g.iter()
                .filter_map(|g| g.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// A smaller poster than the original, where the host makes them.
pub fn poster_size(url: &str) -> String {
    url.replacen("/t/p/original/", "/t/p/w342/", 1)
}

impl Candidate {
    fn from(kind: Kind, raw: Value) -> Option<Self> {
        let key = match kind {
            Kind::Sonarr => raw["tvdbId"].as_i64(),
            Kind::Radarr => raw["tmdbId"].as_i64(),
        }
        .filter(|k| *k > 0)?;
        let text = |field: &str| {
            raw[field]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        let poster = text("remotePoster").or_else(|| {
            raw["images"].as_array()?.iter().find_map(|i| {
                (i["coverType"] == "poster")
                    .then(|| i["remoteUrl"].as_str().map(String::from))
                    .flatten()
            })
        });
        Some(Self {
            title: text("title")?,
            year: raw["year"].as_i64().map(|y| y as i32).filter(|y| *y > 0),
            key,
            overview: text("overview"),
            genres: genres(&raw),
            language: language(&raw),
            network: text("network").or_else(|| text("studio")),
            seasons: raw["seasons"]
                .as_array()
                .map_or(0, |s| s.iter().filter(|s| s["seasonNumber"] != 0).count()),
            status: text("status"),
            poster: poster.map(|p| poster_size(&p)),
            existing: raw["id"].as_i64().filter(|id| *id > 0),
            raw,
        })
    }

    pub fn is_anime(&self) -> bool {
        is_anime(&self.genres, self.language.as_deref())
    }
}

pub(super) fn candidates(kind: Kind, found: Vec<Value>) -> Vec<Candidate> {
    found
        .into_iter()
        .filter_map(|raw| Candidate::from(kind, raw))
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootFolder {
    pub path: String,
    pub free_space: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tag {
    pub id: i64,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub id: i64,
    pub name: String,
}

/// How a root folder's shows or movies are set up, by majority.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Habits {
    pub count: usize,
    pub anime: usize,
    pub profile: Option<i64>,
    /// Tags on at least half of them.
    pub tags: Vec<i64>,
    pub series_type: Option<String>,
    pub season_folder: Option<bool>,
    pub minimum_availability: Option<String>,
}

/// Where things can go, and how each place is used.
#[derive(Debug, Clone)]
pub struct Setup {
    pub kind: Kind,
    pub roots: Vec<RootFolder>,
    pub profiles: Vec<Profile>,
    pub tags: Vec<Tag>,
    /// One per root.
    pub habits: Vec<Habits>,
}

fn most<T: Eq + Hash + Clone>(values: impl Iterator<Item = T>) -> Option<T> {
    let mut counts: HashMap<T, usize> = HashMap::new();
    let mut order = Vec::new();
    for v in values {
        let n = counts.entry(v.clone()).or_default();
        if *n == 0 {
            order.push(v);
        }
        *n += 1;
    }
    // Ties go to the first seen.
    order
        .into_iter()
        .max_by(|a, b| counts[a].cmp(&counts[b]).then(std::cmp::Ordering::Greater))
}

/// The root folder a path is in: the longest that contains it.
fn root_of(roots: &[RootFolder], path: &str) -> Option<usize> {
    let path = path.replace('\\', "/");
    roots
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let root = r.path.replace('\\', "/");
            let root = root.trim_end_matches('/');
            path.starts_with(&format!("{root}/"))
        })
        .max_by_key(|(_, r)| r.path.len())
        .map(|(i, _)| i)
}

/// Each root's habits, from the shows or movies in it.
pub fn habits(roots: &[RootFolder], items: &[Value]) -> Vec<Habits> {
    let mut groups: Vec<Vec<&Value>> = vec![Vec::new(); roots.len()];
    for item in items {
        if let Some(i) = item["path"].as_str().and_then(|p| root_of(roots, p)) {
            groups[i].push(item);
        }
    }
    groups
        .into_iter()
        .map(|items| {
            let count = items.len();
            let mut tag_counts: HashMap<i64, usize> = HashMap::new();
            for item in &items {
                for tag in item["tags"].as_array().into_iter().flatten() {
                    if let Some(tag) = tag.as_i64() {
                        *tag_counts.entry(tag).or_default() += 1;
                    }
                }
            }
            let mut tags: Vec<i64> = tag_counts
                .into_iter()
                .filter(|(_, n)| count > 0 && n * 2 >= count)
                .map(|(t, _)| t)
                .collect();
            tags.sort();
            Habits {
                count,
                anime: items
                    .iter()
                    .filter(|i| is_anime(&genres(i), language(i).as_deref()))
                    .count(),
                profile: most(items.iter().filter_map(|i| i["qualityProfileId"].as_i64())),
                tags,
                series_type: most(
                    items
                        .iter()
                        .filter_map(|i| i["seriesType"].as_str().map(String::from)),
                ),
                season_folder: most(items.iter().filter_map(|i| i["seasonFolder"].as_bool())),
                minimum_availability: most(
                    items
                        .iter()
                        .filter_map(|i| i["minimumAvailability"].as_str().map(String::from)),
                ),
            }
        })
        .collect()
}

impl Setup {
    /// The root folder for something anime or not: the one with the most
    /// anime in it, or the least.
    pub fn suggest(&self, anime: bool) -> usize {
        let share = |h: &Habits| {
            if h.count == 0 {
                0.5
            } else {
                h.anime as f64 / h.count as f64
            }
        };
        // Ties go to the first folder.
        let mut best: Option<(usize, f64)> = None;
        for (i, h) in self.habits.iter().enumerate() {
            let score = if anime { share(h) } else { -share(h) };
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((i, score));
            }
        }
        best.map_or(0, |(i, _)| i)
    }

    /// A tag's name.
    pub fn tag_label(&self, id: i64) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.label.as_str())
    }

    /// The quality profile usually used in a root folder.
    pub fn profile_for(&self, root: usize) -> Option<i64> {
        self.habits
            .get(root)
            .and_then(|h| h.profile)
            .filter(|p| self.profiles.iter().any(|x| x.id == *p))
            .or_else(|| self.profiles.first().map(|p| p.id))
    }
}

/// Which episodes of a new show to monitor (Sonarr's add options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Monitor {
    All,
    Future,
    FirstSeason,
    LatestSeason,
    None,
}

impl Monitor {
    pub const ALL: [Monitor; 5] = [
        Monitor::All,
        Monitor::Future,
        Monitor::FirstSeason,
        Monitor::LatestSeason,
        Monitor::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Monitor::All => "All episodes",
            Monitor::Future => "Future episodes",
            Monitor::FirstSeason => "First season",
            Monitor::LatestSeason => "Latest season",
            Monitor::None => "Nothing",
        }
    }

    fn api(self) -> &'static str {
        match self {
            Monitor::All => "all",
            Monitor::Future => "future",
            Monitor::FirstSeason => "firstSeason",
            Monitor::LatestSeason => "lastSeason",
            Monitor::None => "none",
        }
    }
}

/// What the viewer chose.
#[derive(Debug, Clone)]
pub struct AddOptions {
    pub root: usize,
    pub profile: i64,
    pub monitor: Monitor,
    /// Search for it right away.
    pub search: bool,
}

/// The body that adds `candidate`: the lookup's answer with the choices,
/// and the root folder's habits for the rest.
pub fn add_body(setup: &Setup, candidate: &Candidate, options: &AddOptions) -> Option<Value> {
    let root = setup.roots.get(options.root)?;
    let habits = setup.habits.get(options.root).cloned().unwrap_or_default();
    let mut body = candidate.raw.clone();
    let object = body.as_object_mut()?;
    object.remove("path");
    object.remove("id");
    object.insert("rootFolderPath".into(), json!(root.path));
    object.insert("qualityProfileId".into(), json!(options.profile));
    object.insert("tags".into(), json!(habits.tags));
    match setup.kind {
        Kind::Sonarr => {
            let monitored = options.monitor != Monitor::None;
            object.insert("monitored".into(), json!(monitored));
            object.insert(
                "monitorNewItems".into(),
                json!(if monitored { "all" } else { "none" }),
            );
            object.insert(
                "seriesType".into(),
                json!(habits.series_type.as_deref().unwrap_or("standard")),
            );
            object.insert(
                "seasonFolder".into(),
                json!(habits.season_folder.unwrap_or(true)),
            );
            object.insert(
                "addOptions".into(),
                json!({
                    "monitor": options.monitor.api(),
                    "searchForMissingEpisodes": options.search && monitored,
                    "searchForCutoffUnmetEpisodes": false,
                }),
            );
        }
        Kind::Radarr => {
            object.insert("monitored".into(), json!(true));
            object.insert(
                "minimumAvailability".into(),
                json!(habits.minimum_availability.as_deref().unwrap_or("released")),
            );
            object.insert(
                "addOptions".into(),
                json!({ "monitor": "movieOnly", "searchForMovie": options.search }),
            );
        }
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<RootFolder> {
        ["/data/media/tvshows", "/data/media/anime-series"]
            .iter()
            .map(|p| RootFolder {
                path: p.to_string(),
                free_space: Some(1 << 40),
            })
            .collect()
    }

    fn show(path: &str, genres: &[&str], language: &str, profile: i64, tags: &[i64]) -> Value {
        json!({"path": path, "genres": genres, "originalLanguage": {"name": language},
               "qualityProfileId": profile, "tags": tags, "seriesType": "standard", "seasonFolder": true})
    }

    #[test]
    fn anime_is_the_genre_or_animated_and_japanese() {
        let g = |g: &[&str]| g.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_anime(&g(&["Animation", "Anime"]), Some("Japanese")));
        assert!(is_anime(&g(&["Animation", "Thriller"]), Some("Japanese")));
        // Japanese live action, and western animation, aren't.
        assert!(!is_anime(&g(&["Drama", "Thriller"]), Some("Japanese")));
        assert!(!is_anime(&g(&["Animation", "Comedy"]), Some("English")));
    }

    #[test]
    fn new_things_go_where_their_kind_already_is() {
        let items = vec![
            show("/data/media/tvshows/A", &["Drama"], "English", 7, &[]),
            show("/data/media/tvshows/B", &["Drama"], "Japanese", 7, &[]),
            show("/data/media/tvshows/C", &["Comedy"], "English", 1, &[2]),
            show(
                "/data/media/anime-series/D",
                &["Anime"],
                "Japanese",
                7,
                &[3],
            ),
            show(
                "/data/media/anime-series/E",
                &["Anime", "Animation"],
                "Japanese",
                1,
                &[3],
            ),
            show("/data/media/anime-series/F", &["Anime"], "Japanese", 1, &[]),
            show("/elsewhere/G", &["Anime"], "Japanese", 1, &[]),
        ];
        let roots = roots();
        let habits = habits(&roots, &items);
        assert_eq!(habits[0].count, 3);
        assert_eq!(habits[0].anime, 0);
        assert_eq!(habits[0].profile, Some(7));
        assert!(habits[0].tags.is_empty());
        assert_eq!(habits[1].count, 3);
        assert_eq!(habits[1].anime, 3);
        assert_eq!(habits[1].profile, Some(1));
        assert_eq!(habits[1].tags, [3]);
        let setup = Setup {
            kind: Kind::Sonarr,
            roots,
            profiles: vec![
                Profile {
                    id: 1,
                    name: "Any".into(),
                },
                Profile {
                    id: 7,
                    name: "default".into(),
                },
            ],
            tags: Vec::new(),
            habits,
        };
        assert_eq!(setup.suggest(true), 1);
        assert_eq!(setup.suggest(false), 0);
        assert_eq!(setup.profile_for(0), Some(7));
    }

    #[test]
    fn the_add_request_carries_the_choices_and_the_folders_habits() {
        let candidate = Candidate::from(
            Kind::Radarr,
            json!({"title": "Perfect Blue", "year": 1998, "tmdbId": 10494,
                   "genres": ["Animation", "Thriller"], "originalLanguage": {"name": "Japanese"},
                   "remotePoster": "https://image.tmdb.org/t/p/original/x.jpg", "folder": "Perfect Blue (1998)"}),
        )
        .unwrap();
        assert!(candidate.is_anime());
        assert_eq!(
            candidate.poster.as_deref(),
            Some("https://image.tmdb.org/t/p/w342/x.jpg")
        );
        let setup = Setup {
            kind: Kind::Radarr,
            roots: vec![
                RootFolder {
                    path: "/data/media/movies".into(),
                    free_space: None,
                },
                RootFolder {
                    path: "/data/media/anime-movies".into(),
                    free_space: None,
                },
            ],
            profiles: vec![Profile {
                id: 7,
                name: "default".into(),
            }],
            tags: Vec::new(),
            habits: vec![
                Habits::default(),
                Habits {
                    count: 4,
                    anime: 4,
                    tags: vec![3],
                    minimum_availability: Some("released".into()),
                    ..Habits::default()
                },
            ],
        };
        let options = AddOptions {
            root: setup.suggest(candidate.is_anime()),
            profile: 7,
            monitor: Monitor::All,
            search: true,
        };
        let body = add_body(&setup, &candidate, &options).unwrap();
        assert_eq!(body["rootFolderPath"], "/data/media/anime-movies");
        assert_eq!(body["tags"], json!([3]));
        assert_eq!(body["qualityProfileId"], 7);
        assert_eq!(body["addOptions"]["searchForMovie"], true);
        assert_eq!(body["tmdbId"], 10494);
    }
}
