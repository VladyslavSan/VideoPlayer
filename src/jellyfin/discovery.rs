//! Finds Jellyfin servers on the local network with Jellyfin's UDP
//! auto-discovery (a broadcast to port 7359).

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Found {
    pub address: String,
    pub id: String,
    pub name: String,
}

fn parse(reply: &[u8]) -> Option<Found> {
    serde_json::from_slice(reply).ok()
}

/// Broadcasts once and collects replies for `wait`. Errors (no network,
/// blocked port) just mean nothing was found.
pub fn discover(wait: Duration) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return found;
    };
    if socket.set_broadcast(true).is_err()
        || socket
            .send_to(b"Who is JellyfinServer?", "255.255.255.255:7359")
            .is_err()
    {
        return found;
    }
    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 2048];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if left.is_zero() || socket.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match socket.recv_from(&mut buf) {
            Ok((n, _)) => {
                if let Some(server) = parse(&buf[..n])
                    && !found.iter().any(|f| f.id == server.id)
                {
                    found.push(server);
                }
            }
            Err(_) => break,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reply() {
        let reply = br#"{"Address":"http://192.168.1.20:8096","Id":"abc","Name":"Home","EndpointAddress":null}"#;
        assert_eq!(
            parse(reply),
            Some(Found {
                address: "http://192.168.1.20:8096".into(),
                id: "abc".into(),
                name: "Home".into()
            })
        );
        assert_eq!(parse(b"nonsense"), None);
    }
}
