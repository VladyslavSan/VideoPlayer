//! What a server couldn't be told because it couldn't be reached: where a
//! video stopped, an item marked watched. Each change is written down before
//! it is sent and crossed off once the server has it, so neither a lost
//! connection nor closing the app loses it. They go out once the server
//! answers again, unless the item was played elsewhere since.

use std::time::Duration;

use anyhow::Result;
use gpui::Context;
use serde::{Deserialize, Serialize};

use super::Library;
use crate::jellyfin::{self, Client, PlaySession, PlaybackReport};

/// The setting the changes are saved under.
const SETTING: &str = "undelivered";
/// Tries again this often while the server can't be reached.
const RETRY: Duration = Duration::from_secs(15);
/// How long after the app starts last time's changes go out.
const FIRST_TRY: Duration = Duration::from_secs(2);

/// What to tell the server about an item.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Change {
    /// Playback stopped here (at the runtime when finished).
    StoppedAt(f64),
    Watched(bool),
}

/// A change its server hasn't got yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Undelivered {
    server: i64,
    item: String,
    change: Change,
    /// When it happened (Unix seconds): what the server has from later wins.
    at: i64,
    /// Tells apart two changes to the same item.
    #[serde(rename = "seq")]
    sequence: u64,
}

/// The changes still to send, oldest first.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Outbox {
    changes: Vec<Undelivered>,
    /// The number the last change was given.
    sequence: u64,
    /// A round of sending is waiting or under way.
    delivering: bool,
}

impl Outbox {
    /// The changes as `to_json` saved them; nothing if that can't be read.
    fn from_json(saved: &str) -> Self {
        let changes: Vec<Undelivered> = serde_json::from_str(saved).unwrap_or_default();
        Self {
            sequence: changes
                .iter()
                .map(|change| change.sequence)
                .max()
                .unwrap_or(0),
            changes,
            delivering: false,
        }
    }

    fn to_json(&self) -> String {
        serde_json::to_string(&self.changes).unwrap_or_default()
    }

    pub(super) fn len(&self) -> usize {
        self.changes.len()
    }

    fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Adds a change made at `at` (Unix seconds), in place of any older one
    /// for the same item. Returns its number, to cross it off by.
    fn remember(&mut self, server: i64, item: &str, change: Change, at: i64) -> u64 {
        self.sequence += 1;
        self.changes
            .retain(|known| !(known.server == server && known.item == item));
        self.changes.push(Undelivered {
            server,
            item: item.to_string(),
            change,
            at,
            sequence: self.sequence,
        });
        self.sequence
    }

    /// Crosses a change off. False if it was already gone.
    fn forget(&mut self, sequence: u64) -> bool {
        let before = self.changes.len();
        self.changes.retain(|change| change.sequence != sequence);
        self.changes.len() != before
    }

    /// Drops the changes for servers other than `servers` (signed out of).
    fn keep_servers(&mut self, servers: &[i64]) {
        self.changes
            .retain(|change| servers.contains(&change.server));
    }
}

impl Library {
    /// Reads the changes left from last time, and sends them shortly.
    pub(super) fn load_outbox(&mut self, cx: &mut Context<Self>) {
        let saved = self.profile.db.setting(SETTING).ok().flatten();
        self.outbox = Outbox::from_json(&saved.unwrap_or_default());
        if !self.outbox.is_empty() {
            eprintln!(
                "[jellyfin] {} change(s) from last time still to send",
                self.outbox.len()
            );
            self.deliver_soon(FIRST_TRY, cx);
        }
    }

    /// Writes a change down before it's sent. Returns its number, to cross
    /// it off by once sent.
    pub(super) fn remember(&mut self, server: i64, item: &str, change: Change) -> u64 {
        let sequence = self.outbox.remember(server, item, change, super::now());
        self.save_outbox();
        sequence
    }

    /// The server has it, or it can never be sent (refused): crossed off.
    pub(super) fn forget(&mut self, sequence: u64) {
        if self.outbox.forget(sequence) {
            self.save_outbox();
        }
    }

