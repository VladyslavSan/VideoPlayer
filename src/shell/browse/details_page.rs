//! A movie's page, and what the servers know beyond the library for it and
//! for show pages: overview, genres, ratings, stream names and a large
//! backdrop, asked for when the page opens. Local files show what the
//! library knows.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Window, div, prelude::*, px, rgb};

use super::{Event, Pages, StartAt};
use crate::jellyfin::Client;
use crate::library::{Item, Source};
use crate::shell::details::{self, Details, Hero, Overview, format_runtime};
use crate::shell::{MUTED, widgets};

/// Width of the backdrop downloaded for a details page.
const BACKDROP_WIDTH: u32 = 1920;
const HERO_HEIGHT: f32 = 420.0;

/// What a show's details are kept under.
pub(super) fn show_key(show: &str) -> String {
    format!("show:{show}")
}

/// Whose details to ask a server for, given an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Subject {
    /// The item itself.
    Item,
    /// The show the item is an episode of.
    Show,
}

/// What the servers said, by item key or `show_key`. Each is asked for
/// once per session.
#[derive(Default)]
pub(super) struct Fetched {
    ready: HashMap<String, Details>,
    loading: HashSet<String>,
}

impl Fetched {
    pub(super) fn get(&self, key: &str) -> Option<&Details> {
        self.ready.get(key)
    }

    pub(super) fn is_loading(&self, key: &str) -> bool {
        self.loading.contains(key)
    }

    /// Whether to ask for `key` now: it isn't here or on its way. If so,
    /// it counts as on its way from here on.
    fn start(&mut self, key: &str) -> bool {
        !self.ready.contains_key(key) && self.loading.insert(key.to_string())
    }

    /// The answer for `key` came, or (None) it won't.
    fn finish(&mut self, key: String, details: Option<Details>) {
        self.loading.remove(&key);
        if let Some(details) = details {
            self.ready.insert(key, details);
        }
    }
}

/// Fetches the item's info, and its backdrop at full width (cached on disk).
fn fetch(client: &Client, user_id: &str, item_id: &str, thumbs: &Path) -> anyhow::Result<Details> {
    let info = client.item_info(user_id, item_id)?;
    let backdrop = info.backdrop().and_then(|image| {
        let path = image.cache_path_width(thumbs, BACKDROP_WIDTH);
        if !path.exists()
            && let Err(e) = client.download_image_width(&image, &path, BACKDROP_WIDTH)
        {
            eprintln!("[jellyfin] backdrop: {e:#}");
            return None;
        }
        Some(path)
    });
    Ok(Details::from_info(&info, backdrop))
}

/// What a movie's Play button says: Resume, with what's left, once begun.
fn play_label(item: &Item) -> String {
    match (item.progress().is_some(), item.duration) {
        (true, Some(d)) => format!(
            "▶  Resume · {} left",
            format_runtime((d - item.position).max(0.0))
        ),
        (true, None) => "▶  Resume".into(),
        (false, _) => "▶  Play".into(),
    }
}

