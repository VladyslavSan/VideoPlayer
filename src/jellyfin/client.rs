use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ureq::{Agent, ResponseExt};

use super::{
    AuthResult, BaseItem, ImageRef, InfoPage, ItemInfo, ItemsPage, MediaSegments, PlaybackInfo,
    PublicInfo, Server, UserData, seconds_to_ticks, ticks_to_seconds,
};
use crate::introdb;
use crate::video::Segment;

const CLIENT_NAME: &str = "VideoPlayer";
const PAGE_SIZE: usize = 500;
/// Asking for an item's resume point mustn't hold up playback for long.
const USER_DATA_TIMEOUT: Duration = Duration::from_secs(3);
const IMAGE_WIDTH: u32 = 640;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackReport {
    Started,
    Progress,
    Stopped,
}

#[derive(Clone)]
pub struct Client {
    agent: Agent,
    base: String,
    device_id: String,
    token: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Login<'a> {
    username: &'a str,
    pw: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PlaybackReportBody<'a> {
    item_id: &'a str,
    position_ticks: i64,
    is_paused: bool,
    can_seek: bool,
    play_method: &'static str,
    play_session_id: &'a str,
}

/// One playback of an item: the server ties reports and transcoding to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaySession {
    pub id: String,
    pub transcoding: bool,
}