    /// After sending a change failed: kept if the server couldn't be
    /// reached, to try again.
    pub(super) fn sending_failed(
        &mut self,
        sequence: u64,
        error: &anyhow::Error,
        cx: &mut Context<Self>,
    ) {
        if jellyfin::is_offline(error) {
            self.deliver_soon(RETRY, cx);
        } else {
            self.forget(sequence);
        }
        cx.notify();
    }

    fn save_outbox(&self) {
        if let Err(e) = self.profile.db.set_setting(SETTING, &self.outbox.to_json()) {
            eprintln!("[jellyfin] saving changes to send later: {e}");
        }
    }

    /// Reports get through again: send what's waiting, and bring the
    /// library up to date (all of it if it couldn't be fetched).
    pub(super) fn server_back(&mut self, cx: &mut Context<Self>) {
        self.deliver_now(cx);
        if self.error.is_some() {
            self.refresh_remote(cx);
        } else {
            self.catch_up(cx);
        }
    }

    /// The server answered again (or might have): send what's waiting now.
    pub(super) fn deliver_now(&mut self, cx: &mut Context<Self>) {
        if !self.outbox.is_empty() {
            self.deliver_soon(Duration::ZERO, cx);
        }
    }

    fn deliver_soon(&mut self, wait: Duration, cx: &mut Context<Self>) {
        if self.outbox.delivering {
            return;
        }
        self.outbox.delivering = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let Ok(round) = this.update(cx, |this, _| this.delivery_round()) else {
                return;
            };
            let results = cx
                .background_executor()
                .spawn(async move {
                    round
                        .into_iter()
                        .map(|delivery| {
                            let sent = deliver(&delivery);
                            (delivery.change, sent)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| this.delivered(results, cx)).ok();
        })
        .detach();
    }

    /// Every waiting change whose server is still signed in, ready to send.
    fn delivery_round(&self) -> Vec<Delivery> {
        self.outbox
            .changes
            .iter()
            .filter_map(|change| {
                Some(Delivery {
                    client: self.client(change.server)?,
                    user_id: self.catalog.user_id(change.server)?,
                    change: change.clone(),
                })
            })
            .collect()
    }

    /// Takes in how a round of sending went, and plans the next if needed.
    fn delivered(&mut self, results: Vec<(Undelivered, Result<Sent>)>, cx: &mut Context<Self>) {
        self.outbox.delivering = false;
        let mut offline = false;
        for (change, result) in results {
            match result {
                Ok(Sent::Delivered) => {
                    self.forget(change.sequence);
                    eprintln!("[jellyfin] sent a change made while offline");
                    self.confirm_play_state(change.server, change.item, cx);
                }
                Ok(Sent::PlayedElsewhereSince) => {
                    self.forget(change.sequence);
                    eprintln!("[jellyfin] dropped a change: played elsewhere since");
                }
                Err(e) if jellyfin::is_offline(&e) => offline = true,
                Err(e) => {
                    eprintln!("[jellyfin] sending a change made while offline: {e:#}");
                    self.forget(change.sequence);
                }
            }
        }
        // Signed out of a server meanwhile: nothing to send it to.
        let servers: Vec<i64> = self.catalog.servers().iter().map(|s| s.id).collect();
        self.outbox.keep_servers(&servers);
        self.save_outbox();
        if offline || !self.outbox.is_empty() {
            self.deliver_soon(RETRY, cx);
        }
        cx.notify();
    }
}

/// A change on its way to its server.
struct Delivery {
    client: Client,
    user_id: String,
    change: Undelivered,
}

/// What became of a change the server could be asked about.
enum Sent {
    Delivered,
    /// Dropped: the server knows better.
    PlayedElsewhereSince,
}

/// Sends one change, unless the item has been played elsewhere since.
fn deliver(delivery: &Delivery) -> Result<Sent> {
    let Delivery {
        client,
        user_id,
        change,
    } = delivery;
    let now = client.user_data(user_id, &change.item)?;
    let later_elsewhere = now
        .last_played_date
        .as_deref()
        .and_then(jellyfin::parse_timestamp)
        .is_some_and(|played| played > change.at);
    if later_elsewhere {
        return Ok(Sent::PlayedElsewhereSince);
    }
    match change.change {
        Change::Watched(watched) => client.set_played(user_id, &change.item, watched)?,
        Change::StoppedAt(position) => {
            // The server learns positions from playback reports.
            let session = PlaySession {
                id: PlaySession::new_id(),
                transcoding: false,
            };
            let item = &change.item;
            client.report(PlaybackReport::Started, item, position, true, &session)?;
            client.report(PlaybackReport::Stopped, item, position, true, &session)?;
        }
    }
    Ok(Sent::Delivered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(outbox: &Outbox) -> Vec<(i64, &str, Change)> {
        outbox
            .changes
            .iter()
            .map(|change| (change.server, change.item.as_str(), change.change))
            .collect()
    }

    #[test]
    fn a_newer_change_to_an_item_replaces_the_older_one() {
        let mut outbox = Outbox::default();
        outbox.remember(1, "film", Change::StoppedAt(100.0), 10);
        outbox.remember(1, "other", Change::Watched(true), 11);
        let newest = outbox.remember(1, "film", Change::Watched(true), 12);
        assert_eq!(
            items(&outbox),
            [
                (1, "other", Change::Watched(true)),
                (1, "film", Change::Watched(true)),
            ]
        );
        assert_eq!(outbox.changes[1].sequence, newest);
        assert_eq!(outbox.changes[1].at, 12);
    }

    #[test]
    fn the_same_item_id_on_another_server_is_a_change_of_its_own() {
        let mut outbox = Outbox::default();
        outbox.remember(1, "film", Change::StoppedAt(100.0), 10);
        outbox.remember(2, "film", Change::StoppedAt(200.0), 11);
        assert_eq!(outbox.len(), 2);
    }

    #[test]
    fn a_delivered_change_is_crossed_off_once() {
        let mut outbox = Outbox::default();
        let first = outbox.remember(1, "film", Change::StoppedAt(100.0), 10);
        let second = outbox.remember(1, "other", Change::Watched(false), 11);
        assert!(outbox.forget(first));
        assert!(!outbox.forget(first));
        assert_eq!(items(&outbox), [(1, "other", Change::Watched(false))]);
        assert!(outbox.forget(second));
        assert!(outbox.is_empty());
    }

    #[test]
    fn a_replaced_change_can_no_longer_be_crossed_off() {
        let mut outbox = Outbox::default();
        let older = outbox.remember(1, "film", Change::StoppedAt(100.0), 10);
        outbox.remember(1, "film", Change::StoppedAt(200.0), 11);
        // The older report getting through must not cross off the newer.
        assert!(!outbox.forget(older));
        assert_eq!(items(&outbox), [(1, "film", Change::StoppedAt(200.0))]);
    }

    #[test]
    fn changes_for_a_server_signed_out_of_are_dropped() {
        let mut outbox = Outbox::default();
        outbox.remember(1, "film", Change::Watched(true), 10);
        outbox.remember(2, "film", Change::Watched(true), 11);
        outbox.keep_servers(&[2]);
        assert_eq!(items(&outbox), [(2, "film", Change::Watched(true))]);
    }

    #[test]
    fn changes_survive_a_restart_and_numbering_carries_on() {
        let mut outbox = Outbox::default();
        outbox.remember(1, "e836", Change::StoppedAt(137.0), 1_763_490_000);
        outbox.remember(1, "e837", Change::Watched(true), 1_763_490_050);

        let mut back = Outbox::from_json(&outbox.to_json());
        assert_eq!(back, outbox);
        assert_eq!(back.remember(1, "e838", Change::Watched(false), 0), 3);
    }

    #[test]
    fn changes_saved_by_earlier_builds_are_still_read() {
        let saved =
            r#"[{"server":1,"item":"e836","change":{"StoppedAt":137.0},"at":1763490000,"seq":3}]"#;
        let outbox = Outbox::from_json(saved);
        assert_eq!(items(&outbox), [(1, "e836", Change::StoppedAt(137.0))]);
        assert_eq!(outbox.sequence, 3);
    }

    #[test]
    fn an_unreadable_save_starts_an_empty_outbox() {
        assert!(Outbox::from_json("").is_empty());
        assert!(Outbox::from_json("{not json").is_empty());
    }
}
