use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;
use ureq::{Agent, ResponseExt};

use super::{AuthResult, BaseItem, ImageRef, ItemsPage, PublicInfo, Server, seconds_to_ticks};

const CLIENT_NAME: &str = "VideoPlayer";
const PAGE_SIZE: usize = 500;
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
struct PlaybackInfo<'a> {
    item_id: &'a str,
    position_ticks: i64,
    is_paused: bool,
    can_seek: bool,
    play_method: &'static str,
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
                ("Fields", "ParentBackdropItemId".into()),
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
        let path = format!(
            "/Items/{}/Images/{}/{}",
            image.item_id, image.kind, image.index
        );
        let mut response = self
            .agent
            .get(&self.url(&path))
            .header("Authorization", &self.authorization())
            .query("tag", &image.tag)
            .query("maxWidth", IMAGE_WIDTH.to_string())
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

    /// The original file, streamed without transcoding (keeps HDR intact).
    pub fn stream_url(&self, item_id: &str) -> String {
        self.url(&format!("/Videos/{item_id}/stream?static=true"))
    }

    /// Header that authorises mpv's stream requests, kept out of the URL.
    pub fn stream_header(&self) -> Option<String> {
        self.token.as_ref().map(|t| format!("X-Emby-Token: {t}"))
    }

    pub fn report(
        &self,
        kind: PlaybackReport,
        item_id: &str,
        position: f64,
        paused: bool,
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
            .send_json(PlaybackInfo {
                item_id,
                position_ticks: seconds_to_ticks(position),
                is_paused: paused,
                can_seek: true,
                play_method: "DirectPlay",
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
