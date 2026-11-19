//! Keeping watched state and resume points in step with the server, which
//! other devices (the TV, a laptop, the web) change too.
//!
//! - What the server pushes is applied as it comes: each change carries the
//!   new play state, so nothing has to be fetched for it.
//! - After playback here, the item is read back (a few milliseconds), not
//!   the whole library (several seconds).
//! - Coming back to the app catches up on what was played lately.
//! - A library fetch takes a few seconds; anything learned while it ran is
//!   newer than what it brings, and is kept.
//! - A player left paused here lets go of the item after a while, and never
//!   writes its old position over what another device did meanwhile.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui::Context;

use super::Shell;
use crate::jellyfin::{
    self, Client, ItemUserData, PlaySession, PlaybackReport, UserData, parse_timestamp,
};
use crate::library::Source;

/// A paused player tells the server it's done after this long, before the
/// server would end it by itself (5 minutes without a word), which writes
/// the paused position whatever happened since.
const LET_GO_AFTER: Duration = Duration::from_secs(120);
/// Pushed changes are written to the cached library at most this often.
const CACHE_WRITE_AFTER: Duration = Duration::from_secs(20);

/// A paused playback.
pub(super) struct Pause {
    since: Instant,
    /// The item's play state on the server right after the pause was
    /// reported. Anything else later means it was played elsewhere.
    baseline: Arc<Mutex<Option<UserData>>>,
    /// The server has been told this playback is over (it resumes as a
    /// new one).
    let_go: bool,
}

/// What a library fetch must not undo.
pub(super) struct PlayState {
    position: f64,
    watched: bool,
    last_played: Option<i64>,
}

impl Shell {
    /// Applies play states the server reported. Unknown items (seasons,
    /// shows, things not fetched yet) are skipped.
    pub(super) fn apply_user_data(
        &mut self,
        server: i64,
        updates: Vec<ItemUserData>,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let mut changed = Vec::new();
        for update in updates {
            let Some(item) = self.remote.iter_mut().find(|i| {
                matches!(&i.source, Source::Jellyfin { server: s, id } if *s == server && *id == update.item_id)
            }) else {
                continue;
            };
            self.fresh.insert(item.key(), now);
            let position = jellyfin::ticks_to_seconds(update.data.playback_position_ticks);
            let last_played = update
                .data
                .last_played_date
                .as_deref()
                .and_then(parse_timestamp)
                .or(item.last_played);
            if item.position == position
                && item.watched == update.data.played
                && item.last_played == last_played
            {
                continue;
            }
            item.position = position;
            item.watched = update.data.played;
            item.last_played = last_played;
            changed.push(update);
        }
        if changed.is_empty() {
            return;
        }
        eprintln!("[jellyfin] play state of {} item(s) updated", changed.len());
        self.cache_updates
            .entry(server)
            .or_default()
            .extend(changed);
        self.write_cache_later(cx);
        cx.notify();
    }

