//! Sonarr (TV shows) and Radarr (movies): the apps that find and download
//! what Jellyfin then serves. Both have the same v3 web API, authorized by
//! an API key in the `X-Api-Key` header (never in the address).

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use ureq::Agent;

mod activity;
mod seasons;
pub use activity::{Download, Event, Recent, State as DownloadState, Subject};
pub use seasons::{SeasonSummary, format_date, next_airing, parse_utc, summarize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Sonarr,
    Radarr,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Sonarr, Kind::Radarr];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Sonarr => "Sonarr",
            Kind::Radarr => "Radarr",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Kind::Sonarr => 8989,
            Kind::Radarr => 7878,
        }
    }

    /// The library setting holding its address.
    pub fn url_setting(self) -> &'static str {
        match self {
            Kind::Sonarr => "sonarr_url",
            Kind::Radarr => "radarr_url",
        }
    }

    /// The credential store entry holding its API key.
    pub fn key_account(self) -> &'static str {
        match self {
            Kind::Sonarr => "sonarr",
            Kind::Radarr => "radarr",
        }
    }
}

/// What `/system/status` says about the app.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub app_name: String,
    pub version: String,
}

/// A show in Sonarr.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub id: i64,
    #[serde(default)]
    pub seasons: Vec<SeriesSeason>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesSeason {
    pub season_number: u32,
    pub monitored: bool,
}

/// One episode of a Sonarr show, downloaded or not.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub id: i64,
    pub season_number: u32,
    pub episode_number: u32,
    pub title: Option<String>,
    /// `2020-12-28T02:00:00Z`; none for episodes without a date yet.
    pub air_date_utc: Option<String>,
    pub has_file: bool,
}

/// A release an indexer offers, with Sonarr's verdict on it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub guid: String,
    pub indexer_id: i64,
    #[serde(default)]
    pub indexer: String,
    pub title: String,
    #[serde(default)]
    pub size: u64,
    pub age_hours: Option<f64>,
    pub seeders: Option<i64>,
    pub leechers: Option<i64>,
    /// "torrent" or "usenet".
    #[serde(default)]
    pub protocol: String,
    /// Sonarr would take it: it fits the quality profile and nothing
    /// rejects it.
    #[serde(default)]
    pub approved: bool,
    /// Why not; strings, or objects with a reason, depending on version.
    #[serde(default)]
    rejections: Vec<Value>,
    quality: Option<Value>,
    #[serde(default)]
    languages: Vec<Value>,
    pub custom_format_score: Option<i64>,
    /// Sonarr's order of preference (lower is better).
    pub release_weight: Option<i64>,
    #[serde(default)]
    pub full_season: bool,
}

impl Release {
    /// "WEBDL-1080p".
    pub fn quality(&self) -> Option<String> {
        activity::quality_name(self.quality.as_ref()?)
    }

    pub fn languages(&self) -> Vec<String> {
        self.languages
            .iter()
            .filter_map(|l| l["name"].as_str().or_else(|| l.as_str()).map(String::from))
            .collect()
    }

    pub fn rejections(&self) -> Vec<String> {
        self.rejections
            .iter()
            .filter_map(|r| {
                r.as_str()
                    .or_else(|| r["reason"].as_str())
                    .or_else(|| r["message"].as_str())
                    .map(String::from)
            })
            .collect()
    }
}

/// Sonarr's order: by its weight, approved first when there's none.
pub fn sort_releases(releases: &mut [Release]) {
    releases.sort_by_key(|r| (!r.approved, r.release_weight.unwrap_or(i64::MAX)));
}

/// A download in Sonarr's queue.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub episode_id: Option<i64>,
    #[serde(default)]
    pub size: f64,
    #[serde(default)]
    pub sizeleft: f64,
    /// downloading, queued, paused, completed, warning, failed, ...
    pub status: Option<String>,
}

#[derive(Clone)]
pub struct Client {
    kind: Kind,
    agent: Agent,
    base: String,
    key: String,
}

/// `moto-moto-srv.local:8989/` → `http://moto-moto-srv.local:8989`; keeps
/// `https://` and a path prefix (as behind a reverse proxy).
pub fn normalize_url(input: &str) -> String {
    let input = input.trim().trim_end_matches('/');
    if input.contains("://") {
        input.to_string()
    } else {
        format!("http://{input}")
    }
}

