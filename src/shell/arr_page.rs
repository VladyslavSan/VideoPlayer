//! A server's Sonarr or Radarr, in the sidebar under its libraries. Its page
//! has everything the app keeps track of, as cards (built only for the rows
//! on screen), what it's downloading, and a field for adding to it by name.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClickEvent, Context, ElementId, Entity, FontWeight, SharedString,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, rgba, uniform_list,
};

use super::browse::{SORT_BAR_HEIGHT, columns};
use super::finder::AppKey;
use super::{ACCENT, Card, ERROR, HEADER_HEIGHT, MUTED, Screen, Shell, TEXT};
use crate::arr::{Art, Kind, Subject, Tracked};
use crate::ui::text_input::TextInput;

/// The list is asked again on opening the page after this long.
const STALE: Duration = Duration::from_secs(120);
/// Pictures fetched at once.
const FETCHING: usize = 4;

/// The apps' own colors, for their dots.
fn color(kind: Kind) -> u32 {
    match kind {
        Kind::Sonarr => 0x35c5f4,
        Kind::Radarr => 0xffc230,
    }
}

fn dot(kind: Kind, size: f32) -> gpui::Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(rgb(color(kind)))
}

fn noun(kind: Kind) -> &'static str {
    match kind {
        Kind::Sonarr => "show",
        Kind::Radarr => "movie",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Tab {
    #[default]
    Library,
    Activity,
}

#[derive(Default)]
struct List {
    items: Option<Arc<Vec<Tracked>>>,
    loading: bool,
    error: Option<SharedString>,
    fetched: Option<Instant>,
}

enum Picture {
    Loading,
    Ready,
    Failed,
}

/// What the Sonarr and Radarr pages show, and how.
#[derive(Default)]
pub(super) struct ArrPages {
    lists: HashMap<AppKey, List>,
    pictures: HashMap<PathBuf, Picture>,
    /// Pictures still to fetch; the newest asked (on screen now) first.
    queue: VecDeque<(AppKey, i64, Art, PathBuf)>,
    fetching: usize,
    tab: Tab,
    missing_only: bool,
    pub(super) scroll: UniformListScrollHandle,
    /// The scroll of the page just left, for its fade.
    pub(super) left_scroll: Option<UniformListScrollHandle>,
}

impl ArrPages {
    /// The app's list, once it's been fetched.
    pub(super) fn list(&self, key: AppKey) -> Option<Arc<Vec<Tracked>>> {
        self.lists.get(&key)?.items.clone()
    }
}

/// FNV-1a: a stable name for a cached file.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

impl Shell {
    /// The name field on the app's page.
    pub(super) fn arr_input(&self, kind: Kind) -> &Entity<TextInput> {
        &self.arr_search[match kind {
            Kind::Sonarr => 0,
            Kind::Radarr => 1,
        }]
    }

    /// The app's line in the sidebar: its dot, its name, and how many
    /// downloads it has going.
    pub(super) fn arr_entry(&self, server: i64, kind: Kind, cx: &mut Context<Self>) -> AnyElement {
        let screen = Screen::Arr(server, kind);
        let selected = self.tab == screen;
        let active = self.downloads.active_for((server, kind));
        div()
            .id(SharedString::from(format!("side-{server}-{}", kind.name())))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .pl_6()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .text_color(rgb(if selected { TEXT } else { MUTED }))
            .when(selected, |d| d.bg(rgba(0xffffff1a)))
            .when(!selected, |d| d.hover(|s| s.bg(rgba(0xffffff0d))))
            .child(dot(kind, 8.0))
            .child(div().flex_1().min_w(px(0.0)).truncate().child(kind.name()))
            .when(active > 0, |d| d.child(badge(active)))
            .on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.set_tab(screen.clone(), cx)),
            )
            .into_any_element()
    }

    /// Opening the page: its library, at the top, with an empty name field.
    pub(super) fn arr_page_opened(&mut self, server: i64, kind: Kind, cx: &mut Context<Self>) {
        self.arr_input(kind)
            .clone()
            .update(cx, |input, cx| input.set_text("", cx));
        self.arr_pages.tab = Tab::Library;
        // The page being left (if it's one of these) fades out where it was.
        let left = std::mem::replace(&mut self.arr_pages.scroll, UniformListScrollHandle::new());
        self.arr_pages.left_scroll = Some(left);
        self.refresh_downloads(cx);
        self.load_arr_list((server, kind), false, cx);
    }

    /// Asks the app for its list (`again`: even if it was just asked).
    pub(super) fn load_arr_list(&mut self, key: AppKey, again: bool, cx: &mut Context<Self>) {
        let Some(client) = self.arr_client(key.0, key.1) else {
            return;
        };
        let list = self.arr_pages.lists.entry(key).or_default();
        let fresh = list.fetched.is_some_and(|at| at.elapsed() < STALE);
        if list.loading || (fresh && !again) {
            return;
        }
        list.loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.library() })
                .await;
            this.update(cx, |this, cx| {
                let list = this.arr_pages.lists.entry(key).or_default();
                list.loading = false;
                list.fetched = Some(Instant::now());
                match result {
                    Ok(items) => {
                        list.items = Some(Arc::new(items));
                        list.error = None;
                    }
                    Err(e) => {
                        eprintln!("[{}] library: {e:#}", key.1.name());
                        list.error = Some(format!("Couldn't ask {}: {e:#}", key.1.name()).into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The card's picture, if it's here; asks for it if not.
    fn arr_picture(
        &mut self,
        key: AppKey,
        item: &Tracked,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let (art, _) = item.art.as_ref()?;
        self.arr_art(key, item, *art, cx)
    }

    /// A picture of it, if it's here; asks for it if not. The backdrop
    /// (full size) is only for those with fanart.
    pub(super) fn arr_art(
        &mut self,
        key: AppKey,
        item: &Tracked,
        art: Art,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let (has, url) = item.art.as_ref()?;
        if art == Art::Backdrop && *has != Art::Fanart {
            return None;
        }
        let name = format!(
            "{}-{}-{}-{:016x}{}.jpg",
            key.1.name().to_lowercase(),
            key.0,
            item.id,
            fnv(url),
            if art == Art::Backdrop { "-full" } else { "" }
        );
        let path = self.paths.arr_pictures().join(name);
        match self.arr_pages.pictures.get(&path) {
            Some(Picture::Ready) => return Some(path),
            Some(_) => return None,
            None => {}
        }
        if path.exists() {
            self.arr_pages.pictures.insert(path.clone(), Picture::Ready);
            return Some(path);
        }
        self.arr_pages
            .pictures
            .insert(path.clone(), Picture::Loading);
        self.arr_pages.queue.push_front((key, item.id, art, path));
        self.fetch_pictures(cx);
        None
    }

    fn fetch_pictures(&mut self, cx: &mut Context<Self>) {
        while self.arr_pages.fetching < FETCHING {
            let Some((key, id, art, path)) = self.arr_pages.queue.pop_front() else {
                return;
            };
            let Some(client) = self.arr_client(key.0, key.1) else {
                continue;
            };
            self.arr_pages.fetching += 1;
            cx.spawn(async move |this, cx| {
                let out = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { client.fetch_art(id, art, &out) })
                    .await;
                this.update(cx, |this, cx| {
                    this.arr_pages.fetching -= 1;
                    let picture = match result {
                        Ok(()) => Picture::Ready,
                        Err(e) => {
                            eprintln!("[{}] picture {id}: {e:#}", key.1.name());
                            Picture::Failed
                        }
                    };
                    this.arr_pages.pictures.insert(path, picture);
                    this.fetch_pictures(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The list as shown: everything, or just what's missing.
    fn arr_shown(&self, key: AppKey) -> Vec<Tracked> {
        let Some(items) = self.arr_pages.lists.get(&key).and_then(|l| l.items.clone()) else {
            return Vec::new();
        };
        if self.arr_pages.missing_only {
            items.iter().filter(|t| t.missing()).cloned().collect()
        } else {
            items.to_vec()
        }
    }

    fn arr_card(&mut self, key: AppKey, item: &Tracked, cx: &mut Context<Self>) -> AnyElement {
        let (server, kind) = key;
        let thumb = self.arr_picture(key, item, cx);
        let subtitle = match item.year {
            Some(year) => format!("{year} · {}", item.status()),
            None => item.status(),
        };
        let card = Card {
            id: format!("arr-{server}-{}-{}", kind.name(), item.id).into(),
            thumb,
            title: item.title.clone(),
            subtitle: Some(subtitle),
            progress: None,
            watched: false,
            highlight: false,
            menu: None,
        };
        let subject = match kind {
            Kind::Sonarr => Subject::Episodes {
                show: item.title.clone(),
                year: item.year,
                episodes: Vec::new(),
            },
            Kind::Radarr => Subject::Movie {
                title: item.title.clone(),
                year: item.year,
            },
        };
        // In the Jellyfin library: its page there. If not (yet), its page
        // from what Sonarr or Radarr knows, to get it from.
        let target = self.library_target(server, &subject);
        let id = item.id;
        self.card(card, None, cx, move |this, _, cx| match &target {
            Some(target) => this.open_target(target, cx),
            None => this.open_tracked(server, kind, id, cx),
        })
    }

    pub(super) fn arr_page(
        &self,
        server: i64,
        kind: Kind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = (server, kind);
        let query = self.arr_input(kind).read(cx).text().trim().to_string();
        let body = if !query.is_empty() {
            self.arr_sections(self.arr_add_results(key, &query, cx))
        } else if self.arr_pages.tab == Tab::Activity {
            self.arr_sections(self.app_activity(key, cx))
        } else {
            self.arr_grid(key, window, cx)
        };
        let bar = self.arr_bar(key, cx);
        div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(body)
            // With frosted bars, drawn after the frost, like the header.
            .child(if super::FROSTED {
                gpui::deferred(bar).into_any_element()
            } else {
                bar
            })
            .into_any_element()
    }

    /// Sections, scrolling as one page under the bar.
    fn arr_sections(&self, sections: Vec<AnyElement>) -> AnyElement {
        let mut page = div()
            .id("arr-content")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.content_scroll)
            .px_8()
            .pt(px(HEADER_HEIGHT + SORT_BAR_HEIGHT - 16.0))
            .pb_10()
            .children(sections);
        page.style().restrict_scroll_to_axis = Some(true);
        page.into_any_element()
    }

    fn arr_add_results(&self, key: AppKey, query: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rows = if self.finder_shows(key, query) {
            self.finder_rows(key, cx)
        } else {
            vec![
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Keep typing: at least three letters.")
                    .into_any_element(),
            ]
        };
        vec![Self::section(
            &format!("Add a {} to {}", noun(key.1), key.1.name()),
            div().flex().flex_col().gap_2().children(rows),
        )]
    }

    /// Every card, built only for the rows on screen.
    fn arr_grid(&self, key: AppKey, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let list = self.arr_pages.lists.get(&key);
        let message = |text: String, color: u32| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(color))
                .child(text)
                .into_any_element()
        };
        let Some(items) = list.and_then(|l| l.items.as_ref()) else {
            return match list.and_then(|l| l.error.clone()) {
                Some(error) => message(error.to_string(), ERROR),
                None => message(format!("Asking {}…", key.1.name()), MUTED),
            };
        };
        let count = if self.arr_pages.missing_only {
            items.iter().filter(|t| t.missing()).count()
        } else {
            items.len()
        };
        if count == 0 {
            return message(
                if self.arr_pages.missing_only {
                    "Nothing missing.".into()
                } else {
                    format!("{} has no {}s yet.", key.1.name(), noun(key.1))
                },
                MUTED,
            );
        }
        let columns = columns(window);
        let rows = count.div_ceil(columns);
        uniform_list(
            ElementId::Name(format!("arr-grid-{}-{}", key.0, key.1.name()).into()),
            rows,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let shown = this.arr_shown(key);
                let (start, end) = (
                    range.start * columns,
                    (range.end * columns).min(shown.len()),
                );
                let cards: Vec<AnyElement> = shown[start.min(end)..end]
                    .iter()
                    .map(|item| this.arr_card(key, item, cx))
                    .collect();
                let mut cards = cards.into_iter();
                range
                    .map(|_| {
                        div()
                            .flex()
                            .gap_5()
                            .px_8()
                            .pb_5()
                            .children(cards.by_ref().take(columns))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.arr_pages.scroll.clone())
        .flex_1()
        .min_h(px(0.0))
        .pt(px(HEADER_HEIGHT + SORT_BAR_HEIGHT))
        .into_any_element()
    }

    /// The bar under the header: which app, Library or Activity, and the
    /// field for adding to it.
    fn arr_bar(&self, key: AppKey, cx: &mut Context<Self>) -> AnyElement {
        let (server, kind) = key;
        let items = self.arr_pages.lists.get(&key).and_then(|l| l.items.clone());
        let total = items.as_ref().map(|i| i.len());
        let missing = items
            .as_ref()
            .map(|i| i.iter().filter(|t| t.missing()).count());
        let adding = !self.arr_input(kind).read(cx).text().trim().is_empty();
        let tab = self.arr_pages.tab;
        let everything = !adding && tab == Tab::Library && !self.arr_pages.missing_only;
        let library = self.arr_chip("library", "Library", everything, total, cx, |this| {
            this.arr_pages.tab = Tab::Library;
            this.arr_pages.missing_only = false;
        });
        let missing_chip = self.arr_chip(
            "missing",
            "Missing",
            !adding && tab == Tab::Library && self.arr_pages.missing_only,
            missing,
            cx,
            |this| {
                this.arr_pages.tab = Tab::Library;
                this.arr_pages.missing_only = true;
            },
        );
        let active = self.downloads.active_for(key);
        let activity = self.arr_chip(
            "activity",
            "Activity",
            !adding && tab == Tab::Activity,
            Some(active),
            cx,
            |this| this.arr_pages.tab = Tab::Activity,
        );
        div()
            .id("arr-bar")
            .absolute()
            .top(px(HEADER_HEIGHT))
            .left_0()
            .right_0()
            .h(px(SORT_BAR_HEIGHT))
            .bg(super::bar_background(0.0, super::BAR_JOIN_OPACITY, 0.0))
            .when(super::FROSTED, |bar| {
                bar.border_b_1().border_color(rgba(super::FROST_EDGE))
            })
            .occlude()
            .flex()
            .items_center()
            .gap_2()
            .px_8()
            .pb_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mr_3()
                    .child(dot(kind, 10.0))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(kind.name()))
                    .when(self.servers.len() > 1, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .child(self.source_name(Some(server))),
                        )
                    }),
            )
            .child(library)
            .child(missing_chip)
            .child(activity)
            .child(div().flex_1())
            .child(
                div()
                    .w(px(300.0))
                    .flex_none()
                    .child(self.arr_input(kind).clone()),
            )
            .into_any_element()
    }

    fn arr_chip(
        &self,
        id: &str,
        label: &str,
        on: bool,
        count: Option<usize>,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut Self) + 'static,
    ) -> AnyElement {
        let kind = match self.page_screen() {
            Screen::Arr(_, kind) => Some(kind),
            _ => None,
        };
        div()
            .id(SharedString::from(format!("arr-{id}")))
            .flex()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_1()
            .rounded_full()
            .text_sm()
            .cursor_pointer()
            .when(on, |d| d.bg(rgba(0xffffff26)))
            .hover(|d| d.bg(rgba(0xffffff1f)))
            .child(label.to_string())
            .children(
                count
                    .filter(|n| *n > 0)
                    .map(|n| div().text_xs().text_color(rgb(MUTED)).child(n.to_string())),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                // Picking a view leaves the name field.
                if let Some(kind) = kind {
                    this.arr_input(kind)
                        .clone()
                        .update(cx, |input, cx| input.set_text("", cx));
                }
                this.arr_pages.scroll = UniformListScrollHandle::new();
                pick(this);
                cx.notify();
            }))
            .into_any_element()
    }
}

fn badge(n: usize) -> gpui::Div {
    div()
        .flex_none()
        .px_2()
        .rounded_full()
        .text_xs()
        .text_color(rgb(TEXT))
        .bg(rgb(ACCENT))
        .child(n.to_string())
}