/// The facts under a page's title: year, length, ratings, a few genres.
pub(super) fn facts(
    item_year: Option<u16>,
    duration: Option<f64>,
    details: Option<&Details>,
) -> Vec<String> {
    let mut facts = Vec::new();
    facts.extend(item_year.map(|y| y.to_string()));
    facts.extend(duration.filter(|d| *d > 0.0).map(format_runtime));
    if let Some(d) = details {
        facts.extend(d.official_rating.clone());
        facts.extend(d.rating.map(|r| format!("★ {r:.1}")));
        if !d.genres.is_empty() {
            facts.push(
                d.genres
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
    }
    facts
}

/// A labelled line of the page's information, if there is any to show.
fn info_row(label: &str, values: Vec<String>) -> Option<AnyElement> {
    if values.is_empty() {
        return None;
    }
    Some(
        div()
            .flex()
            .gap_6()
            .child(
                div()
                    .w(px(110.0))
                    .flex_none()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(label.to_string()),
            )
            .child(div().flex().flex_col().gap_1().text_sm().children(values))
            .into_any_element(),
    )
}

impl Pages {
    /// Asks the shell for a movie's page, and its server for its details.
    pub(super) fn open_details(&mut self, key: String, cx: &mut Context<Self>) {
        cx.emit(Event::DetailsOpened { key: key.clone() });
        let item = self.catalog.item(&key).cloned();
        if let Some(item) = item {
            self.load_details(key, &item, Subject::Item, cx);
        }
        cx.notify();
    }

    /// Asks the item's server for the details of `subject`, kept under
    /// `key`. Local files have none.
    pub(super) fn load_details(
        &mut self,
        key: String,
        item: &Item,
        subject: Subject,
        cx: &mut Context<Self>,
    ) {
        let Source::Jellyfin { server, id } = &item.source else {
            return;
        };
        if !self.details.start(&key) {
            return;
        }
        let (Some(client), Some(user_id)) = (self.client(*server), self.catalog.user_id(*server))
        else {
            self.details.finish(key, None);
            return;
        };
        let id = id.clone();
        let thumbs = self.profile.paths.thumbs.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let target = match subject {
                        // An episode knows its series; the series has the overview.
                        Subject::Show => client.item_info(&user_id, &id)?.series_id.unwrap_or(id),
                        Subject::Item => id,
                    };
                    fetch(&client, &user_id, &target, &thumbs)
                })
                .await;
            this.update(cx, |this, cx| this.details_loaded(key, result, cx))
                .ok();
        })
        .detach();
    }

    /// A server answered (or failed to) about `key`.
    fn details_loaded(
        &mut self,
        key: String,
        result: anyhow::Result<Details>,
        cx: &mut Context<Self>,
    ) {
        let details = result
            .map_err(|e| eprintln!("[jellyfin] details: {e:#}"))
            .ok();
        let loaded = details.is_some();
        self.details.finish(key.clone(), details);
        // A show's page can now find it in Sonarr.
        if loaded && let Some(show) = key.strip_prefix("show:") {
            self.load_sonarr(show.to_string(), cx);
        }
        cx.notify();
    }

    /// A movie's page.
    pub(in crate::shell) fn details_page(
        &self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(item) = self.catalog.item(key) else {
            return vec![
                div()
                    .pt_10()
                    .text_color(rgb(MUTED))
                    .child("This item is no longer in the library.")
                    .into_any_element(),
            ];
        };
        let details = self.details.get(key);
        let mut images: Vec<PathBuf> = item
            .thumb
            .iter()
            .chain(details.and_then(|d| d.backdrop.as_ref()))
            .cloned()
            .collect();
        images.dedup();
        let images = details::ready_pictures(images, window, cx);
        let hero = Hero {
            height: HERO_HEIGHT,
            images,
            title: item.title.clone(),
            facts: facts(item.year, item.duration, details),
            tagline: details.and_then(|d| d.tagline.clone()),
        };
        let mut page = vec![details::hero(&self.motion, hero)];
        page.push(self.details_buttons(item, cx));
        let about = Overview {
            details,
            loading: self.details.is_loading(key),
            lines: None,
        };
        page.extend(details::overview(&self.motion, about));

        let mut info = Vec::new();
        if let Some(d) = details {
            info.extend(info_row("Video", d.video.clone()));
            info.extend(info_row("Audio", d.audio.clone()));
            info.extend(info_row("Subtitles", d.subtitles.clone()));
        }
        if let Source::Local { path, .. } = &item.source {
            info.extend(info_row("File", vec![path.display().to_string()]));
        }
        if !info.is_empty() {
            page.push(self.motion.arriving(
                format!("details-info-{}", details.is_some()),
                div().flex().flex_col().gap_3().pt_8().children(info),
            ));
        }
        page
    }

    /// A movie's buttons: play (or resume), start over, mark watched.
    fn details_buttons(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let (play_key, restart_key, watched_key) = (item.key(), item.key(), item.key());
        let resume = item.progress().is_some();
        let watched = item.watched;
        div()
            .flex()
            .items_center()
            .gap_3()
            .pt_5()
            .child(widgets::primary_action_button(
                "details-play",
                play_label(item),
                cx,
                move |this, _, cx| this.play(&play_key, StartAt::ResumePoint, cx),
            ))
            .when(resume, |d| {
                d.child(widgets::action_button(
                    "details-restart",
                    "Play from beginning",
                    cx,
                    move |this, _, cx| this.play(&restart_key, StartAt::Beginning, cx),
                ))
            })
            .child(widgets::action_button(
                "details-watched",
                if watched {
                    "Mark as unwatched"
                } else {
                    "Mark as watched"
                },
                cx,
                move |this, _, cx| {
                    this.mark_watched(std::slice::from_ref(&watched_key), !watched, cx)
                },
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_are_asked_for_once() {
        let mut fetched = Fetched::default();
        assert!(fetched.start("jf-1-a"));
        assert!(fetched.is_loading("jf-1-a"));
        // On its way: not asked again.
        assert!(!fetched.start("jf-1-a"));
        fetched.finish("jf-1-a".into(), Some(Details::default()));
        assert!(!fetched.is_loading("jf-1-a"));
        assert!(fetched.get("jf-1-a").is_some());
        // Here: not asked again.
        assert!(!fetched.start("jf-1-a"));
    }

    #[test]
    fn details_that_failed_can_be_asked_for_again() {
        let mut fetched = Fetched::default();
        assert!(fetched.start(&show_key("Dark")));
        fetched.finish(show_key("Dark"), None);
        assert!(fetched.get("show:Dark").is_none());
        assert!(fetched.start("show:Dark"));
    }

    #[test]
    fn facts_list_what_is_known_in_order() {
        assert_eq!(
            facts(Some(1995), Some(6129.0), None),
            ["1995", "1 h 42 min"]
        );
        let details = Details {
            official_rating: Some("R".into()),
            rating: Some(8.26),
            genres: ["Crime", "Drama", "Action", "Thriller"]
                .map(String::from)
                .to_vec(),
            ..Details::default()
        };
        let known = facts(None, Some(0.0), Some(&details));
        assert_eq!(known, ["R", "★ 8.3", "Crime, Drama, Action"]);
    }
}
