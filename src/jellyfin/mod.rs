//! Jellyfin client: login, library listing, artwork, direct-play URLs and
//! playback reporting so resume and watched state stay in sync on the server.

mod client;
pub mod discovery;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use client::{Client, PlaybackReport};

use crate::library::{Item, Source};

const TICKS_PER_SECOND: f64 = 10_000_000.0;

pub fn ticks_to_seconds(ticks: i64) -> f64 {
    ticks as f64 / TICKS_PER_SECOND
}

pub fn seconds_to_ticks(seconds: f64) -> i64 {
    (seconds * TICKS_PER_SECOND) as i64
}

/// A signed-in Jellyfin account, as stored in the library database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub id: i64,
    pub url: String,
    pub name: String,
    pub server_id: String,
    pub user_id: String,
    pub user_name: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicInfo {
    pub server_name: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthResult {
    pub access_token: String,
    pub user: User,
    pub server_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsPage {
    pub items: Vec<BaseItem>,
    pub total_record_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItem {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub kind: String,
    pub production_year: Option<u16>,
    pub series_name: Option<String>,
    pub parent_index_number: Option<u16>,
    pub index_number: Option<u16>,
    pub run_time_ticks: Option<i64>,
    #[serde(default)]
    pub image_tags: HashMap<String, String>,
    #[serde(default)]
    pub backdrop_image_tags: Vec<String>,
    #[serde(default)]
    pub parent_backdrop_image_tags: Vec<String>,
    pub parent_backdrop_item_id: Option<String>,
    pub user_data: Option<UserData>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserData {
    #[serde(default)]
    pub playback_position_ticks: i64,
    #[serde(default)]
    pub played: bool,
    pub last_played_date: Option<String>,
}

/// A piece of server artwork to download for an item's card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub item_id: String,
    pub kind: &'static str,
    pub index: usize,
    pub tag: String,
}

impl ImageRef {
    /// Tags change whenever the image does, so the file name doubles as a cache key.
    pub fn cache_path(&self, dir: &Path) -> PathBuf {
        dir.join(format!("jf-{}-{}.jpg", self.kind.to_lowercase(), self.tag))
    }
}

impl BaseItem {
    /// Picks 16:9 artwork: an episode still, or a movie's thumb or backdrop.
    pub fn card_image(&self) -> Option<ImageRef> {
        let own = |kind: &'static str| {
            self.image_tags.get(kind).map(|tag| ImageRef {
                item_id: self.id.clone(),
                kind,
                index: 0,
                tag: tag.clone(),
            })
        };
        let backdrop = self.backdrop_image_tags.first().map(|tag| ImageRef {
            item_id: self.id.clone(),
            kind: "Backdrop",
            index: 0,
            tag: tag.clone(),
        });
        let parent_backdrop = self
            .parent_backdrop_item_id
            .as_ref()
            .zip(self.parent_backdrop_image_tags.first())
            .map(|(id, tag)| ImageRef {
                item_id: id.clone(),
                kind: "Backdrop",
                index: 0,
                tag: tag.clone(),
            });
        if self.kind == "Episode" {
            own("Primary").or(parent_backdrop)
        } else {
            own("Thumb")
                .or(backdrop)
                .or(parent_backdrop)
                .or_else(|| own("Primary"))
        }
    }

    /// Converts to a library item. `thumbs` is where downloaded artwork lives.
    pub fn to_item(&self, server: i64, thumbs: &Path) -> Item {
        let user = self.user_data.clone().unwrap_or_default();
        let is_episode = self.kind == "Episode";
        Item {
            source: Source::Jellyfin {
                server,
                id: self.id.clone(),
            },
            title: self.name.clone(),
            year: self.production_year.filter(|_| !is_episode),
            show: self.series_name.clone().filter(|_| is_episode),
            season: self.parent_index_number.filter(|_| is_episode),
            episode: self.index_number.filter(|_| is_episode),
            duration: self.run_time_ticks.map(ticks_to_seconds),
            position: ticks_to_seconds(user.playback_position_ticks),
            watched: user.played,
            last_played: user.last_played_date.as_deref().and_then(parse_timestamp),
            thumb: self
                .card_image()
                .map(|img| img.cache_path(thumbs))
                .filter(|p| p.exists()),
        }
    }
}

/// Parses the `2026-11-07T18:11:04.4168678Z` timestamps Jellyfin returns
/// into Unix seconds. Fractions and the zone suffix are ignored (always UTC).
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let num = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3_600 + mi * 60 + sec)
}

