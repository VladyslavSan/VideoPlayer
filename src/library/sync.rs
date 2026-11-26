//! Keeping watched state and resume points in step with the servers, which
//! other devices (the TV, a laptop, the web) change too. What a server
//! pushes is applied as it comes; after playback here the item is read back;
//! coming back to the app catches up on what was played lately. A library
//! fetch takes seconds: what was learned while it ran is newer, and kept.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui::Context;

use super::{Catalog, Item, Library, Source, outbox::Change};
use crate::jellyfin::{self, Client, ItemUserData, PlaySession, PlaybackReport, UserData};

/// Pushed changes are written to the cached library at most this often.
const CACHE_WRITE_AFTER: Duration = Duration::from_secs(20);
/// Switching back to the app catches up on what was played elsewhere, if
/// it hasn't for this long...
const CATCH_UP_ON_FOCUS_AFTER: Duration = Duration::from_secs(15);
/// ...and fetches the whole library again if it's older than this.
const REFRESH_ON_FOCUS_AFTER: Duration = Duration::from_secs(30 * 60);

/// An item's watched state and resume point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PlayState {
    position: f64,
    watched: bool,
    last_played: Option<i64>,
}

impl PlayState {
    fn of(item: &Item) -> Self {
        Self {
            position: item.position,
            watched: item.watched,
            last_played: item.last_played,
        }
    }

    /// What a server says; without a date, the item keeps `last_played`.
    fn from_server(data: &UserData, last_played: Option<i64>) -> Self {
        Self {
            position: jellyfin::ticks_to_seconds(data.playback_position_ticks),
            watched: data.played,
            last_played: data
                .last_played_date
                .as_deref()
                .and_then(jellyfin::parse_timestamp)
                .or(last_played),
        }
    }

    fn apply_to(self, item: &mut Item) {
        item.position = self.position;
        item.watched = self.watched;
        item.last_played = self.last_played;
    }
}

/// When each item's play state (by key) was last learned from its server
/// outside a library fetch, which must not undo it.
#[derive(Debug, Default)]
pub(super) struct Fresh {
    learned: HashMap<String, Instant>,
}

impl Fresh {
    pub(super) fn stamp(&mut self, key: String, at: Instant) {
        self.learned.insert(key, at);
    }

    /// Stops protecting `keys` from the next fetch.
    pub(super) fn forget(&mut self, keys: &[String]) {
        self.learned.retain(|key, _| !keys.contains(key));
    }

    /// The play states learned since a fetch of `server` began at `started`,
    /// to put back over what it brings. Its older stamps are dropped.
    pub(super) fn newer_than(
        &mut self,
        remote: &[Item],
        server: i64,
        started: Instant,
    ) -> HashMap<String, PlayState> {
        let prefix = format!("jf-{server}-");
        self.learned
            .retain(|key, at| !key.starts_with(&prefix) || *at > started);
        remote
            .iter()
            .map(|item| (item.key(), PlayState::of(item)))
            .filter(|(key, _)| self.learned.contains_key(key))
            .collect()
    }
}

/// Puts back the play states `Fresh::newer_than` kept.
pub(super) fn restore(remote: &mut [Item], kept: &HashMap<String, PlayState>) {
    if kept.is_empty() {
        return;
    }
    for item in remote {
        if let Some(state) = kept.get(&item.key()) {
            state.apply_to(item);
        }
    }
}

/// Applies play states `server` reported, stamping each as fresh. Returns
/// the ones that changed something; unknown items are skipped.
pub(super) fn apply_user_data(
    catalog: &mut Arc<Catalog>,
    fresh: &mut Fresh,
    server: i64,
    updates: Vec<ItemUserData>,
    now: Instant,
) -> Vec<ItemUserData> {
    let mut changed = Vec::new();
    for update in updates {
        // Seasons, shows and things not fetched yet aren't in the catalog.
        let Some(index) = catalog.remote_index(server, &update.item_id) else {
            continue;
        };
        let item = &catalog.remote[index];
        fresh.stamp(item.key(), now);
        let state = PlayState::from_server(&update.data, item.last_played);
        if state == PlayState::of(item) {
            continue;
        }
        // Copies the catalog only once something does change.
        state.apply_to(&mut Arc::make_mut(catalog).remote[index]);
        changed.push(update);
    }
    changed
}

/// Watched state and resume points learned from the servers.
#[derive(Debug, Default)]
pub(super) struct PlayStates {
    /// What a library fetch under way must not undo.
    pub(super) fresh: Fresh,
    /// When the app last caught up on what was played lately.
    caught_up: Option<Instant>,
    /// Pushed play states not yet written to each server's cached library.
    cache_updates: HashMap<i64, Vec<ItemUserData>>,
    cache_write_pending: bool,
}

