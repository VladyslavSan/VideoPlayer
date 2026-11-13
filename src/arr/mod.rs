//! Sonarr (TV shows) and Radarr (movies): the apps that find and download
//! what Jellyfin then serves. Both have the same v3 web API, authorized by
//! an API key in the `X-Api-Key` header (never in the address).

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use ureq::Agent;

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

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}/api/v3{path}", self.base);
        let mut response = self
            .agent
            .get(&url)
            .header("X-Api-Key", &self.key)
            .header("Accept", "application/json")
            .call()
            .with_context(|| format!("couldn't reach {}", self.base))?;
        let name = self.kind.name();
        match response.status().as_u16() {
            200..=299 => {}
            401 => bail!("{name} didn't accept the API key"),
            404 => bail!("no {name} at {}; check the address", self.base),
            code => bail!("{name} answered HTTP {code}"),
        }
        response
            .body_mut()
            .read_json()
            .with_context(|| format!("unexpected answer from {}; is this {name}?", self.base))
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
