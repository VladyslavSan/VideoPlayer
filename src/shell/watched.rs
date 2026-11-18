//! Marking things watched or unwatched by hand, from a card's right-click
//! menu: a movie or episode, every episode up to one, a season, a show.
//! The cards change at once; the server is told in the background.

use std::collections::HashMap;
use std::time::Instant;

use gpui::{AnyElement, ClickEvent, Context, Pixels, Point, Window, prelude::*};

use super::Shell;
use crate::library::Source;
use crate::ui::menu;

/// What a card's menu acts on.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MenuTarget {
    /// A movie or episode, by library key.
    Item(String),
    /// A show, by name.
    Show(String),
    /// A season of a show (0: specials).
    Season(String, u16),
}

/// An open right-click menu, where the click was.
pub(super) struct CardMenu {
    target: MenuTarget,
    at: Point<Pixels>,
}

impl Shell {
    pub(super) fn open_card_menu(
        &mut self,
        target: MenuTarget,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.card_menu = Some(CardMenu { target, at });
        cx.notify();
    }

    pub(super) fn close_card_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.card_menu.take().is_some();
        if open {
            cx.notify();
        }
        open
    }

    /// The choices for the open menu: (label, items to mark, watched).
    fn card_menu_choices(&self, target: &MenuTarget) -> Vec<(String, Vec<String>, bool)> {
        let mut choices = Vec::new();
        match target {
            MenuTarget::Item(key) => {
                let Some(item) = self.items().find(|i| i.key() == *key) else {
                    return choices;
                };
                let unwatch = item.watched && item.progress().is_none();
                choices.push((
                    if unwatch {
                        "Mark as unwatched"
                    } else {
                        "Mark as watched"
                    }
                    .to_string(),
                    vec![key.clone()],
                    !unwatch,
                ));
                let (episodes, current) = self.episodes_around(item);
                if let Some(current) = current {
                    let before = &episodes[..current];
                    if before.iter().any(|e| !e.watched) {
                        let mut keys: Vec<String> = before.iter().map(|e| e.key.clone()).collect();
                        keys.push(key.clone());
                        choices.push(("Mark all up to here as watched".into(), keys, true));
                    }
                    let season = episodes[current].season;
                    let in_season: Vec<String> = episodes
                        .iter()
                        .filter(|e| e.season == season)
                        .map(|e| e.key.clone())
                        .collect();
                    let all_watched = episodes
                        .iter()
                        .filter(|e| e.season == season)
                        .all(|e| e.watched);
                    if in_season.len() > 1 {
                        let name = if season == 0 {
                            "specials".to_string()
                        } else {
                            format!("season {season}")
                        };
                        choices.push(if all_watched {
                            (format!("Mark {name} as unwatched"), in_season, false)
                        } else {
                            (format!("Mark {name} as watched"), in_season, true)
                        });
                    }
                }
            }
            MenuTarget::Show(name) | MenuTarget::Season(name, _) => {
                let season = match target {
                    MenuTarget::Season(_, season) => Some(*season),
                    _ => None,
                };
                let episodes: Vec<_> = self
                    .items()
                    .filter(|i| i.show.as_deref() == Some(name.as_str()))
                    .filter(|i| season.is_none_or(|s| i.season.unwrap_or(0) == s))
                    .collect();
                let what = match season {
                    None => "show".to_string(),
                    Some(0) => "specials".to_string(),
                    Some(n) => format!("season {n}"),
                };
                let keys: Vec<String> = episodes.iter().map(|i| i.key()).collect();
                if episodes.iter().any(|i| !i.watched) {
                    choices.push((format!("Mark {what} as watched"), keys.clone(), true));
                }
                if episodes.iter().any(|i| i.watched || i.progress().is_some()) {
                    choices.push((format!("Mark {what} as unwatched"), keys, false));
                }
            }
        }
        choices
    }

    pub(super) fn card_menu_view(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.card_menu.as_ref()?;
        let items = self
            .card_menu_choices(&menu.target)
            .into_iter()
            .enumerate()
            .map(|(i, (label, keys, watched))| {
                menu::item(("card-menu", i), label, false)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.card_menu = None;
                        this.mark_watched(&keys, watched, cx);
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            return None;
        }
        let shell = cx.entity().downgrade();
        Some(menu::context(menu.at, items, window, move |_, _, cx| {
            shell.update(cx, |this, cx| this.close_card_menu(cx)).ok();
        }))
    }

    /// Marks items watched or unwatched, here and on their servers. Either
    /// way the resume point goes. Items already that way are left alone.
    pub(super) fn mark_watched(&mut self, keys: &[String], watched: bool, cx: &mut Context<Self>) {
        let now = Instant::now();
        // Per server: item ids, and their keys.
        let mut remote: HashMap<i64, (Vec<String>, Vec<String>)> = HashMap::new();
        let mut local_changed = false;
        for key in keys {
            let Some(item) = self.items().find(|i| i.key() == *key).cloned() else {
                continue;
            };
            if item.watched == watched && item.progress().is_none() {
                continue;
            }
            match &item.source {
                Source::Local { id, .. } => {
                    if let Err(e) = self.db.set_watched(*id, watched) {
                        self.error = Some(format!("Couldn't update {}: {e}", item.title).into());
                    }
                    local_changed = true;
                }
                Source::Jellyfin { server, id } => {
                    if let Some(remote) = self.remote.iter_mut().find(|i| i.key() == *key) {
                        remote.watched = watched;
                        remote.position = 0.0;
                        if watched {
                            remote.last_played = Some(super::now());
                        }
                    }
                    // A library fetch already under way mustn't undo it.
                    self.fresh.insert(key.clone(), now);
                    let (ids, keys) = remote.entry(*server).or_default();
                    ids.push(id.clone());
                    keys.push(key.clone());
                }
            }
        }
        if local_changed {
            self.reload();
        }
        for (server, (ids, keys)) in remote {
            let (Some(client), Some(user_id)) = (self.client(server), self.user_id(server)) else {
                continue;
            };
            let count = ids.len();
            eprintln!(
                "[jellyfin] marking {count} item(s) {}",
                if watched { "watched" } else { "unwatched" }
            );
            cx.spawn(async move |this, cx| {
                let failed = cx
                    .background_executor()
                    .spawn(async move { client.set_played_many(&user_id, &ids, watched) })
                    .await;
                if let Err(e) = failed {
                    eprintln!("[jellyfin] marking watched: {e:#}");
                    this.update(cx, |this, cx| {
                        this.error = Some(
                            format!("Couldn't tell the server about {count} item(s): {e:#}").into(),
                        );
                        // Show what the server has instead.
                        this.fresh.retain(|key, _| !keys.contains(key));
                        this.refresh_remote(cx);
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        cx.notify();
    }
}
