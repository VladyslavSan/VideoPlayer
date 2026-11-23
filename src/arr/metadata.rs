//! What's known of a show or movie before it's added: a show's episodes,
//! from Skyhook (the metadata service Sonarr itself uses, so seasons and
//! episodes are numbered as Sonarr will number them), and the series of
//! films a movie belongs to, from Radarr.

use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::Value;
use ureq::Agent;

/// Where Sonarr gets a show's details, by TVDB id.
const SKYHOOK: &str = "https://skyhook.sonarr.tv/v1/tvdb/shows/en";

/// An episode of a show.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaEpisode {
    pub season: u32,
    pub number: u32,
    pub title: Option<String>,
    /// `2023-09-29`.
    pub air_date: Option<String>,
    pub runtime: Option<u32>,
    pub overview: Option<String>,
}

/// Skyhook's address; a test run can point it at a fake
/// (`VIDEO_PLAYER_SKYHOOK`).
fn skyhook() -> String {
    std::env::var("VIDEO_PLAYER_SKYHOOK")
        .ok()
        .filter(|_| crate::test_window::enabled())
        .unwrap_or_else(|| SKYHOOK.to_string())
}

/// The show's episodes, in order. Only its TVDB id is sent.
pub fn show_episodes(tvdb: i64) -> Result<Vec<MetaEpisode>> {
    if tvdb <= 0 {
        bail!("no TVDB id");
    }
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let bytes = agent
        .get(&format!("{}/{tvdb}", skyhook()))
        .header("Accept", "application/json")
        .header("User-Agent", "VideoPlayer")
        .call()?
        .body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_vec()?;
    Ok(episodes(&serde_json::from_slice(&bytes)?))
}

/// A number given as a number or as text.
fn number(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.trim().parse().ok())
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn episodes(show: &Value) -> Vec<MetaEpisode> {
    let mut list: Vec<MetaEpisode> = show["episodes"]
        .as_array()
        .map(|eps| {
            eps.iter()
                .filter_map(|e| {
                    Some(MetaEpisode {
                        season: u32::try_from(number(&e["seasonNumber"])?).ok()?,
                        number: u32::try_from(number(&e["episodeNumber"])?).ok()?,
                        title: text(&e["title"]),
                        air_date: text(&e["airDate"]).and_then(|d| d.get(..10).map(String::from)),
                        runtime: number(&e["runtime"])
                            .filter(|m| *m > 0)
                            .and_then(|m| u32::try_from(m).ok()),
                        overview: text(&e["overview"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    list.sort_by_key(|e| (e.season, e.number));
    list.dedup_by_key(|e| (e.season, e.number));
    list
}

/// A series of films, as Radarr knows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub title: String,
    /// In order of release.
    pub movies: Vec<CollectionMovie>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollectionMovie {
    pub tmdb: i64,
    pub title: String,
    pub year: Option<i32>,
    pub poster: Option<String>,
    /// Radarr has it.
    pub in_radarr: bool,
}

/// `/collection?tmdbId=`'s answer: the collection, if Radarr knows it
/// (it does once one of its films is added).
pub(super) fn collection(found: &[Value]) -> Option<Collection> {
    let c = found.first()?;
    let mut movies: Vec<CollectionMovie> = c["movies"]
        .as_array()?
        .iter()
        .filter_map(|m| {
            let poster = m["images"].as_array().and_then(|images| {
                images.iter().find_map(|i| {
                    (i["coverType"] == "poster")
                        .then(|| text(&i["remoteUrl"]))
                        .flatten()
                })
            });
            Some(CollectionMovie {
                tmdb: m["tmdbId"].as_i64().filter(|id| *id > 0)?,
                title: text(&m["title"])?,
                year: m["year"]
                    .as_i64()
                    .filter(|y| *y > 0)
                    .and_then(|y| i32::try_from(y).ok()),
                poster: poster.map(|p| super::add::poster_size(&p)),
                in_radarr: m["isExisting"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    // Unannounced ones (no year yet) last.
    movies.sort_by_key(|m| (m.year.is_none(), m.year));
    Some(Collection {
        title: text(&c["title"])?,
        movies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_episodes_in_order() {
        let show = json!({"episodes": [
            {"seasonNumber": 1, "episodeNumber": 2, "title": "Second", "airDate": "2023-10-06", "runtime": 25},
            {"seasonNumber": "1", "episodeNumber": "1", "title": " First ", "airDate": "2023-09-29", "runtime": "25", "overview": "It starts."},
            {"seasonNumber": 0, "episodeNumber": 1, "title": "Special", "runtime": 0},
            {"seasonNumber": 1, "episodeNumber": 1, "title": "Again"},
            {"title": "No numbers"}
        ]});
        let list = episodes(&show);
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].season, list[0].number), (0, 1));
        assert_eq!(list[0].runtime, None);
        assert_eq!(list[1].title.as_deref(), Some("First"));
        assert_eq!(list[1].air_date.as_deref(), Some("2023-09-29"));
        assert_eq!(list[1].runtime, Some(25));
        assert_eq!(list[2].number, 2);
    }

    #[test]
    fn reads_a_collection() {
        let found = vec![json!({"title": "Dune Collection", "movies": [
            {"tmdbId": 3, "title": "Dune: Part Three", "year": 0, "isExisting": false},
            {"tmdbId": 2, "title": "Dune: Part Two", "year": 2024, "isExisting": true,
             "images": [{"coverType": "poster", "remoteUrl": "https://image.tmdb.org/t/p/original/a.jpg"}]},
            {"tmdbId": 1, "title": "Dune", "year": 2021, "isExisting": true}
        ]})];
        let c = collection(&found).unwrap();
        assert_eq!(c.title, "Dune Collection");
        let titles: Vec<_> = c.movies.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["Dune", "Dune: Part Two", "Dune: Part Three"]);
        assert_eq!(
            c.movies[1].poster.as_deref(),
            Some("https://image.tmdb.org/t/p/w342/a.jpg")
        );
        assert!(collection(&[]).is_none());
    }
}