    /// Reads one item's play state back from the server, after playing it.
    pub(super) fn confirm_play_state(&mut self, server: i64, id: String, cx: &mut Context<Self>) {
        let (Some(client), Some(user_id)) = (self.client(server), self.user_id(server)) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let read_id = id.clone();
            let data = cx
                .background_executor()
                .spawn(async move { client.user_data(&user_id, &read_id) })
                .await;
            this.update(cx, |this, cx| match data {
                Ok(data) => {
                    this.apply_user_data(server, vec![ItemUserData { item_id: id, data }], cx)
                }
                Err(e) => eprintln!("[jellyfin] reading back the play state: {e:#}"),
            })
            .ok();
        })
        .detach();
    }

    /// Catches up on what was played lately, on every server: for when the
    /// app may have missed changes (it was in the background, or the
    /// connection dropped). About a second, against several for a full
    /// library fetch.
    pub(super) fn catch_up(&mut self, cx: &mut Context<Self>) {
        self.caught_up = Some(Instant::now());
        for server in self.servers.clone() {
            if server.token.is_empty() {
                continue;
            }
            let client = Client::for_server(&server, &self.device_id);
            let user_id = server.user_id.clone();
            cx.spawn(async move |this, cx| {
                let recent = cx
                    .background_executor()
                    .spawn(async move { client.recently_played(&user_id) })
                    .await;
                this.update(cx, |this, cx| match recent {
                    Ok(items) => {
                        this.deliver_now(cx);
                        let updates = items
                            .into_iter()
                            .map(|i| ItemUserData {
                                item_id: i.id,
                                data: i.user_data.unwrap_or_default(),
                            })
                            .collect();
                        this.apply_user_data(server.id, updates, cx);
                    }
                    Err(e) => eprintln!("[jellyfin] {}: catching up: {e:#}", server.name),
                })
                .ok();
            })
            .detach();
        }
    }

    /// The play states learned from `server` since a library fetch began,
    /// to put back over what it brought. Older ones are forgotten.
    pub(super) fn fresher_than(
        &mut self,
        server: i64,
        started: Instant,
    ) -> HashMap<String, PlayState> {
        let prefix = format!("jf-{server}-");
        self.fresh
            .retain(|key, at| !key.starts_with(&prefix) || *at > started);
        self.remote
            .iter()
            .filter(|i| self.fresh.contains_key(&i.key()))
            .map(|i| {
                (
                    i.key(),
                    PlayState {
                        position: i.position,
                        watched: i.watched,
                        last_played: i.last_played,
                    },
                )
            })
            .collect()
    }

    pub(super) fn restore_fresh(&mut self, kept: HashMap<String, PlayState>) {
        if kept.is_empty() {
            return;
        }
        for item in &mut self.remote {
            if let Some(state) = kept.get(&item.key()) {
                item.position = state.position;
                item.watched = state.watched;
                item.last_played = state.last_played;
            }
        }
    }

    fn write_cache_later(&mut self, cx: &mut Context<Self>) {
        if self.cache_write_pending {
            return;
        }
        self.cache_write_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CACHE_WRITE_AFTER).await;
            this.update(cx, |this, cx| {
                this.cache_write_pending = false;
                for (server, updates) in std::mem::take(&mut this.cache_updates) {
                    let path = this.paths.library_cache(server);
                    this.queue_task(
                        "updating the library cache",
                        move || jellyfin::cache_user_data(&path, &updates),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- the player's reports ----

    /// Passes the player's position on to the server. Pausing is said
    /// once; playing again after the server was let go starts anew.
    pub(super) fn playback_progress(
        &mut self,
        position: f64,
        paused: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(active) = self.player.as_mut() else {
            return;
        };
        let (Some(source), Some(session)) = (active.source.clone(), active.session.clone()) else {
            return;
        };
        let Source::Jellyfin { server, id } = source.clone() else {
            return;
        };
        let was = active.pause.as_ref().map(|p| p.let_go);
        match (paused, was) {
            (true, Some(_)) => {}
            (true, None) => {
                let since = Instant::now();
                let baseline = Arc::new(Mutex::new(None));
                active.pause = Some(Pause {
                    since,
                    baseline: baseline.clone(),
                    let_go: false,
                });
                self.queue_report(
                    &source,
                    PlaybackReport::Progress,
                    position,
                    true,
                    Some(session),
                    false,
                    cx,
                );
                if let (Some(client), Some(user_id)) = (self.client(server), self.user_id(server)) {
                    self.queue_task(
                        "reading the play state",
                        move || {
                            *baseline.lock().unwrap() = Some(client.user_data(&user_id, &id)?);
                            Ok(())
                        },
                        cx,
                    );
                }
                self.let_go_later(since, cx);
            }
            (false, was) => {
                active.pause = None;
                if was == Some(true) {
                    eprintln!("[jellyfin] playing again after letting go");
                    self.queue_report(
                        &source,
                        PlaybackReport::Started,
                        position,
                        false,
                        Some(session.clone()),
                        false,
                        cx,
                    );
                }
                self.queue_report(
                    &source,
                    PlaybackReport::Progress,
                    position,
                    false,
                    Some(session),
                    false,
                    cx,
                );
            }
        }
    }

    fn let_go_later(&mut self, since: Instant, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LET_GO_AFTER).await;
            this.update(cx, |this, cx| {
                let current = this
                    .player
                    .as_ref()
                    .and_then(|a| a.pause.as_ref())
                    .is_some_and(|p| p.since == since && !p.let_go);
                if current {
                    this.let_go(false, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Tells the server a paused playback is over, at the paused position,
    /// unless the item was played elsewhere since (then its state stays).
    /// `closing`: the player is closing, so a transcode ends too.
    fn let_go(&mut self, closing: bool, cx: &mut Context<Self>) {
        let Some(active) = self.player.as_mut() else {
            return;
        };
        let (Some(Source::Jellyfin { server, id }), Some(session), Some(pause)) = (
            active.source.clone(),
            active.session.clone(),
            active.pause.as_mut(),
        ) else {
            return;
        };
        pause.let_go = true;
        let baseline = pause.baseline.clone();
        let position = active.view.read(cx).position();
        let (Some(client), Some(user_id)) = (self.client(server), self.user_id(server)) else {
            return;
        };
        eprintln!("[jellyfin] letting go of the paused video at {position:.0} s");
        let read_id = id.clone();
        self.queue_checked(
            "letting go of the paused video",
            move || {
                let_go(
                    &client, &user_id, &read_id, position, &session, &baseline, closing,
                )
            },
            (server, id),
            super::offline::Change::StoppedAt(position),
            cx,
        );
    }

    /// The player closed while paused, or after letting go: what the
    /// server needs to hear. False if it was playing (the usual report).
    pub(super) fn finish_paused(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(active) = self.player.as_ref() else {
            return false;
        };
        let (Some(Source::Jellyfin { server, id }), Some(session), Some(pause)) = (
            active.source.clone(),
            active.session.clone(),
            active.pause.as_ref(),
        ) else {
            return false;
        };
        if !pause.let_go {
            self.let_go(true, cx);
            return true;
        }
        if session.transcoding
            && let Some(client) = self.client(server)
        {
            self.queue_task(
                "ending the transcode",
                move || client.stop_transcoding(&session),
                cx,
            );
        }
        self.confirm_play_state(server, id, cx);
        true
    }

    /// A new stream (another quality) restarts the server's playback: a
    /// pause carries on from now.
    pub(super) fn restart_pause(&mut self, cx: &mut Context<Self>) {
        let Some(pause) = self.player.as_mut().and_then(|a| a.pause.as_mut()) else {
            return;
        };
        let since = Instant::now();
        pause.since = since;
        pause.let_go = false;
        self.let_go_later(since, cx);
    }
}

fn let_go(
    client: &Client,
    user_id: &str,
    id: &str,
    position: f64,
    session: &PlaySession,
    baseline: &Mutex<Option<UserData>>,
    closing: bool,
) -> Result<()> {
    let before = baseline.lock().unwrap().clone();
    let now = client.user_data(user_id, id).ok();
    let elsewhere =
        matches!((&before, &now), (Some(before), Some(now)) if now.moved_on_from(before));
    if elsewhere {
        eprintln!("[jellyfin] played elsewhere while paused here; its play state stays");
    }
    let reported = client.report_let_go(id, position, session, elsewhere);
    if closing
        && session.transcoding
        && let Err(e) = client.stop_transcoding(session)
    {
        eprintln!("[jellyfin] stopping transcoding: {e:#}");
    }
    reported
}