/// Where the app probably is, judging by another server's address: the
/// same host on the app's default port.
pub fn guess_url(kind: Kind, other: &str) -> Option<String> {
    let rest = other.split_once("://").map_or(other, |(_, rest)| rest);
    let host_port = rest.split('/').next()?;
    let host = match host_port.rsplit_once(':') {
        // An IPv6 address without a port is all colons; keep it whole.
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => host_port,
    };
    (!host.is_empty()).then(|| format!("http://{host}:{}", kind.default_port()))
}

impl Client {
    pub fn new(kind: Kind, base: &str, key: &str) -> Self {
        let agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            kind,
            agent,
            base: normalize_url(base),
            key: key.trim().to_string(),
        }
    }

    /// GET, or PUT/POST with a JSON body; the answer's JSON.
    fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T> {
        let url = format!("{}/api/v3{path}", self.base);
        let sent = match (method, body) {
            ("PUT", Some(body)) => self
                .agent
                .put(&url)
                .header("X-Api-Key", &self.key)
                .send_json(body),
            ("POST", Some(body)) => self
                .agent
                .post(&url)
                .header("X-Api-Key", &self.key)
                .send_json(body),
            _ => self
                .agent
                .get(&url)
                .header("X-Api-Key", &self.key)
                .header("Accept", "application/json")
                .call(),
        };
        let mut response = sent.with_context(|| format!("couldn't reach {}", self.base))?;
        let name = self.kind.name();
        match response.status().as_u16() {
            200..=299 => {}
            401 => bail!("{name} didn't accept the API key"),
            404 => bail!("no {name} at {}; check the address", self.base),
            code => {
                let detail = response.body_mut().read_to_string().unwrap_or_default();
                bail!(
                    "{name} answered HTTP {code} {}",
                    detail.chars().take(200).collect::<String>()
                )
            }
        }
        response
            .body_mut()
            .read_json()
            .with_context(|| format!("unexpected answer from {}; is this {name}?", self.base))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request("GET", path, None)
    }

    /// The Sonarr show with this TVDB id, if Sonarr has it.
    pub fn series_by_tvdb(&self, tvdb: &str) -> Result<Option<Series>> {
        let found: Vec<Series> = self.get(&format!("/series?tvdbId={}", tvdb.trim()))?;
        Ok(found.into_iter().next())
    }

    pub fn episodes(&self, series: i64) -> Result<Vec<Episode>> {
        self.get(&format!("/episode?seriesId={series}"))
    }

    /// The same, with a longer wait: searching indexers takes a while.
    fn patient(&self) -> Self {
        Self {
            agent: Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(180)))
                .http_status_as_error(false)
                .build()
                .into(),
            ..self.clone()
        }
    }

    /// Searches the indexers for a season, like Sonarr's interactive
    /// search: every release found, best first, with why Sonarr would or
    /// wouldn't take each.
    pub fn season_releases(&self, series: i64, season: u32) -> Result<Vec<Release>> {
        let mut releases: Vec<Release> = self
            .patient()
            .get(&format!("/release?seriesId={series}&seasonNumber={season}"))?;
        sort_releases(&mut releases);
        Ok(releases)
    }

    /// Sends a release to the download client, like the download button in
    /// Sonarr's interactive search.
    pub fn grab(&self, release: &Release) -> Result<()> {
        let body = json!({ "guid": release.guid, "indexerId": release.indexer_id });
        let _: Value = self.patient().request("POST", "/release", Some(&body))?;
        Ok(())
    }

    /// The show's downloads in progress.
    pub fn queue(&self, series: i64) -> Result<Vec<QueueItem>> {
        self.get(&format!("/queue/details?seriesId={series}"))
    }

    /// What the Download button does: monitors the show, the season and its
    /// episodes (so the ones still to air come too), then asks Sonarr to
    /// search for the season.
    pub fn download_season(&self, series: i64, season: u32, episodes: &[i64]) -> Result<()> {
        // The whole series object goes back, with the flags changed.
        let mut show: Value = self.get(&format!("/series/{series}"))?;
        show["monitored"] = json!(true);
        if let Some(seasons) = show["seasons"].as_array_mut() {
            for entry in seasons {
                if entry["seasonNumber"] == json!(season) {
                    entry["monitored"] = json!(true);
                }
            }
        }
        let _: Value = self.request("PUT", &format!("/series/{series}"), Some(&show))?;
        if !episodes.is_empty() {
            let monitor = json!({ "episodeIds": episodes, "monitored": true });
            let _: Value = self.request("PUT", "/episode/monitor", Some(&monitor))?;
        }
        let search = json!({ "name": "SeasonSearch", "seriesId": series, "seasonNumber": season });
        let _: Value = self.request("POST", "/command", Some(&search))?;
        Ok(())
    }

    /// Everything in the queue, one entry per download.
    pub fn downloads(&self) -> Result<Vec<Download>> {
        let include = match self.kind {
            Kind::Sonarr => "includeSeries=true&includeEpisode=true&includeUnknownSeriesItems=true",
            Kind::Radarr => "includeMovie=true&includeUnknownMovieItems=true",
        };
        let page: activity::Page<activity::QueueRecord> =
            self.get(&format!("/queue?page=1&pageSize=200&{include}"))?;
        Ok(activity::downloads(page.records))
    }

    /// What last happened to the latest downloads (`limit` of them), apart
    /// from the ones still `queued`.
    pub fn recent(&self, queued: &HashSet<String>, limit: usize) -> Result<Vec<Recent>> {
        let include = match self.kind {
            Kind::Sonarr => "includeSeries=true&includeEpisode=true",
            Kind::Radarr => "includeMovie=true",
        };
        let page: activity::Page<activity::HistoryRecord> = self.get(&format!(
            "/history?page=1&pageSize=100&sortKey=date&sortDirection=descending&{include}"
        ))?;
        Ok(activity::recent(page.records, queued, limit))
    }

    /// Checks the address and key, and that it's the right app.
    pub fn status(&self) -> Result<Status> {
        let status: Status = self.get("/system/status")?;
        if !status.app_name.eq_ignore_ascii_case(self.kind.name()) {
            bail!(
                "that's {}, not {}; check the address",
                status.app_name,
                self.kind.name()
            );
        }
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_releases() {
        let mut releases: Vec<Release> = serde_json::from_value(json!([
            {"guid": "b", "indexerId": 1, "title": "Show.S08.720p", "approved": false,
             "rejections": ["Not wanted in profile"], "releaseWeight": 2,
             "quality": {"quality": {"name": "HDTV-720p"}}},
            {"guid": "a", "indexerId": 1, "title": "Show.S08.1080p", "approved": true,
             "rejections": [], "releaseWeight": 1, "size": 1000,
             "quality": {"quality": {"name": "WEBDL-1080p"}}, "languages": [{"name": "English"}]},
            {"guid": "c", "indexerId": 2, "title": "Show.S08.2160p", "approved": false,
             "rejections": [{"reason": "Size too big", "type": "permanent"}]}
        ]))
        .unwrap();
        sort_releases(&mut releases);
        let order: Vec<&str> = releases.iter().map(|r| r.guid.as_str()).collect();
        assert_eq!(order, ["a", "b", "c"]);
        assert_eq!(releases[0].quality().as_deref(), Some("WEBDL-1080p"));
        assert_eq!(releases[0].languages(), ["English"]);
        assert_eq!(releases[2].rejections(), ["Size too big"]);
    }

    #[test]
    fn normalizes_addresses() {
        assert_eq!(
            normalize_url(" moto-moto-srv.local:8989/ "),
            "http://moto-moto-srv.local:8989"
        );
        assert_eq!(
            normalize_url("https://arr.example.com/sonarr/"),
            "https://arr.example.com/sonarr"
        );
    }

    #[test]
    fn guesses_the_address_from_jellyfin() {
        assert_eq!(
            guess_url(Kind::Sonarr, "http://moto-moto-srv.local:8096").as_deref(),
            Some("http://moto-moto-srv.local:8989")
        );
        assert_eq!(
            guess_url(Kind::Radarr, "https://media.example.com/jellyfin").as_deref(),
            Some("http://media.example.com:7878")
        );
        assert_eq!(
            guess_url(Kind::Radarr, "http://192.168.1.10:8096/").as_deref(),
            Some("http://192.168.1.10:7878")
        );
    }
}