impl Client {
    pub fn new(base: &str, device_id: &str) -> Self {
        let agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            base: base.trim_end_matches('/').to_string(),
            device_id: device_id.to_string(),
            token: None,
        }
    }

    pub fn for_server(server: &Server, device_id: &str) -> Self {
        let mut client = Self::new(&server.url, device_id);
        client.token = Some(server.token.clone());
        client
    }

    fn authorization(&self) -> String {
        let device = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".into());
        let mut value = format!(
            r#"MediaBrowser Client="{CLIENT_NAME}", Device="{device}", DeviceId="{}", Version="{}""#,
            self.device_id,
            env!("CARGO_PKG_VERSION"),
        );
        if let Some(token) = &self.token {
            value.push_str(&format!(r#", Token="{token}""#));
        }
        value
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn check(response: &ureq::http::Response<ureq::Body>, what: &str) -> Result<()> {
        match response.status().as_u16() {
            200..=299 => Ok(()),
            401 => bail!("{what}: wrong username or password, or the session expired"),
            404 => bail!("{what}: not found; is this a Jellyfin server?"),
            code => bail!("{what}: server returned HTTP {code}"),
        }
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let mut request = self
            .agent
            .get(&self.url(path))
            .header("Authorization", &self.authorization());
        for (key, value) in query {
            request = request.query(key, value);
        }
        let mut response = request
            .call()
            .with_context(|| format!("connecting to {}", self.base))?;
        Self::check(&response, path)?;
        Ok(response.body_mut().read_json()?)
    }

    /// Checks that the address is a Jellyfin server. If the server redirects
    /// (typically http -> https), the client switches to the final address,
    /// because redirects would drop the body of later POST requests.
    pub fn public_info(&mut self) -> Result<PublicInfo> {
        const PATH: &str = "/System/Info/Public";
        let mut response = self
            .agent
            .get(&self.url(PATH))
            .call()
            .with_context(|| format!("connecting to {}", self.base))?;
        Self::check(&response, PATH)
            .with_context(|| format!("{} doesn't look like a Jellyfin server", self.base))?;
        if let Some(base) = base_from_final(&response.get_uri().to_string(), PATH) {
            self.base = base;
        }
        Ok(response.body_mut().read_json()?)
    }

    /// The server address in use, after any redirect.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn login(&mut self, username: &str, password: &str) -> Result<AuthResult> {
        let mut response = self
            .agent
            .post(&self.url("/Users/AuthenticateByName"))
            .header("Authorization", &self.authorization())
            .send_json(Login {
                username,
                pw: password,
            })
            .with_context(|| format!("connecting to {}", self.base))?;
        Self::check(&response, "Sign in")?;
        let auth: AuthResult = response.body_mut().read_json()?;
        self.token = Some(auth.access_token.clone());
        Ok(auth)
    }

    pub fn logout(&self) -> Result<()> {
        let response = self
            .agent
            .post(&self.url("/Sessions/Logout"))
            .header("Authorization", &self.authorization())
            .send_empty()?;
        Self::check(&response, "Sign out")
    }

    /// Every movie and episode the user can see, with their watch state.
    pub fn library(&self, user_id: &str) -> Result<Vec<BaseItem>> {
        let mut items = Vec::new();
        loop {
            let query = [
                ("userId", user_id.to_string()),
                ("Recursive", "true".into()),
                ("IncludeItemTypes", "Movie,Episode".into()),
                ("Fields", "ParentBackdropItemId,DateCreated".into()),
                ("EnableUserData", "true".into()),
                ("SortBy", "SortName".into()),
                ("StartIndex", items.len().to_string()),
                ("Limit", PAGE_SIZE.to_string()),
            ];
            let page: ItemsPage = self.get_json("/Items", &query)?;
            let received = page.items.len();
            items.extend(page.items);
            if received == 0 || items.len() >= page.total_record_count {
                return Ok(items);
            }
        }
    }

    pub fn download_image(&self, image: &ImageRef, out: &Path) -> Result<()> {
        self.download_image_width(image, out, IMAGE_WIDTH)
    }

    /// Downloads artwork scaled to at most `width` pixels across.
    pub fn download_image_width(&self, image: &ImageRef, out: &Path, width: u32) -> Result<()> {
        let path = format!(
            "/Items/{}/Images/{}/{}",
            image.item_id, image.kind, image.index
        );
        let mut response = self
            .agent
            .get(&self.url(&path))
            .header("Authorization", &self.authorization())
            .query("tag", &image.tag)
            .query("maxWidth", width.to_string())
            .query("quality", "85")
            .call()?;
        Self::check(&response, &path)?;
        let bytes = response.body_mut().read_to_vec()?;
        // Write then rename, so a half-written file never looks cached.
        let partial = out.with_extension("part");
        fs::write(&partial, bytes)?;
        fs::rename(partial, out)?;
        Ok(())
    }

    /// Overview, genres, ratings and stream names, for a details page.
    /// The user's play state for one item, straight from the server. Asked
    /// right before playback starts, so it gives up quickly.
    pub fn user_data(&self, user_id: &str, item_id: &str) -> Result<UserData> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Page {
            items: Vec<Entry>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Entry {
            user_data: Option<UserData>,
        }
        let path = "/Items";
        let mut response = self
            .agent
            .get(&self.url(path))
            .config()
            .timeout_global(Some(USER_DATA_TIMEOUT))
            .build()
            .header("Authorization", &self.authorization())
            .query("ids", item_id)
            .query("userId", user_id)
            .call()
            .with_context(|| format!("connecting to {}", self.base))?;
        Self::check(&response, path)?;
        let page: Page = response.body_mut().read_json()?;
        page.items
            .into_iter()
            .next()
            .and_then(|e| e.user_data)
            .with_context(|| format!("no play state for item {item_id}"))
    }

    pub fn item_info(&self, user_id: &str, item_id: &str) -> Result<ItemInfo> {
        let page: InfoPage = self.get_json(
            "/Items",
            &[
                ("ids", item_id.to_string()),
                ("userId", user_id.to_string()),
                ("Fields", "Overview,Genres,Taglines,MediaStreams".into()),
            ],
        )?;
        page.items
            .into_iter()
            .next()
            .with_context(|| format!("item {item_id} not found"))
    }

    /// Marks an item watched or unwatched for the user.
    pub fn set_played(&self, user_id: &str, item_id: &str, played: bool) -> Result<()> {
        let url = self.url(&format!("/UserPlayedItems/{item_id}"));
        let response = if played {
            self.agent
                .post(&url)
                .query("userId", user_id)
                .header("Authorization", &self.authorization())
                .send_empty()?
        } else {
            self.agent
                .delete(&url)
                .query("userId", user_id)
                .header("Authorization", &self.authorization())
                .call()?
        };
        Self::check(&response, "Marking as watched")
    }

    /// External subtitle files for an item, ready to hand to mpv.
    /// The item's media streams, for subtitle files.
    pub fn playback_info(&self, user_id: &str, item_id: &str) -> Result<PlaybackInfo> {
        self.get_json(
            &format!("/Items/{item_id}/PlaybackInfo"),
            &[("userId", user_id.to_string())],
        )
    }

    /// An HLS stream transcoded by the server to fit `bitrate` (bits/s).
    /// The server picks codecs it can produce from the lists, and copies
    /// the audio when it already fits.
    pub fn transcode_url(&self, item_id: &str, session: &PlaySession, bitrate: u32) -> String {
        let audio_bitrate = 192_000.min(bitrate / 4);
        let query = [
            ("MediaSourceId", item_id.to_string()),
            ("DeviceId", self.device_id.clone()),
            ("PlaySessionId", session.id.clone()),
            ("VideoCodec", "hevc,h264".into()),
            ("AudioCodec", "aac,ac3,eac3,opus,flac,mp3".into()),
            ("MaxStreamingBitrate", bitrate.to_string()),
            ("VideoBitrate", (bitrate - audio_bitrate).to_string()),
            ("AudioBitrate", audio_bitrate.to_string()),
            ("TranscodingMaxAudioChannels", "6".into()),
            ("SegmentContainer", "ts".into()),
            ("MinSegments", "1".into()),
            ("BreakOnNonKeyFrames", "true".into()),
        ];
        let query: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
        self.url(&format!(
            "/Videos/{item_id}/master.m3u8?{}",
            query.join("&")
        ))
    }

    /// Ends the server's transcoding for a play session.
    pub fn stop_transcoding(&self, session: &PlaySession) -> Result<()> {
        let response = self
            .agent
            .delete(&self.url("/Videos/ActiveEncodings"))
            .query("deviceId", &self.device_id)
            .query("playSessionId", &session.id)
            .header("Authorization", &self.authorization())
            .call()?;
        Self::check(&response, "Stopping transcoding")
    }

    /// Intro and credits markers, if the server has any for the item.
    pub fn media_segments(&self, item_id: &str) -> Result<Vec<Segment>> {
        let segments: MediaSegments = self.get_json(&format!("/MediaSegments/{item_id}"), &[])?;
        Ok(segments.to_segments())
    }

    /// What the intro databases need to find an item: for a movie its own
    /// TVDB/TMDB/IMDb ids, for an episode the show's, with its season and
    /// episode number.
    pub fn intro_lookup(&self, user_id: &str, item_id: &str) -> Result<introdb::Lookup> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Page {
            items: Vec<Entry>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Entry {
            #[serde(rename = "Type")]
            kind: String,
            series_id: Option<String>,
            parent_index_number: Option<u16>,
            index_number: Option<u16>,
            run_time_ticks: Option<i64>,
            #[serde(default)]
            provider_ids: HashMap<String, String>,
        }
        let fetch = |id: &str| -> Result<Entry> {
            let page: Page = self.get_json(
                "/Items",
                &[
                    ("ids", id.to_string()),
                    ("userId", user_id.to_string()),
                    ("Fields", "ProviderIds".into()),
                ],
            )?;
            page.items
                .into_iter()
                .next()
                .with_context(|| format!("no item {id}"))
        };
        let item = fetch(item_id)?;
        let duration = item.run_time_ticks.map(ticks_to_seconds);
        let (ids, episode) = match (item.kind.as_str(), &item.series_id) {
            ("Episode", Some(series)) => {
                let (Some(season), Some(number)) = (item.parent_index_number, item.index_number)
                else {
                    bail!("episode {item_id} has no season/episode number");
                };
                (fetch(series)?.provider_ids, Some((season, number)))
            }
            _ => (item.provider_ids, None),
        };
        // Jellyfin's keys vary in case between versions ("Tmdb", "Imdb").
        let id = |name: &str| {
            ids.iter()
                .find(|(k, v)| k.eq_ignore_ascii_case(name) && !v.is_empty())
                .map(|(_, v)| v.clone())
        };
        Ok(introdb::Lookup {
            episode,
            duration,
            tvdb: id("Tvdb"),
            tmdb: id("Tmdb"),
            imdb: id("Imdb"),
        })
    }

    /// The original file, streamed without transcoding (keeps HDR intact).
    pub fn stream_url(&self, item_id: &str) -> String {
        self.url(&format!("/Videos/{item_id}/stream?static=true"))
    }

    /// Header that authorises mpv's stream requests, kept out of the URL.
    /// Current servers only accept the `Authorization` form for transcoded
    /// (HLS) streams; `X-Emby-Token` works just for direct ones. It carries
    /// only the token: mpv's header list is comma-separated.
    pub fn stream_header(&self) -> Option<String> {
        self.token
            .as_ref()
            .map(|t| format!(r#"Authorization: MediaBrowser Token="{t}""#))
    }

    pub fn report(
        &self,
        kind: PlaybackReport,
        item_id: &str,
        position: f64,
        paused: bool,
        session: &PlaySession,
    ) -> Result<()> {
        let path = match kind {
            PlaybackReport::Started => "/Sessions/Playing",
            PlaybackReport::Progress => "/Sessions/Playing/Progress",
            PlaybackReport::Stopped => "/Sessions/Playing/Stopped",
        };
        let response = self
            .agent
            .post(&self.url(path))
            .header("Authorization", &self.authorization())
            .send_json(PlaybackReportBody {
                item_id,
                position_ticks: seconds_to_ticks(position),
                is_paused: paused,
                can_seek: true,
                play_method: if session.transcoding {
                    "Transcode"
                } else {
                    "DirectPlay"
                },
                play_session_id: &session.id,
            })?;
        Self::check(&response, path)
    }
}

/// `https://host/jf/System/Info/Public` -> `https://host/jf`.
fn base_from_final(uri: &str, path: &str) -> Option<String> {
    let without_query = uri.split('?').next()?;
    without_query.strip_suffix(path).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_follows_redirect() {
        assert_eq!(
            base_from_final(
                "https://demo.example.org/stable/System/Info/Public",
                "/System/Info/Public"
            ),
            Some("https://demo.example.org/stable".to_string())
        );
        assert_eq!(
            base_from_final("https://x/other", "/System/Info/Public"),
            None
        );
    }

    /// Talks to Jellyfin's public demo server: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn demo_server_round_trip() -> Result<()> {
        let mut client = Client::new("https://demo.jellyfin.org/stable", "videoplayer-test");
        let info = client.public_info()?;
        assert!(!info.server_name.is_empty());
        let auth = client.login("demo", "")?;
        let items = client.library(&auth.user.id)?;
        assert!(items.iter().any(|i| i.kind == "Movie"));
        let dir = tempfile::tempdir()?;
        let image = items
            .iter()
            .find_map(|i| i.card_image())
            .expect("some artwork");
        let out = image.cache_path(dir.path());
        client.download_image(&image, &out)?;
        assert!(fs::metadata(&out)?.len() > 1000);
        client.logout()?;
        Ok(())
    }
}