/// Accepts what people type: `jellyfin.local:8096`, `https://host/jf/`, ...
pub fn normalize_url(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real response of the Jellyfin 12.4 demo server.
    const EPISODE: &str = r#"{
        "Id": "f717d85df782816c2ab75423e32f740b", "Name": "Earthfall", "Type": "Episode",
        "ProductionYear": 2010, "SeriesName": "Pioneer One", "SeriesId": "9bd5926326d651153928e0d11c6ee7cb",
        "ParentIndexNumber": 1, "IndexNumber": 1, "RunTimeTicks": 18963580000,
        "ImageTags": {"Primary": "745a7a9a17c6332bda53814ec489f281"}, "BackdropImageTags": [],
        "ParentBackdropImageTags": ["970367b9a680d18ccf8502ae3e111b92"],
        "ParentBackdropItemId": "9bd5926326d651153928e0d11c6ee7cb",
        "UserData": {"PlayedPercentage": 46.08, "PlaybackPositionTicks": 8738670000, "PlayCount": 13,
                     "IsFavorite": false, "LastPlayedDate": "2026-11-07T18:11:04.4168678Z", "Played": true}
    }"#;

    const MOVIE: &str = r#"{
        "Id": "m1", "Name": "Sintel", "Type": "Movie", "ProductionYear": 2010, "RunTimeTicks": 8880000000,
        "ImageTags": {"Primary": "p1"}, "BackdropImageTags": ["b1"]
    }"#;

    #[test]
    fn episode_maps_to_item() {
        let base: BaseItem = serde_json::from_str(EPISODE).unwrap();
        let item = base.to_item(7, Path::new("C:/thumbs"));
        assert_eq!(item.display_title(), "Pioneer One · S1 · E1");
        assert_eq!(item.title, "Earthfall");
        assert_eq!(item.year, None);
        assert!((item.duration.unwrap() - 1896.358).abs() < 1e-6);
        assert!((item.position - 873.867).abs() < 1e-6);
        assert!(item.watched);
        assert_eq!(item.last_played, Some(1_794_075_064));
        assert_eq!(
            item.source,
            Source::Jellyfin {
                server: 7,
                id: "f717d85df782816c2ab75423e32f740b".into()
            }
        );
        let img = base.card_image().unwrap();
        assert_eq!(
            (img.kind, img.tag.as_str()),
            ("Primary", "745a7a9a17c6332bda53814ec489f281")
        );
    }

    #[test]
    fn movie_prefers_backdrop_over_poster() {
        let base: BaseItem = serde_json::from_str(MOVIE).unwrap();
        let img = base.card_image().unwrap();
        assert_eq!((img.kind, img.tag.as_str()), ("Backdrop", "b1"));
        let item = base.to_item(1, Path::new("C:/thumbs"));
        assert_eq!(
            (item.year, item.show.clone(), item.position),
            (Some(2010), None, 0.0)
        );
        assert!(!item.watched);
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2000-03-01T00:00:00.5Z"), Some(951_868_800));
        assert_eq!(parse_timestamp("garbage"), None);
    }

    #[test]
    fn urls() {
        assert_eq!(
            normalize_url(" jellyfin.local:8096/ "),
            "http://jellyfin.local:8096"
        );
        assert_eq!(
            normalize_url("https://example.com/jf/"),
            "https://example.com/jf"
        );
    }
}
