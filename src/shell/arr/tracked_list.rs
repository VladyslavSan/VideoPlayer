//! A server's Sonarr or Radarr page: everything the app keeps track of,
//! as cards (built only for the rows on screen), what it's downloading,
//! and a field for adding to it by name.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClickEvent, Context, ElementId, FontWeight, ScrollHandle, SharedString,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, rgba, uniform_list,
};

use super::sidebar::dot;
use super::{Pages, fnv, noun, subject};
use crate::arr::connections::Id;
use crate::arr::{Art, Tracked};
use crate::shell::browse::{SORT_BAR_HEIGHT, columns};
use crate::shell::cards::{Card, Clicks};
use crate::shell::downloads::Activity;
use crate::shell::header::{self, HEADER_HEIGHT};
use crate::shell::navigation::frozen_list;
use crate::shell::{ERROR, MUTED, widgets};

/// The list is asked again on opening the page after this long.
const STALE: Duration = Duration::from_secs(120);
/// Pictures fetched at once.
const FETCHING: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Library,
    Activity,
}

/// Which of what an app tracks its Library tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Filter {
    #[default]
    Everything,
    /// Only what the app is after and hasn't got.
    Missing,
}

/// What a chip on the page's bar picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Library(Filter),
    Activity,
}

#[derive(Default)]
struct List {
    items: Option<Arc<Vec<Tracked>>>,
    loading: bool,
    error: Option<SharedString>,
    fetched: Option<Instant>,
}

impl List {
    /// Whether to ask the app for the list, `since` it was last fetched
    /// (None: never). `again`: even if it was just asked.
    fn needs_asking(&self, since: Option<Duration>, again: bool) -> bool {
        let fresh = since.is_some_and(|since| since < STALE);
        !(self.loading || (fresh && !again))
    }
}

enum Picture {
    Loading,
    Ready,
    Failed,
}

/// A picture still to fetch.
struct Wanted {
    app: Id,
    /// The show's or movie's id in the app.
    id: i64,
    art: Art,
    path: PathBuf,
}

/// What the Sonarr and Radarr pages show, and how.
#[derive(Default)]
pub(super) struct Lists {
    lists: HashMap<Id, List>,
    pictures: HashMap<PathBuf, Picture>,
    /// Pictures still to fetch; the newest asked (on screen now) first.
    queue: VecDeque<Wanted>,
    fetching: usize,
    tab: Tab,
    filter: Filter,
    scroll: UniformListScrollHandle,
    /// The scroll of the page left for another app's, until that's drawn.
    left_scroll: Option<UniformListScrollHandle>,
    /// The scroll of the page being left, for its fade.
    leaving_scroll: UniformListScrollHandle,
}

impl Lists {
    /// The app's list, once it's been fetched.
    pub(super) fn list(&self, app: Id) -> Option<Arc<Vec<Tracked>>> {
        self.lists.get(&app)?.items.clone()
    }
}

/// The items as shown: everything, or just what's missing.
fn shown(items: &[Tracked], filter: Filter) -> Vec<Tracked> {
    match filter {
        Filter::Missing => items.iter().filter(|t| t.missing()).cloned().collect(),
        Filter::Everything => items.to_vec(),
    }
}

/// The file a picture of something an app tracks is kept in. Named by the
/// picture's address in the app, which changes along with the picture.
fn picture_name(app: Id, item: i64, url: &str, art: Art) -> String {
    format!(
        "{}-{}-{}-{:016x}{}.jpg",
        app.kind.name().to_lowercase(),
        app.server,
        item,
        fnv(url),
        if art == Art::Backdrop { "-full" } else { "" }
    )
}

impl Pages {
    /// Opening the page: its library, at the top, with an empty name field.
    pub(in crate::shell) fn opened(&mut self, app: Id, cx: &mut Context<Self>) {
        self.clear_name(app.kind, cx);
        self.lists.tab = Tab::Library;
        // The page being left (if it's one of these) fades out where it was.
        let left = std::mem::replace(&mut self.lists.scroll, UniformListScrollHandle::new());
        self.lists.left_scroll = Some(left);
        self.refresh_downloads(cx);
        self.load_list(app, false, cx);
    }

