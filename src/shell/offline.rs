//! What the server couldn't be told because it couldn't be reached: where
//! a video stopped, an item marked watched. Each is written down before it
//! is sent and crossed off once the server has it, so neither a lost
//! connection nor closing the app loses it. Once the server answers again,
//! they go out, unless the item has been played elsewhere since (then the
//! server knows better).

use std::time::Duration;

use anyhow::Result;
use gpui::Context;
use serde::{Deserialize, Serialize};

use super::{Shell, new_session_id};
use crate::jellyfin::{self, Client, PlaySession, PlaybackReport, parse_timestamp};

const SETTING: &str = "undelivered";
/// Tries again this often while the server can't be reached.
const RETRY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(super) enum Change {
    /// Playback stopped here (at the runtime when finished).
    StoppedAt(f64),
    Watched(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Undelivered {
    pub server: i64,
    pub item: String,
    pub change: Change,
    /// When it happened (Unix seconds): anything the server has from later
    /// wins.
    pub at: i64,
    /// Tells apart two changes to the same item.
    pub seq: u64,
}

impl Shell {
    pub(super) fn load_undelivered(&mut self, cx: &mut Context<Self>) {
        let saved = self.db.setting(SETTING).ok().flatten().unwrap_or_default();
        self.undelivered = serde_json::from_str(&saved).unwrap_or_default();
        self.undelivered_seq = self.undelivered.iter().map(|u| u.seq).max().unwrap_or(0);
        if !self.undelivered.is_empty() {
            eprintln!(
                "[jellyfin] {} change(s) from last time still to send",
                self.undelivered.len()
            );
            self.deliver_soon(Duration::from_secs(2), cx);
        }
    }

    /// Writes a change down before it's sent; it replaces any older one for
    /// the same item. Returns it, to cross off once sent.
    pub(super) fn remember(&mut self, server: i64, item: &str, change: Change) -> Undelivered {
        self.undelivered_seq += 1;
        let entry = Undelivered {
            server,
            item: item.to_string(),
            change,
            at: super::now(),
            seq: self.undelivered_seq,
        };
        self.undelivered
            .retain(|u| !(u.server == server && u.item == item));
        self.undelivered.push(entry.clone());
        self.save_undelivered();
        entry
    }

    /// The server has it, or it can never be sent (refused): crossed off.
    pub(super) fn forget(&mut self, seq: u64) {
        let before = self.undelivered.len();
        self.undelivered.retain(|u| u.seq != seq);
        if self.undelivered.len() != before {
            self.save_undelivered();
        }
    }

    /// After sending `entry` failed: kept if the server couldn't be
    /// reached, to try again.
    pub(super) fn sending_failed(
        &mut self,
        seq: u64,
        error: &anyhow::Error,
        cx: &mut Context<Self>,
    ) {
        if jellyfin::is_offline(error) {
            self.deliver_soon(RETRY, cx);
        } else {
            self.forget(seq);
        }
        cx.notify();
    }

    fn save_undelivered(&self) {
        let json = serde_json::to_string(&self.undelivered).unwrap_or_default();
        if let Err(e) = self.db.set_setting(SETTING, &json) {
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
        if !self.undelivered.is_empty() {
            self.deliver_soon(Duration::ZERO, cx);
        }
    }

    fn deliver_soon(&mut self, wait: Duration, cx: &mut Context<Self>) {
        if self.delivering {
            return;
        }
        self.delivering = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let round = this.update(cx, |this, _| {
                this.undelivered
                    .iter()
                    .filter_map(|u| {
                        let client = this.client(u.server)?;
                        let user_id = this.user_id(u.server)?;
                        Some((u.clone(), client, user_id))
                    })
                    .collect::<Vec<_>>()
            });
            let Ok(round) = round else {
                return;
            };
            let results = cx
                .background_executor()
                .spawn(async move {
                    round
                        .into_iter()
                        .map(|(u, client, user_id)| {
                            let result = deliver(&client, &user_id, &u);
                            (u, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.delivering = false;
                let mut offline = false;
                for (u, result) in results {
                    match result {
                        Ok(sent) => {
                            this.forget(u.seq);
                            if sent {
                                eprintln!("[jellyfin] sent a change made while offline");
                                this.confirm_play_state(u.server, u.item.clone(), cx);
                            } else {
                                eprintln!("[jellyfin] dropped a change: played elsewhere since");
                            }
                        }
                        Err(e) if jellyfin::is_offline(&e) => offline = true,
                        Err(e) => {
                            eprintln!("[jellyfin] sending a change made while offline: {e:#}");
                            this.forget(u.seq);
                        }
                    }
                }
                // Signed out of a server meanwhile: nothing to send it to.
                let servers: Vec<i64> = this.servers.iter().map(|s| s.id).collect();
                this.undelivered.retain(|u| servers.contains(&u.server));
                this.save_undelivered();
                if offline || !this.undelivered.is_empty() {
                    this.deliver_soon(RETRY, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Sends one change. False if it was dropped because the item has been
/// played elsewhere since.
fn deliver(client: &Client, user_id: &str, u: &Undelivered) -> Result<bool> {
    let now = client.user_data(user_id, &u.item)?;
    let later_elsewhere = now
        .last_played_date
        .as_deref()
        .and_then(parse_timestamp)
        .is_some_and(|played| played > u.at);
    if later_elsewhere {
        return Ok(false);
    }
    match u.change {
        Change::Watched(watched) => client.set_played(user_id, &u.item, watched)?,
        Change::StoppedAt(position) => {
            // The server learns positions from playback reports.
            let session = PlaySession {
                id: new_session_id(),
                transcoding: false,
            };
            client.report(PlaybackReport::Started, &u.item, position, true, &session)?;
            client.report(PlaybackReport::Stopped, &u.item, position, true, &session)?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_survive_a_restart() {
        let saved = vec![Undelivered {
            server: 1,
            item: "e836".into(),
            change: Change::StoppedAt(137.0),
            at: 1_763_490_000,
            seq: 3,
        }];
        let json = serde_json::to_string(&saved).unwrap();
        let back: Vec<Undelivered> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, saved);
    }
}
