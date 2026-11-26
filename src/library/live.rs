//! Keeps the Jellyfin libraries current while the app is open: each server
//! says when something changed. Watched state and resume points come with
//! the message and are applied at once (see `sync`); for new or removed
//! items (a season imported), the library is fetched again shortly after.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use gpui::Context;

use super::Library;
use crate::jellyfin::Client;
use crate::jellyfin::live::{self, Change, Watcher};

/// Imports and scans come in bursts: wait for the rest of one before
/// fetching the library.
const SETTLE: Duration = Duration::from_secs(3);
/// Fetch at most this often because of server changes.
const MIN_INTERVAL: Duration = Duration::from_secs(15);

/// A server being listened to.
struct Listener {
    /// The token it listens with: a new sign-in needs a new connection.
    token: String,
    /// Listens for as long as it's kept.
    _watcher: Watcher,
}

/// Listening to the servers, and the fetches their changes ask for.
#[derive(Default)]
pub(super) struct Live {
    /// Each signed-in server's change notifications.
    listeners: HashMap<i64, Listener>,
    /// A fetch because of server changes is waiting to run.
    refresh_pending: bool,
    /// Items came or went while a video played: fetch when it stops.
    pub(super) stale: bool,
    /// A video is playing, which a fetch would only disturb.
    pub(super) playing: bool,
}

impl Library {
    /// Listens to every signed-in server, and stops listening to servers
    /// that were signed out of.
    pub(super) fn sync_live(&mut self, cx: &mut Context<Self>) {
        let servers = self.catalog.servers();
        self.live.listeners.retain(|id, listener| {
            servers
                .iter()
                .any(|s| s.id == *id && !s.token.is_empty() && s.token == listener.token)
        });
        for server in servers {
            if server.token.is_empty() || self.live.listeners.contains_key(&server.id) {
                continue;
            }
            let client = Client::for_server(server, &self.profile.device_id);
            let (watcher, mut changes) = match live::watch(&client) {
                Ok(started) => started,
                Err(e) => {
                    eprintln!("[jellyfin] {}: live updates: {e:#}", server.name);
                    continue;
                }
            };
            let listener = Listener {
                token: server.token.clone(),
                _watcher: watcher,
            };
            self.live.listeners.insert(server.id, listener);
            let server = server.id;
            cx.spawn(async move |this, cx| {
                while let Some(change) = changes.next().await {
                    if this
                        .update(cx, |this, cx| this.server_changed(server, change, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    fn server_changed(&mut self, server: i64, change: Change, cx: &mut Context<Self>) {
        match change {
            Change::UserData(items) => self.apply_user_data(server, items, cx),
            Change::Library => self.library_changed(cx),
            Change::Reconnected => {
                // Whatever was pushed while disconnected is lost, and what
                // couldn't be sent can go now.
                self.deliver_now(cx);
                self.catch_up(cx);
                self.library_changed(cx);
            }
        }
    }

    /// Items were added or removed: the library is fetched again, once
    /// things settle and not while a video plays (then when it stops).
    fn library_changed(&mut self, cx: &mut Context<Self>) {
        if self.live.playing {
            self.live.stale = true;
            return;
        }
        if self.live.refresh_pending {
            return;
        }
        eprintln!("[jellyfin] library changed; fetching it");
        self.live.refresh_pending = true;
        let wait = self
            .fetching
            .refreshed
            .map_or(Duration::ZERO, |at| {
                MIN_INTERVAL.saturating_sub(at.elapsed())
            })
            .max(SETTLE);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                this.live.refresh_pending = false;
                if this.live.playing {
                    this.live.stale = true;
                } else {
                    this.refresh_remote(cx);
                }
            })
            .ok();
        })
        .detach();
    }
}
