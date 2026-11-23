//! Everything Sonarr or Radarr keeps track of: its shows or movies, with
//! how much of each is downloaded.

use serde_json::Value;

use super::Kind;

/// How much of it is downloaded.
#[derive(Debug, Clone, PartialEq)]
pub enum Have {
    /// Episodes on disk, of those it's after (monitored, aired); and all
    /// it knows of.
    Episodes { files: u32, wanted: u32, total: u32 },
    Movie {
        file: bool,
        /// Available, by its minimum availability (out on digital or disc,
        /// say): what Radarr waits for before it looks by itself.
        released: bool,
        /// Its cinema date has passed.
        in_cinemas: bool,
    },
}

/// Which picture of it to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Art {
    /// Wide, like the library's cards.
    Fanart,
    /// For the few with no fanart.
    Poster,
    /// The fanart at full size, behind the title on its page.
    Backdrop,
}

impl Art {
    /// The app's resized copy of it.
    pub fn file(self) -> &'static str {
        match self {
            Art::Fanart => "fanart-360.jpg",
            Art::Poster => "poster-250.jpg",
            Art::Backdrop => "fanart.jpg",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tracked {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i32>,
    /// Whether the app is looking for what it hasn't got.
    pub monitored: bool,
    pub have: Have,
    /// The picture, and its address in the app, which changes along with
    /// it (`/MediaCover/115/fanart.jpg?lastWrite=…`).
    pub art: Option<(Art, String)>,
    /// Its page in the app's web interface: `/series/<slug>`, `/movie/<slug>`.
    pub slug: Option<String>,
    pub overview: Option<String>,
    pub genres: Vec<String>,
    /// An episode's or the movie's length.
    pub runtime_minutes: Option<u32>,
    /// "Continuing", "Ended", "Released", …
    pub state: Option<String>,
}

impl Tracked {
    /// Something it's after and hasn't got.
    pub fn missing(&self) -> bool {
        self.monitored
            && match self.have {
                Have::Episodes { files, wanted, .. } => files < wanted,
                Have::Movie { file, released, .. } => released && !file,
            }
    }

    /// "6 of 8 episodes", "Downloaded", "Not released yet".
    pub fn status(&self) -> String {
        let mut text = match self.have {
            Have::Episodes {
                files,
                wanted,
                total,
            } => match (wanted, total) {
                (0, 0) => "No episodes yet".to_string(),
                (0, _) if files == 0 => "Nothing downloaded".to_string(),
                (0, _) => plural(files, "episode"),
                _ if files == 1 && wanted == 1 => "1 episode".to_string(),
                _ if files >= wanted => format!("All {files} episodes"),
                _ => format!("{files} of {wanted} episodes"),
            },
            Have::Movie { file: true, .. } => "Downloaded".into(),
            Have::Movie {
                released: false,
                in_cinemas: true,
                ..
            } => "In cinemas".into(),
            Have::Movie {
                released: false, ..
            } => "Not released yet".into(),
            Have::Movie { .. } if self.monitored => "Missing".into(),
            Have::Movie { .. } => "Not downloaded".into(),
        };
        // Only worth saying when something isn't here.
        let complete = match self.have {
            Have::Episodes { files, wanted, .. } => files > 0 && files >= wanted,
            Have::Movie { file, .. } => file,
        };
        if !self.monitored && !complete {
            text.push_str(" · not monitored");
        }
        text
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn plural(n: u32, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// `/series` or `/movie`'s answer, sorted the app's way (by its sort title).
pub fn tracked(kind: Kind, items: Vec<Value>) -> Vec<Tracked> {
    let mut list: Vec<Tracked> = items.iter().filter_map(|v| read(kind, v)).collect();
    list.sort_by(|a, b| a.sort_title.cmp(&b.sort_title).then(a.year.cmp(&b.year)));
    list
}

fn read(kind: Kind, v: &Value) -> Option<Tracked> {
    let id = v["id"].as_i64()?;
    let title = v["title"].as_str()?.to_string();
    let count = |key: &str| v["statistics"][key].as_u64().unwrap_or(0) as u32;
    let have = match kind {
        Kind::Sonarr => Have::Episodes {
            files: count("episodeFileCount"),
            wanted: count("episodeCount"),
            total: count("totalEpisodeCount"),
        },
        Kind::Radarr => Have::Movie {
            file: v["hasFile"].as_bool().unwrap_or(false),
            released: v["isAvailable"].as_bool().unwrap_or(true),
            in_cinemas: v["inCinemas"]
                .as_str()
                .and_then(super::parse_utc)
                .is_some_and(|at| at <= now()),
        },
    };
    let image = |cover: &str| {
        v["images"].as_array()?.iter().find_map(|i| {
            (i["coverType"] == cover)
                .then(|| i["url"].as_str().map(String::from))
                .flatten()
        })
    };
    let art = image("fanart")
        .map(|url| (Art::Fanart, url))
        .or_else(|| image("poster").map(|url| (Art::Poster, url)));
    Some(Tracked {
        id,
        sort_title: v["sortTitle"]
            .as_str()
            .map_or_else(|| title.to_lowercase(), String::from),
        title,
        year: v["year"]
            .as_i64()
            .filter(|y| *y > 0)
            .and_then(|y| i32::try_from(y).ok()),
        monitored: v["monitored"].as_bool().unwrap_or(false),
        have,
        art,
        slug: v["titleSlug"].as_str().map(String::from),
        overview: v["overview"]
            .as_str()
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .map(String::from),
        genres: v["genres"]
            .as_array()
            .map(|g| {
                g.iter()
                    .filter_map(|g| g.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        runtime_minutes: v["runtime"]
            .as_u64()
            .filter(|m| *m > 0)
            .and_then(|m| u32::try_from(m).ok()),
        state: v["status"].as_str().map(|s| {
            let mut c = s.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_shows() {
        let list = tracked(
            Kind::Sonarr,
            vec![
                json!({"id": 115, "title": "The Show", "sortTitle": "show", "year": 2011,
                       "monitored": true, "titleSlug": "the-show", "overview": " A show. ",
                       "genres": ["Drama", "Anime"], "runtime": 24, "status": "continuing",
                       "statistics": {"episodeFileCount": 4, "episodeCount": 6, "totalEpisodeCount": 34},
                       "images": [{"coverType": "poster", "url": "/MediaCover/115/poster.jpg?lastWrite=1"},
                                  {"coverType": "fanart", "url": "/MediaCover/115/fanart.jpg?lastWrite=2"}]}),
                json!({"id": 7, "title": "Another", "sortTitle": "another", "year": 0,
                       "monitored": false,
                       "statistics": {"episodeFileCount": 0, "episodeCount": 0, "totalEpisodeCount": 10},
                       "images": [{"coverType": "poster", "url": "/MediaCover/7/poster.jpg"}]}),
            ],
        );
        assert_eq!(list[0].title, "Another");
        assert_eq!(list[0].year, None);
        assert_eq!(list[0].status(), "Nothing downloaded · not monitored");
        assert_eq!(
            list[0].art,
            Some((Art::Poster, "/MediaCover/7/poster.jpg".into()))
        );
        assert!(!list[0].missing());
        let show = &list[1];
        assert_eq!(show.status(), "4 of 6 episodes");
        assert!(show.missing());
        assert_eq!(show.art.as_ref().map(|a| a.0), Some(Art::Fanart));
        assert_eq!(show.slug.as_deref(), Some("the-show"));
        assert_eq!(show.overview.as_deref(), Some("A show."));
        assert_eq!(show.genres, ["Drama", "Anime"]);
        assert_eq!(show.runtime_minutes, Some(24));
        assert_eq!(show.state.as_deref(), Some("Continuing"));
        assert_eq!(list[0].overview, None);
    }

    #[test]
    fn a_movie_in_cinemas_says_so() {
        let movie = |cinemas: &str| {
            tracked(
                Kind::Radarr,
                vec![
                    json!({"id": 1, "title": "Film", "hasFile": false, "isAvailable": false,
                            "monitored": true, "inCinemas": cinemas, "images": []}),
                ],
            )
            .remove(0)
        };
        assert_eq!(movie("2020-01-01T00:00:00Z").status(), "In cinemas");
        assert_eq!(movie("2999-01-01T00:00:00Z").status(), "Not released yet");
    }

    #[test]
    fn reads_movies() {
        let movie = |file: bool, available: bool, monitored: bool| {
            tracked(
                Kind::Radarr,
                vec![
                    json!({"id": 1, "title": "Film", "year": 2020, "hasFile": file,
                            "isAvailable": available, "monitored": monitored, "images": []}),
                ],
            )
            .remove(0)
        };
        assert_eq!(movie(true, true, true).status(), "Downloaded");
        assert_eq!(movie(true, true, false).status(), "Downloaded");
        assert!(movie(false, true, true).missing());
        assert_eq!(movie(false, true, true).status(), "Missing");
        assert_eq!(movie(false, false, true).status(), "Not released yet");
        assert!(!movie(false, false, true).missing());
        assert_eq!(
            movie(false, true, false).status(),
            "Not downloaded · not monitored"
        );
        assert_eq!(movie(true, true, true).art, None);
    }
}