    /// The page last drawn starts fading out: its list stays scrolled
    /// where it was while it does.
    pub(in crate::shell) fn start_leaving(&mut self) {
        let left = self.lists.left_scroll.take();
        self.lists.leaving_scroll = left.unwrap_or_else(|| frozen_list(&self.lists.scroll));
    }

    /// Asks the app for its list (`again`: even if it was just asked).
    pub(super) fn load_list(&mut self, app: Id, again: bool, cx: &mut Context<Self>) {
        let Some(client) = self.client(app) else {
            return;
        };
        let list = self.lists.lists.entry(app).or_default();
        let since = list.fetched.map(|at| at.elapsed());
        if !list.needs_asking(since, again) {
            return;
        }
        list.loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.library() })
                .await;
            this.update(cx, |this, cx| {
                let list = this.lists.lists.entry(app).or_default();
                list.loading = false;
                list.fetched = Some(Instant::now());
                match result {
                    Ok(items) => {
                        list.items = Some(Arc::new(items));
                        list.error = None;
                    }
                    Err(e) => {
                        eprintln!("[{}] library: {e:#}", app.kind.name());
                        list.error =
                            Some(format!("Couldn't ask {}: {e:#}", app.kind.name()).into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The card's picture, if it's here; asks for it if not.
    fn card_picture(&mut self, app: Id, item: &Tracked, cx: &mut Context<Self>) -> Option<PathBuf> {
        let (art, _) = item.art.as_ref()?;
        self.art(app, item, *art, cx)
    }

    /// A picture of it, if it's here; asks for it if not. The backdrop
    /// (full size) is only for those with fanart.
    pub(super) fn art(
        &mut self,
        app: Id,
        item: &Tracked,
        art: Art,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let (has, url) = item.art.as_ref()?;
        if art == Art::Backdrop && *has != Art::Fanart {
            return None;
        }
        let name = picture_name(app, item.id, url, art);
        let path = self.profile.paths.arr_pictures().join(name);
        match self.lists.pictures.get(&path) {
            Some(Picture::Ready) => return Some(path),
            Some(_) => return None,
            None => {}
        }
        if path.exists() {
            self.lists.pictures.insert(path.clone(), Picture::Ready);
            return Some(path);
        }
        self.lists.pictures.insert(path.clone(), Picture::Loading);
        let id = item.id;
        self.lists.queue.push_front(Wanted { app, id, art, path });
        self.fetch_pictures(cx);
        None
    }

    fn fetch_pictures(&mut self, cx: &mut Context<Self>) {
        while self.lists.fetching < FETCHING {
            let Some(Wanted { app, id, art, path }) = self.lists.queue.pop_front() else {
                return;
            };
            let Some(client) = self.client(app) else {
                continue;
            };
            self.lists.fetching += 1;
            cx.spawn(async move |this, cx| {
                let out = path.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { client.fetch_art(id, art, &out) })
                    .await;
                this.update(cx, |this, cx| {
                    this.lists.fetching -= 1;
                    let picture = match result {
                        Ok(()) => Picture::Ready,
                        Err(e) => {
                            eprintln!("[{}] picture {id}: {e:#}", app.kind.name());
                            Picture::Failed
                        }
                    };
                    this.lists.pictures.insert(path, picture);
                    this.fetch_pictures(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The list as shown: everything, or just what's missing.
    fn shown(&self, app: Id) -> Vec<Tracked> {
        let Some(items) = self.lists.list(app) else {
            return Vec::new();
        };
        shown(&items, self.lists.filter)
    }

    fn card(&mut self, app: Id, item: &Tracked, cx: &mut Context<Self>) -> AnyElement {
        let Id { server, kind } = app;
        let thumb = self.card_picture(app, item, cx);
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
        };
        let subject = subject(kind, &item.title, item.year);
        // In the Jellyfin library: its page there. If not (yet), its page
        // from what Sonarr or Radarr knows, to get it from.
        let target = self.library_target(server, &subject, cx);
        let id = item.id;
        let clicks = Clicks::new(move |this: &mut Self, _, cx| match &target {
            Some(target) => this.open_target(target, cx),
            None => this.open_tracked(app, id, cx),
        });
        self.cards.card(card, clicks, cx)
    }

    /// The app's page: its bar over the list, the downloads, or what its
    /// name field found. `content_scroll` scrolls a page of sections.
    pub(in crate::shell) fn list_page(
        &self,
        app: Id,
        content_scroll: &ScrollHandle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.name_field(app.kind).read(cx).text().trim().to_string();
        let body = if !query.is_empty() {
            sections(self.add_results(app, &query, cx), content_scroll)
        } else if self.lists.tab == Tab::Activity {
            let activity = Activity::of(&self.downloads, cx);
            sections(
                activity.app_sections(app, cx, Self::open_target),
                content_scroll,
            )
        } else {
            self.grid(app, window, cx)
        };
        let bar = self.bar(app, cx);
        div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(body)
            // With frosted bars, drawn after the frost, like the header.
            .child(if header::FROSTED {
                gpui::deferred(bar).into_any_element()
            } else {
                bar
            })
            .into_any_element()
    }

    fn add_results(&self, app: Id, query: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let rows = if self.finder_shows(app, query) {
            self.finder_rows(app, cx)
        } else {
            vec![
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Keep typing: at least three letters.")
                    .into_any_element(),
            ]
        };
        vec![widgets::section(
            &format!("Add a {} to {}", noun(app.kind), app.kind.name()),
            div().flex().flex_col().gap_2().children(rows),
        )]
    }

    /// Every card, built only for the rows on screen.
    fn grid(&self, app: Id, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let list = self.lists.lists.get(&app);
        let message = |text: String, color: u32| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(color))
                .child(text)
        };
        let Some(items) = list.and_then(|l| l.items.as_ref()) else {
            return match list.and_then(|l| l.error.clone()) {
                Some(error) => self
                    .motion
                    .arriving("arr-error", message(error.to_string(), ERROR)),
                None => self.motion.waiting(
                    "arr-asking",
                    message(format!("Asking {}…", app.kind.name()), MUTED),
                ),
            };
        };
        let missing_only = self.lists.filter == Filter::Missing;
        let count = if missing_only {
            items.iter().filter(|t| t.missing()).count()
        } else {
            items.len()
        };
        if count == 0 {
            return self.motion.arriving(
                "arr-empty",
                message(
                    if missing_only {
                        "Nothing missing.".into()
                    } else {
                        format!("{} has no {}s yet.", app.kind.name(), noun(app.kind))
                    },
                    MUTED,
                ),
            );
        }
        let columns = columns(window);
        let rows = count.div_ceil(columns);
        // The page being left keeps the scroll it had.
        let scroll = if self.motion.is_leaving() {
            self.lists.leaving_scroll.clone()
        } else {
            self.lists.scroll.clone()
        };
        let grid = uniform_list(
            ElementId::Name(format!("arr-grid-{}-{}", app.server, app.kind.name()).into()),
            rows,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                let shown = this.shown(app);
                let (start, end) = (
                    range.start * columns,
                    (range.end * columns).min(shown.len()),
                );
                let cards: Vec<AnyElement> = shown[start.min(end)..end]
                    .iter()
                    .map(|item| this.card(app, item, cx))
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
        .track_scroll(scroll)
        .flex_1()
        .min_h(px(0.0))
        .pt(px(HEADER_HEIGHT + SORT_BAR_HEIGHT));
        self.motion.arriving("arr-grid", grid)
    }

    /// The bar under the header: which app, Library or Activity, and the
    /// field for adding to it.
    fn bar(&self, app: Id, cx: &mut Context<Self>) -> AnyElement {
        let Id { server, kind } = app;
        let items = self.lists.list(app);
        let total = items.as_ref().map(|i| i.len());
        let missing = items
            .as_ref()
            .map(|i| i.iter().filter(|t| t.missing()).count());
        let adding = !self.name_field(kind).read(cx).text().trim().is_empty();
        let showing = match (adding, self.lists.tab) {
            (true, _) => None,
            (false, Tab::Library) => Some(View::Library(self.lists.filter)),
            (false, Tab::Activity) => Some(View::Activity),
        };
        let chip = |id, label, view, count| Chip {
            id,
            label,
            kind,
            view,
            on: showing == Some(view),
            count,
        };
        let everything = View::Library(Filter::Everything);
        let library = self.chip(chip("library", "Library", everything, total), cx);
        let only_missing = View::Library(Filter::Missing);
        let missing_chip = self.chip(chip("missing", "Missing", only_missing, missing), cx);
        let active = self.downloads.read(cx).active_for(app);
        let activity = chip("activity", "Activity", View::Activity, Some(active));
        let activity = self.chip(activity, cx);
        div()
            .id("arr-bar")
            .absolute()
            .top(px(HEADER_HEIGHT))
            .left_0()
            .right_0()
            .h(px(SORT_BAR_HEIGHT))
            .bg(header::bar_background(0.0, header::BAR_JOIN_OPACITY, 0.0))
            .when(header::FROSTED, |bar| {
                bar.border_b_1().border_color(rgba(header::FROST_EDGE))
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
                    .when(self.catalog.servers().len() > 1, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .child(self.server_name(server)),
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
                    .child(self.name_field(kind).clone()),
            )
            .into_any_element()
    }

    fn chip(&self, chip: Chip, cx: &mut Context<Self>) -> AnyElement {
        let Chip {
            id,
            label,
            kind,
            view,
            on,
            count,
        } = chip;
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
                this.clear_name(kind, cx);
                this.pick(view);
                cx.notify();
            }))
            .into_any_element()
    }

    /// Shows the page's Library (all of it or what's missing) or its
    /// Activity, from the top.
    fn pick(&mut self, view: View) {
        self.lists.scroll = UniformListScrollHandle::new();
        match view {
            View::Library(filter) => {
                self.lists.tab = Tab::Library;
                self.lists.filter = filter;
            }
            View::Activity => self.lists.tab = Tab::Activity,
        }
    }
}

/// A choice on the page's bar: what it shows, and how many it has.
struct Chip {
    id: &'static str,
    label: &'static str,
    /// The app whose page this is.
    kind: crate::arr::Kind,
    view: View,
    /// It's what the page shows now.
    on: bool,
    count: Option<usize>,
}

/// Sections, scrolling as one page under the bar.
fn sections(sections: Vec<AnyElement>, content_scroll: &ScrollHandle) -> AnyElement {
    let mut page = div()
        .id("arr-content")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .track_scroll(content_scroll)
        .px_8()
        .pt(px(HEADER_HEIGHT + SORT_BAR_HEIGHT - 16.0))
        .pb_10()
        .children(sections);
    page.style().restrict_scroll_to_axis = Some(true);
    page.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::{Have, Kind};

    fn movie(id: i64, file: bool) -> Tracked {
        Tracked {
            id,
            title: format!("Movie {id}"),
            sort_title: format!("movie {id}"),
            year: None,
            monitored: true,
            have: Have::Movie {
                file,
                released: true,
                in_cinemas: true,
            },
            art: None,
            slug: None,
            overview: None,
            genres: Vec::new(),
            runtime_minutes: None,
            state: None,
        }
    }

    fn ids(items: &[Tracked]) -> Vec<i64> {
        items.iter().map(|item| item.id).collect()
    }

    #[test]
    fn the_library_tab_shows_everything_or_only_what_is_missing() {
        let items = [movie(1, true), movie(2, false), movie(3, false)];
        assert_eq!(ids(&shown(&items, Filter::Everything)), [1, 2, 3]);
        assert_eq!(ids(&shown(&items, Filter::Missing)), [2, 3]);
        assert!(shown(&[], Filter::Missing).is_empty());
    }

    #[test]
    fn a_list_is_asked_for_again_once_it_is_stale() {
        let list = List::default();
        assert!(list.needs_asking(None, false));
        assert!(!list.needs_asking(Some(Duration::from_secs(60)), false));
        assert!(list.needs_asking(Some(STALE), false));
        // Asked for anyway after something was added.
        assert!(list.needs_asking(Some(Duration::from_secs(1)), true));
    }

    #[test]
    fn a_list_being_fetched_is_not_asked_for_twice() {
        let list = List {
            loading: true,
            ..List::default()
        };
        assert!(!list.needs_asking(None, true));
    }

    #[test]
    fn a_picture_s_file_is_named_by_app_server_item_and_address() {
        let app = Id {
            server: 2,
            kind: Kind::Sonarr,
        };
        let url = "/MediaCover/115/fanart.jpg?lastWrite=1";
        let name = picture_name(app, 115, url, Art::Fanart);
        assert_eq!(name, format!("sonarr-2-115-{:016x}.jpg", fnv(url)));
        let full = picture_name(app, 115, url, Art::Backdrop);
        assert_eq!(full, format!("sonarr-2-115-{:016x}-full.jpg", fnv(url)));
    }
}