/// Where a video stopped playing here.
#[derive(Debug, Clone, Copy)]
pub struct Stop {
    pub position: f64,
    pub duration: f64,
    /// Played to the end (or far enough to count).
    pub finished: bool,
}

/// Items being marked on one server.
#[derive(Debug, Default)]
struct Marked {
    ids: Vec<String>,
    keys: Vec<String>,
}

impl Library {
    /// Applies play states the server reported.
    pub(super) fn apply_user_data(
        &mut self,
        server: i64,
        updates: Vec<ItemUserData>,
        cx: &mut Context<Self>,
    ) {
        let fresh = &mut self.play_states.fresh;
        let changed = apply_user_data(&mut self.catalog, fresh, server, updates, Instant::now());
        if changed.is_empty() {
            return;
        }
        eprintln!("[jellyfin] play state of {} item(s) updated", changed.len());
        self.play_states
            .cache_updates
            .entry(server)
            .or_default()
            .extend(changed);
        self.write_cache_later(cx);
        cx.notify();
    }

    /// Reads one item's play state back from the server, after playing it.
    pub fn confirm_play_state(&mut self, server: i64, id: String, cx: &mut Context<Self>) {
        let (Some(client), Some(user_id)) = (self.client(server), self.catalog.user_id(server))
        else {
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

    /// The app came back to the front: picks up what was done elsewhere
    /// meanwhile, with a whole fetch if the library is old.
    pub fn came_back(&mut self, cx: &mut Context<Self>) {
        let older = |at: Option<Instant>, than| at.is_none_or(|at| at.elapsed() >= than);
        let refreshed = self.fetching.refreshed;
        if older(refreshed, REFRESH_ON_FOCUS_AFTER) {
            self.refresh_remote(cx);
        } else if older(self.play_states.caught_up, CATCH_UP_ON_FOCUS_AFTER)
            && older(refreshed, CATCH_UP_ON_FOCUS_AFTER)
        {
            self.catch_up(cx);
        }
    }

    /// Catches up on what was played lately, on every server: about a
    /// second, against several for a full library fetch.
    pub(super) fn catch_up(&mut self, cx: &mut Context<Self>) {
        self.play_states.caught_up = Some(Instant::now());
        for server in self.catalog.servers() {
            if server.token.is_empty() {
                continue;
            }
            let client = Client::for_server(server, &self.profile.device_id);
            let user_id = server.user_id.clone();
            let (server, name) = (server.id, server.name.clone());
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
                            .map(|item| ItemUserData {
                                item_id: item.id,
                                data: item.user_data.unwrap_or_default(),
                            })
                            .collect();
                        this.apply_user_data(server, updates, cx);
                    }
                    Err(e) => eprintln!("[jellyfin] {name}: catching up: {e:#}"),
                })
                .ok();
            })
            .detach();
        }
    }

    fn write_cache_later(&mut self, cx: &mut Context<Self>) {
        if self.play_states.cache_write_pending {
            return;
        }
        self.play_states.cache_write_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CACHE_WRITE_AFTER).await;
            this.update(cx, |this, cx| {
                this.play_states.cache_write_pending = false;
                for (server, updates) in std::mem::take(&mut this.play_states.cache_updates) {
                    let path = this.profile.paths.library_cache(server);
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

    /// Takes in a resume point and watched state just read from the server.
    pub fn set_play_state(
        &mut self,
        key: &str,
        position: f64,
        watched: bool,
        cx: &mut Context<Self>,
    ) {
        let known = self
            .catalog
            .remote
            .iter()
            .position(|item| item.key() == key);
        let Some(index) = known else {
            return;
        };
        let item = &self.catalog.remote[index];
        if item.position == position && item.watched == watched {
            return;
        }
        let item = &mut self.catalog_mut(cx).remote[index];
        item.position = position;
        item.watched = watched;
    }

    /// A video started playing: library fetches wait until it stops.
    pub fn playback_started(&mut self) {
        self.live.playing = true;
    }

    /// A server's video stopped here: its card shows it at once, and the
    /// server and the cached library are told (the server confirms it).
    pub fn playback_stopped(
        &mut self,
        source: &Source,
        stop: Stop,
        session: Option<PlaySession>,
        cx: &mut Context<Self>,
    ) {
        let Source::Jellyfin { server, id } = source else {
            return;
        };
        let Stop {
            position,
            duration,
            finished,
        } = stop;
        if let Some(index) = self.catalog.remote_index(*server, id) {
            let item = &mut self.catalog_mut(cx).remote[index];
            item.position = if finished { 0.0 } else { position };
            item.watched |= finished;
            item.last_played = Some(super::now());
        }
        eprintln!(
            "[jellyfin] stopped at {position:.0}/{duration:.0} s{}",
            if finished { " (watched)" } else { "" }
        );
        // A finished video is reported at its full runtime, so the server
        // marks it watched even when it was left during the end credits.
        let reported = if finished { duration } else { position };
        if let Some(session) = session {
            let report = super::Report {
                kind: PlaybackReport::Stopped,
                position: reported,
                paused: false,
                session,
            };
            self.report(source, report, super::AfterReport::ReadPlayState, cx);
        }
        // The cached library is what the next start shows first.
        let (cache, id) = (self.profile.paths.library_cache(*server), id.clone());
        self.queue_task(
            "updating the library cache",
            move || jellyfin::cache_play_state(&cache, &id, position, finished, super::now()),
            cx,
        );
    }

    /// The video is gone: local progress saved meanwhile shows, and a fetch
    /// put off while it played runs.
    pub fn playback_ended(&mut self, cx: &mut Context<Self>) {
        self.live.playing = false;
        self.reload(cx);
        if std::mem::take(&mut self.live.stale) {
            self.refresh_remote(cx);
        }
    }

    /// Marks items watched or unwatched, here and on their servers. Either
    /// way the resume point goes. Items already that way are left alone.
    pub fn mark_watched(&mut self, keys: &[String], watched: bool, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut remote: HashMap<i64, Marked> = HashMap::new();
        let mut local_changed = false;
        for key in keys {
            let Some(item) = self.catalog.item(key) else {
                continue;
            };
            if item.watched == watched && item.progress().is_none() {
                continue;
            }
            match &item.source {
                Source::Local { id, .. } => {
                    if let Err(e) = self.profile.db.set_watched(*id, watched) {
                        self.error = Some(format!("Couldn't update {}: {e}", item.title).into());
                    }
                    local_changed = true;
                }
                Source::Jellyfin { server, id } => {
                    let marked = remote.entry(*server).or_default();
                    marked.ids.push(id.clone());
                    marked.keys.push(key.clone());
                    self.catalog_mut(cx)
                        .mark_watched(key, watched, super::now());
                    // A library fetch already under way mustn't undo it.
                    self.play_states.fresh.stamp(key.clone(), now);
                }
            }
        }
        if local_changed {
            self.reload(cx);
        }
        for (server, marked) in remote {
            self.send_watched(server, marked, watched, cx);
        }
        cx.notify();
    }

    /// Tells `server` what was marked; written down until it has it.
    fn send_watched(&mut self, server: i64, marked: Marked, watched: bool, cx: &mut Context<Self>) {
        let (Some(client), Some(user_id)) = (self.client(server), self.catalog.user_id(server))
        else {
            return;
        };
        let Marked { ids, keys } = marked;
        let count = ids.len();
        eprintln!(
            "[jellyfin] marking {count} item(s) {}",
            if watched { "watched" } else { "unwatched" }
        );
        let sequences: Vec<u64> = ids
            .iter()
            .map(|id| self.remember(server, id, Change::Watched(watched)))
            .collect();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.set_played_many(&user_id, &ids, watched) })
                .await;
            this.update(cx, |this, cx| {
                this.watched_sent(sequences, &keys, result, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Takes in whether the server took what was marked.
    fn watched_sent(
        &mut self,
        sequences: Vec<u64>,
        keys: &[String],
        sent: Result<()>,
        cx: &mut Context<Self>,
    ) {
        match sent {
            Ok(()) => {
                for sequence in sequences {
                    self.forget(sequence);
                }
            }
            Err(e) if jellyfin::is_offline(&e) => {
                eprintln!(
                    "[jellyfin] marking watched: {e:#}; will send it when the server answers"
                );
                for sequence in sequences {
                    self.sending_failed(sequence, &e, cx);
                }
            }
            Err(e) => {
                eprintln!("[jellyfin] marking watched: {e:#}");
                let count = sequences.len();
                for sequence in sequences {
                    self.forget(sequence);
                }
                self.error =
                    Some(format!("Couldn't tell the server about {count} item(s): {e:#}").into());
                // Show what the server has instead.
                self.play_states.fresh.forget(keys);
                self.refresh_remote(cx);
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::catalog::tests::movie;

    fn catalog(remote: Vec<Item>) -> Arc<Catalog> {
        Arc::new(Catalog {
            remote,
            ..Catalog::default()
        })
    }

    fn pushed(id: &str, seconds: i64, played: bool, date: Option<&str>) -> ItemUserData {
        ItemUserData {
            item_id: id.into(),
            data: UserData {
                playback_position_ticks: seconds * 10_000_000,
                played,
                last_played_date: date.map(Into::into),
            },
        }
    }

    #[test]
    fn a_pushed_play_state_shows_on_its_item() {
        let mut catalog = catalog(vec![movie(1, "a"), movie(1, "b")]);
        let mut fresh = Fresh::default();
        let update = pushed("b", 120, true, Some("2026-11-07T18:11:04.4168678Z"));
        let changed = apply_user_data(
            &mut catalog,
            &mut fresh,
            1,
            vec![update.clone()],
            Instant::now(),
        );
        assert_eq!(changed, [update]);
        let item = &catalog.remote[1];
        assert_eq!(item.position, 120.0);
        assert!(item.watched);
        assert_eq!(item.last_played, Some(1_794_075_064));
        assert_eq!(catalog.remote[0], movie(1, "a"));
    }

    #[test]
    fn a_play_state_without_a_date_keeps_when_the_item_was_last_played() {
        let mut played = movie(1, "a");
        played.last_played = Some(500);
        let mut catalog = catalog(vec![played]);
        let update = pushed("a", 60, false, None);
        apply_user_data(
            &mut catalog,
            &mut Fresh::default(),
            1,
            vec![update],
            Instant::now(),
        );
        assert_eq!(catalog.remote[0].position, 60.0);
        assert_eq!(catalog.remote[0].last_played, Some(500));
    }

    #[test]
    fn a_play_state_the_item_already_has_changes_nothing_but_still_counts_as_fresh() {
        let mut catalog = catalog(vec![movie(1, "a")]);
        let snapshot = catalog.clone();
        let mut fresh = Fresh::default();
        let before = Instant::now();
        let changed = apply_user_data(
            &mut catalog,
            &mut fresh,
            1,
            vec![pushed("a", 0, false, None)],
            Instant::now(),
        );
        assert!(changed.is_empty());
        // Nothing changed, so views' snapshots are still the catalog itself.
        assert!(Arc::ptr_eq(&catalog, &snapshot));
        assert_eq!(fresh.newer_than(&catalog.remote, 1, before).len(), 1);
    }

    #[test]
    fn play_states_of_unknown_items_and_other_servers_are_skipped() {
        let mut catalog = catalog(vec![movie(1, "a")]);
        let mut fresh = Fresh::default();
        let updates = vec![
            pushed("season-3", 0, true, None),
            pushed("a", 90, true, None),
        ];
        // Server 2 has an item "a" of its own, which isn't held.
        let changed = apply_user_data(&mut catalog, &mut fresh, 2, updates, Instant::now());
        assert!(changed.is_empty());
        assert_eq!(catalog.remote[0], movie(1, "a"));
        assert!(fresh.learned.is_empty());
    }

    #[test]
    fn a_fetch_does_not_undo_what_was_learned_while_it_ran() {
        let fetch_started = Instant::now();
        let mut catalog = catalog(vec![movie(1, "a"), movie(1, "b")]);
        let mut fresh = Fresh::default();
        let learned = fetch_started + Duration::from_secs(1);
        apply_user_data(
            &mut catalog,
            &mut fresh,
            1,
            vec![pushed("a", 300, false, None)],
            learned,
        );

        let kept = fresh.newer_than(&catalog.remote, 1, fetch_started);
        // The fetch brings both items as the server had them when it began.
        let mut fetched = vec![movie(1, "a"), movie(1, "b")];
        restore(&mut fetched, &kept);
        assert_eq!(fetched[0].position, 300.0);
        assert_eq!(fetched[1], movie(1, "b"));
    }

    #[test]
    fn a_fetch_replaces_what_was_learned_before_it_began() {
        let mut catalog = catalog(vec![movie(1, "a"), movie(2, "a")]);
        let mut fresh = Fresh::default();
        let learned = Instant::now();
        for server in [1, 2] {
            let update = pushed("a", 300, false, None);
            apply_user_data(&mut catalog, &mut fresh, server, vec![update], learned);
        }
        let fetch_started = learned + Duration::from_secs(1);

        let kept = fresh.newer_than(&catalog.remote, 1, fetch_started);
        // Server 1's stamp is older than its fetch and goes; server 2's
        // stays until its own fetch, so its item is still protected.
        assert_eq!(kept.keys().collect::<Vec<_>>(), ["jf-2-a"]);
        assert!(!fresh.learned.contains_key("jf-1-a"));
        let mut fetched = vec![movie(1, "a")];
        restore(&mut fetched, &kept);
        assert_eq!(fetched[0].position, 0.0);
    }

    #[test]
    fn forgetting_fresh_items_lets_the_next_fetch_replace_them() {
        let mut fresh = Fresh::default();
        let fetch_started = Instant::now();
        let at = fetch_started + Duration::from_secs(1);
        fresh.stamp("jf-1-a".into(), at);
        fresh.stamp("jf-1-b".into(), at);
        fresh.forget(&["jf-1-a".to_string()]);
        let remote = [movie(1, "a"), movie(1, "b")];
        let kept = fresh.newer_than(&remote, 1, fetch_started);
        assert_eq!(kept.keys().collect::<Vec<_>>(), ["jf-1-b"]);
    }
}
