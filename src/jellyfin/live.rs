//! Changes pushed by the server. Jellyfin tells connected clients over a
//! WebSocket when items are added, changed or removed (a new season
//! arriving) and when the user's watched state changes elsewhere. The
//! connection runs on its own thread and reconnects by itself.
//!
//! The token goes in the Authorization header, as for every other request,
//! never in the address.

use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde::Deserialize;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::{Client, ItemUserData};

/// The server drops a client it hasn't heard from in 60 seconds.
const KEEP_ALIVE: Duration = Duration::from_secs(30);
/// How long a read waits, so a stop request is noticed soon.
const READ_POLL: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Items were added, changed or removed.
    Library,
    /// Watched state or resume points changed (here, on the TV, on the
    /// web): the new state of each item comes with it.
    UserData(Vec<ItemUserData>),
    /// Connected again after losing the connection; anything may have
    /// changed in between.
    Reconnected,
}

/// Listens while it's alive; dropping it closes the connection.
pub struct Watcher {
    stop: Arc<AtomicBool>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Starts listening to `client`'s server. The receiver ends when the
/// watcher is dropped.
pub fn watch(client: &Client) -> Result<(Watcher, UnboundedReceiver<Change>)> {
    let url = socket_url(client.base())?;
    let auth = client.authorization();
    let stop = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = unbounded();
    let running = stop.clone();
    thread::Builder::new()
        .name("jellyfin-live".into())
        .spawn(move || run(&url, &auth, &running, &sender))?;
    Ok((Watcher { stop }, receiver))
}

fn socket_url(base: &str) -> Result<String> {
    let base = base.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        Ok(format!("wss://{rest}/socket"))
    } else if let Some(rest) = base.strip_prefix("http://") {
        Ok(format!("ws://{rest}/socket"))
    } else {
        bail!("unexpected server address {base}")
    }
}

fn run(url: &str, auth: &str, stop: &AtomicBool, sender: &UnboundedSender<Change>) {
    let stopped = || stop.load(Ordering::Relaxed) || sender.is_closed();
    let mut retry = RETRY_FIRST;
    let mut connected_before = false;
    let mut last_error = String::new();
    while !stopped() {
        let mut connected = false;
        let result = listen(url, auth, &stopped, connected_before, |change| {
            connected = true;
            sender.unbounded_send(change).is_ok()
        });
        match result {
            Ok(()) => return,
            Err(e) => {
                let message = format!("{e:#}");
                if message != last_error {
                    eprintln!("[jellyfin] live updates: {message}");
                    last_error = message;
                }
            }
        }
        if connected {
            connected_before = true;
            retry = RETRY_FIRST;
        }
        let until = Instant::now() + retry;
        while Instant::now() < until && !stopped() {
            thread::sleep(Duration::from_millis(250));
        }
        retry = (retry * 2).min(RETRY_MAX);
    }
}

/// One connection, until it fails (an error) or nobody listens anymore.
/// `notify` gets the changes and says whether to go on. Connections after
/// the first start with `Reconnected`.
fn listen(
    url: &str,
    auth: &str,
    stopped: &dyn Fn() -> bool,
    reconnect: bool,
    mut notify: impl FnMut(Change) -> bool,
) -> Result<()> {
    let mut socket = connect(url, auth)?;
    if reconnect && !notify(Change::Reconnected) {
        return Ok(());
    }
    keep_alive(&mut socket)?;
    let mut last_sent = Instant::now();
    loop {
        if stopped() {
            let _ = socket.close(None);
            return Ok(());
        }
        if last_sent.elapsed() >= KEEP_ALIVE {
            keep_alive(&mut socket)?;
            last_sent = Instant::now();
        }
        match socket.read() {
            Ok(Message::Text(text)) => match parse(text.as_str()) {
                Some(Pushed::Change(change)) => {
                    if !notify(change) {
                        return Ok(());
                    }
                }
                Some(Pushed::KeepAlive) => {
                    keep_alive(&mut socket)?;
                    last_sent = Instant::now();
                }
                _ => {}
            },
            Ok(Message::Close(_)) => bail!("the server closed the connection"),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.into()),
        }
    }
}

