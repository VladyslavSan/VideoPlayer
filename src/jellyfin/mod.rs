//! Jellyfin client: login, library listing, artwork, direct-play URLs and
//! playback reporting so resume and watched state stay in sync on the server.

mod client;
pub mod discovery;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use client::{Client, PlaybackReport};

use crate::library::{Item, Source};
use crate::video::{ExternalSubtitle, Segment, SegmentKind};

const TICKS_PER_SECOND: f64 = 10_000_000.0;

pub fn ticks_to_seconds(ticks: i64) -> f64 {
    ticks as f64 / TICKS_PER_SECOND
}

pub fn seconds_to_ticks(seconds: f64) -> i64 {
    (seconds * TICKS_PER_SECOND) as i64
}

/// A signed-in Jellyfin account. The library database stores everything but
/// the token, which lives in the system credential store (see `secrets`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub id: i64,
    pub url: String,
    pub name: String,
    pub server_id: String,
    pub user_id: String,
    pub user_name: String,
    /// Empty if the credential store has no token for this account.
    pub token: String,
}

impl Server {
    /// The credential store entry for this account's token. The device id
    /// keeps installations with separate data folders from sharing entries.
    pub fn token_account(&self, device_id: &str) -> String {
        format!("jellyfin:{device_id}:{}:{}", self.server_id, self.user_id)
    }
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserData {
    #[serde(default)]
    pub playback_position_ticks: i64,
    #[serde(default)]
    pub played: bool,
    pub last_played_date: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfo {
    #[serde(default)]
    pub media_sources: Vec<MediaSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSource {
    pub id: String,
    #[serde(default)]
    pub media_streams: Vec<MediaStream>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    #[serde(rename = "Type")]
    pub kind: String,
    pub index: i64,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub is_external: bool,
}

impl PlaybackInfo {
    /// Subtitle files stored next to the video on the server. Subtitles
    /// inside the video file aren't listed: mpv finds those itself.
    /// `base` is the server address; URLs need the auth header to download.
    pub fn external_subtitles(&self, base: &str, item_id: &str) -> Vec<ExternalSubtitle> {
        let Some(source) = self.media_sources.first() else {
            return Vec::new();
        };
        source
            .media_streams
            .iter()
            .filter(|s| s.kind == "Subtitle" && s.is_external)
            .map(|s| {
                let codec = s.codec.as_deref().unwrap_or("srt").to_ascii_lowercase();
                // Jellyfin converts text formats on request; others are sent as-is.
                let format = match codec.as_str() {
                    "subrip" | "srt" => "srt",
                    "webvtt" | "vtt" => "vtt",
                    "ass" | "ssa" => codec.as_str(),
                    "pgssub" => "sup",
                    other => other,
                };
                ExternalSubtitle {
                    url: format!(
                        "{base}/Videos/{item_id}/{}/Subtitles/{}/Stream.{format}",
                        source.id, s.index
                    ),
                    title: s.title.clone(),
                    lang: s.language.clone().filter(|l| l != "und"),
                }
            })
            .collect()
    }
}

/// Intro/credits markers (Jellyfin 10.10+, filled in by plugins such as
/// Intro Skipper).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSegments {
    #[serde(default)]
    pub items: Vec<MediaSegment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSegment {
    #[serde(rename = "Type")]
    pub kind: String,
    pub start_ticks: i64,
    pub end_ticks: i64,
}

impl MediaSegments {
    pub fn to_segments(&self) -> Vec<Segment> {
        self.items
            .iter()
            .filter_map(|s| {
                let kind = match s.kind.as_str() {
                    "Intro" => SegmentKind::Intro,
                    "Recap" => SegmentKind::Recap,
                    "Preview" => SegmentKind::Preview,
                    "Outro" => SegmentKind::Credits,
                    _ => return None,
                };
                let (start, end) = (
                    ticks_to_seconds(s.start_ticks),
                    ticks_to_seconds(s.end_ticks),
                );
                (end > start).then_some(Segment { kind, start, end })
            })
            .collect()
    }
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

/// Saves the library last fetched from a server, so it can be shown at once
/// on the next start, and when the server can't be reached.
pub fn save_library_cache(path: &Path, items: &[BaseItem]) -> Result<()> {
    // Write then rename, so a crash never leaves a half-written cache.
    let partial = path.with_extension("part");
    std::fs::write(&partial, serde_json::to_vec(items)?)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

pub fn load_library_cache(path: &Path) -> Result<Vec<BaseItem>> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
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

/// Accepts what people type or paste: `jellyfin.local:8096`, `https://host/jf/`,
/// or a web-app link such as `http://host:8096/web/#/video`.
pub fn normalize_url(input: &str) -> String {
    let without_fragment = input.trim().split(['#', '?']).next().unwrap_or_default();
    let mut trimmed = without_fragment.trim_end_matches('/');
    for suffix in ["/web/index.html", "/web"] {
        if let Some(base) = trimmed.strip_suffix(suffix) {
            trimmed = base.trim_end_matches('/');
            break;
        }
    }
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
    fn external_subtitles_get_download_urls() {
        let info: PlaybackInfo = serde_json::from_str(
            r#"{"MediaSources": [{"Id": "src1", "MediaStreams": [
                {"Type": "Video", "Index": 0, "Codec": "hevc"},
                {"Type": "Subtitle", "Index": 2, "Codec": "subrip", "Language": "eng", "IsExternal": false},
                {"Type": "Subtitle", "Index": 3, "Codec": "subrip", "Language": "ukr", "Title": "Full", "IsExternal": true},
                {"Type": "Subtitle", "Index": 4, "Codec": "ass", "Language": "und", "IsExternal": true}
            ]}]}"#,
        )
        .unwrap();
        assert_eq!(
            info.external_subtitles("http://nas:8096", "item1"),
            vec![
                ExternalSubtitle {
                    url: "http://nas:8096/Videos/item1/src1/Subtitles/3/Stream.srt".into(),
                    title: Some("Full".into()),
                    lang: Some("ukr".into()),
                },
                ExternalSubtitle {
                    url: "http://nas:8096/Videos/item1/src1/Subtitles/4/Stream.ass".into(),
                    title: None,
                    lang: None,
                },
            ]
        );
    }

    #[test]
    fn library_cache_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("jellyfin-1.json");
        let items: Vec<BaseItem> =
            vec![serde_json::from_str(EPISODE)?, serde_json::from_str(MOVIE)?];
        save_library_cache(&path, &items)?;
        let loaded = load_library_cache(&path)?;
        let thumbs = Path::new("C:/thumbs");
        let as_items = |list: &[BaseItem]| -> Vec<Item> {
            list.iter().map(|b| b.to_item(1, thumbs)).collect()
        };
        assert_eq!(as_items(&loaded), as_items(&items));
        assert_eq!(loaded[0].card_image(), items[0].card_image());
        Ok(())
    }

    #[test]
    fn media_segments() {
        let parsed: MediaSegments = serde_json::from_str(
            r#"{"Items": [
                {"Id": "a", "ItemId": "i", "Type": "Intro", "StartTicks": 600000000, "EndTicks": 1500000000},
                {"Id": "b", "ItemId": "i", "Type": "Commercial", "StartTicks": 0, "EndTicks": 10},
                {"Id": "c", "ItemId": "i", "Type": "Outro", "StartTicks": 25000000000, "EndTicks": 26000000000}
            ], "TotalRecordCount": 3, "StartIndex": 0}"#,
        )
        .unwrap();
        assert_eq!(
            parsed.to_segments(),
            vec![
                Segment {
                    kind: SegmentKind::Intro,
                    start: 60.0,
                    end: 150.0
                },
                Segment {
                    kind: SegmentKind::Credits,
                    start: 2500.0,
                    end: 2600.0
                },
            ]
        );
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
        assert_eq!(
            normalize_url("http://moto-moto-srv.local:8096/web/#/video"),
            "http://moto-moto-srv.local:8096"
        );
        assert_eq!(
            normalize_url("https://example.com/jf/web/index.html#/home.html"),
            "https://example.com/jf"
        );
    }
}
