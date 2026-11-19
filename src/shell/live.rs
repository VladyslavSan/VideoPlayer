//! Keeps the Jellyfin libraries current while the app is open: each server
//! says when something changed. Watched state and resume points come with
//! the message and are applied at once (see `sync`); for new or removed
//! items (a season imported), the library is fetched again shortly after.

use std::time::Duration;

use futures::StreamExt;
use gpui::Context;

use super::Shell;
use crate::jellyfin::Client;
use crate::jellyfin::live::{self, Change};

/// Imports and scans come in bursts: wait for the rest of one before
/// fetching the library.
const SETTLE: Duration = Duration::from_secs(3);
/// Fetch at most this often because of server changes.
const MIN_INTERVAL: Duration = Duration::from_secs(15);

impl Shell {
    /// Listens to every signed-in server, and stops listening to servers
    /// that were signed out of.
    pub(super) fn sync_live(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<_> = self
            .servers
            .iter()
            .filter(|s| !s.token.is_empty())
            .map(|s| (s.id, s.token.clone()))
            .collect();
        self.live
            .retain(|id, (token, _)| wanted.contains(&(*id, token.clone())));
        for server in self.servers.clone() {
            if server.token.is_empty() || self.live.contains_key(&server.id) {
                continue;
            }
            let client = Client::for_server(&server, &self.device_id);
            let (watcher, mut changes) = match live::watch(&client) {
                Ok(started) => started,
                Err(e) => {
                    eprintln!("[jellyfin] {}: live updates: {e:#}", server.name);
                    continue;
                }
            };
            self.live.insert(server.id, (server.token.clone(), watcher));
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
        if self.player.is_some() {
            self.library_stale = true;
            return;
        }
        if self.live_refresh_pending {
            return;
        }
        eprintln!("[jellyfin] library changed; fetching it");
        self.live_refresh_pending = true;
        let wait = self
            .remote_refreshed
            .map_or(Duration::ZERO, |at| {
                MIN_INTERVAL.saturating_sub(at.elapsed())
            })
            .max(SETTLE);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                this.live_refresh_pending = false;
                if this.player.is_none() {
                    this.refresh_remote(cx);
                } else {
                    this.library_stale = true;
                }
            })
            .ok();
        })
        .detach();
    }
}
