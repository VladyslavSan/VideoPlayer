//! Finding servers by name when the name lookup is unreliable.
//!
//! Home servers often go by a `.local` name (`moto-moto-srv.local`), which
//! Windows resolves by multicast, and that fails now and then for minutes
//! at a time ("No such host is known") while the server itself is fine.
//! So every address a name resolves to is remembered (on disk too, for the
//! next start), and when a lookup fails the last one that worked is used.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// After a failed lookup, the remembered address is used straight away for
/// this long (a failing lookup takes seconds).
const SKIP_LOOKUP_AFTER_FAILURE: Duration = Duration::from_secs(30);

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    hosts: HashMap<String, Vec<IpAddr>>,
}

#[derive(Default)]
struct Known {
    hosts: HashMap<String, Vec<IpAddr>>,
    failed: HashMap<String, Instant>,
    file: Option<PathBuf>,
}

fn known() -> &'static Mutex<Known> {
    static KNOWN: OnceLock<Mutex<Known>> = OnceLock::new();
    KNOWN.get_or_init(Mutex::default)
}

/// Loads the addresses remembered last time, and keeps them in `file`.
pub fn init(file: PathBuf) {
    let saved: Saved = std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut known = known().lock().unwrap();
    known.hosts.extend(saved.hosts);
    known.file = Some(file);
}

fn remember(host: &str, addresses: &[SocketAddr]) {
    let ips: Vec<IpAddr> = addresses.iter().map(|a| a.ip()).collect();
    let mut known = known().lock().unwrap();
    known.failed.remove(host);
    if known.hosts.get(host) == Some(&ips) {
        return;
    }
    known.hosts.insert(host.to_string(), ips);
    if let Some(file) = &known.file {
        let saved = Saved {
            hosts: known.hosts.clone(),
        };
        if let Ok(json) = serde_json::to_vec(&saved) {
            let _ = std::fs::write(file, json);
        }
    }
}

/// The remembered addresses of `host`, at `port`.
fn remembered(host: &str, port: u16) -> Option<Vec<SocketAddr>> {
    let known = known().lock().unwrap();
    let ips = known.hosts.get(host)?;
    (!ips.is_empty()).then(|| ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
}

fn failed_lately(host: &str) -> bool {
    let known = known().lock().unwrap();
    known
        .failed
        .get(host)
        .is_some_and(|at| at.elapsed() < SKIP_LOOKUP_AFTER_FAILURE)
}

/// Resolves `host`, falling back to the last addresses that worked.
pub fn resolve(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    // An address already: nothing to look up.
    if let Ok(ip) = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    if failed_lately(host)
        && let Some(addresses) = remembered(host, port)
    {
        return Ok(addresses);
    }
    match (host, port).to_socket_addrs() {
        Ok(addresses) => {
            let addresses: Vec<SocketAddr> = addresses.collect();
            if !addresses.is_empty() {
                remember(host, &addresses);
                return Ok(addresses);
            }
            fallback(host, port, std::io::Error::other("no addresses"))
        }
        Err(e) => fallback(host, port, e),
    }
}

fn fallback(host: &str, port: u16, error: std::io::Error) -> std::io::Result<Vec<SocketAddr>> {
    let first_failure = {
        let mut known = known().lock().unwrap();
        let fresh = !known
            .failed
            .get(host)
            .is_some_and(|at| at.elapsed() < SKIP_LOOKUP_AFTER_FAILURE * 2);
        known.failed.insert(host.to_string(), Instant::now());
        fresh
    };
    match remembered(host, port) {
        Some(addresses) => {
            if first_failure {
                eprintln!(
                    "[net] can't look up {host} ({error}); using its last address {}",
                    addresses[0].ip()
                );
            }
            Ok(addresses)
        }
        None => Err(error),
    }
}

/// For a player that looks names up by itself: an `http://` address on a
/// `.local` name with the name swapped for its last known address, so the
/// stream doesn't depend on the lookup. Other addresses stay as they are
/// (`https` needs the name for its certificate).
pub fn direct_url(url: &str) -> String {
    let Some(rest) = url.strip_prefix("http://") else {
        return url.to_string();
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(']') || host.ends_with(']') => {
            (host, port.parse::<u16>().ok())
        }
        _ => (authority, None),
    };
    if !host.to_ascii_lowercase().ends_with(".local") {
        return url.to_string();
    }
    let port = port.unwrap_or(80);
    let Some(address) = remembered(host, port).and_then(|a| {
        a.iter()
            .find(|a| a.is_ipv4())
            .or_else(|| a.first())
            .copied()
    }) else {
        return url.to_string();
    };
    format!("http://{address}{tail}")
}

/// An HTTP agent whose lookups fall back to remembered addresses.
pub fn agent(config: ureq::config::Config) -> ureq::Agent {
    ureq::Agent::with_parts(
        config,
        ureq::unversioned::transport::DefaultConnector::default(),
        RememberingResolver,
    )
}

/// ureq's lookups, with the fallback.
#[derive(Debug, Default)]
pub struct RememberingResolver;

impl ureq::unversioned::resolver::Resolver for RememberingResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        _config: &ureq::config::Config,
        _timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let host = uri.host().ok_or(ureq::Error::HostNotFound)?;
        let port = uri.port_u16().unwrap_or(match uri.scheme_str() {
            Some("https") => 443,
            _ => 80,
        });
        let addresses = resolve(host, port).map_err(|_| ureq::Error::HostNotFound)?;
        let mut out = self.empty();
        for address in addresses {
            if out.try_push(address).is_err() {
                break;
            }
        }
        if out.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_the_last_address() {
        let host = "test-nas-for-fallback.invalid";
        assert!(resolve(host, 8096).is_err());
        remember(host, &["192.168.1.221:1".parse().unwrap()]);
        let found = resolve(host, 8096).unwrap();
        assert_eq!(found, vec!["192.168.1.221:8096".parse().unwrap()]);
    }

    #[test]
    fn streams_use_the_known_address_of_local_names() {
        remember("box-for-test.local", &["192.168.1.221:1".parse().unwrap()]);
        assert_eq!(
            direct_url("http://box-for-test.local:8096/Videos/x/stream?static=true"),
            "http://192.168.1.221:8096/Videos/x/stream?static=true"
        );
        // Names that aren't .local, https, and unknown names stay.
        assert_eq!(
            direct_url("http://example.com:8096/a"),
            "http://example.com:8096/a"
        );
        assert_eq!(
            direct_url("https://box-for-test.local/a"),
            "https://box-for-test.local/a"
        );
        assert_eq!(
            direct_url("http://unknown-box.local:8096/a"),
            "http://unknown-box.local:8096/a"
        );
        assert_eq!(direct_url("C:\\videos\\a.mkv"), "C:\\videos\\a.mkv");
    }
}