fn connect(url: &str, auth: &str) -> Result<WebSocket<MaybeTlsStream<TcpStream>>> {
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", HeaderValue::from_str(auth)?);
    let uri = request.uri();
    let host = uri
        .host()
        .context("no host in the server address")?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let secure = uri.scheme_str() == Some("wss");
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let mut stream = None;
    let mut last_error = None;
    for address in (host.as_str(), port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_error = Some(e),
        }
    }
    let stream = match (stream, last_error) {
        (Some(stream), _) => stream,
        (None, Some(e)) => return Err(e).context(format!("connecting to {host}")),
        (None, None) => bail!("{host} has no address"),
    };
    stream.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    // The same socket: shortens reads once the handshake is done.
    let socket = stream.try_clone()?;
    let (websocket, _) = tungstenite::client_tls_with_config(request, stream, None, None)
        .map_err(|e| anyhow!("{e}"))?;
    socket.set_read_timeout(Some(READ_POLL))?;
    Ok(websocket)
}

fn keep_alive(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Result<()> {
    socket.send(Message::text(r#"{"MessageType":"KeepAlive"}"#))?;
    Ok(())
}

enum Pushed {
    Change(Change),
    KeepAlive,
}

fn parse(text: &str) -> Option<Pushed> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Envelope {
        message_type: String,
        #[serde(default)]
        data: serde_json::Value,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct UserDataChanged {
        #[serde(default)]
        user_data_list: Vec<ItemUserData>,
    }
    let envelope: Envelope = serde_json::from_str(text).ok()?;
    match envelope.message_type.as_str() {
        "LibraryChanged" => Some(Pushed::Change(Change::Library)),
        "UserDataChanged" => {
            let changed: UserDataChanged = serde_json::from_value(envelope.data).ok()?;
            let items = changed
                .user_data_list
                .into_iter()
                .map(ItemUserData::normalized)
                .collect();
            Some(Pushed::Change(Change::UserData(items)))
        }
        "ForceKeepAlive" => Some(Pushed::KeepAlive),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_url_follows_the_server_address() {
        assert_eq!(
            socket_url("http://moto-moto-srv.local:8096").unwrap(),
            "ws://moto-moto-srv.local:8096/socket"
        );
        assert_eq!(
            socket_url("https://example.com/jellyfin/").unwrap(),
            "wss://example.com/jellyfin/socket"
        );
        assert!(socket_url("ftp://x").is_err());
    }

    fn change(text: &str) -> Option<Change> {
        match parse(text)? {
            Pushed::Change(change) => Some(change),
            Pushed::KeepAlive => None,
        }
    }

    #[test]
    fn reads_library_changes() {
        let text =
            r#"{"MessageType":"LibraryChanged","Data":{"ItemsAdded":["a"]},"MessageId":"x"}"#;
        assert_eq!(change(text), Some(Change::Library));
        assert!(parse("not json").is_none());
        assert!(parse(r#"{"MessageType":"Sessions","Data":[]}"#).is_none());
    }

    #[test]
    fn reads_the_play_states_pushed() {
        // As Jellyfin sends it when an episode is finished on another device.
        let text = r#"{"MessageType":"UserDataChanged","Data":{"UserId":"d29f2020b4894243ab6e08a0e469ccc7",
            "UserDataList":[{"PlayedPercentage":0,"PlaybackPositionTicks":0,"PlayCount":2,"IsFavorite":false,
            "LastPlayedDate":"2026-11-18T18:09:57.9751521Z","Played":true,"Key":"401","ItemId":"E836DACE-F2E5-5373-FDA9-3250DC1C5AAF"},
            {"UnplayedItemCount":3,"PlaybackPositionTicks":0,"PlayCount":0,"IsFavorite":false,"Played":false,"Key":"x",
            "ItemId":"ac3e3624aaaaaaaaaaaaaaaaaaaaaaaa"}]},"MessageId":"m"}"#;
        let Some(Change::UserData(items)) = change(text) else {
            panic!("not read");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].item_id, "e836dacef2e55373fda93250dc1c5aaf");
        assert!(items[0].data.played);
        assert_eq!(
            items[0].data.last_played_date.as_deref(),
            Some("2026-11-18T18:09:57.9751521Z")
        );
        assert_eq!(items[1].data.last_played_date, None);
    }
}
