//! Browsing the library: the Home rows, the Movies and TV Shows grids
//! (virtualized, so large libraries stay fast), sorting, and search.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    AnyElement, ClickEvent, Context, ElementId, FontWeight, SharedString, Window, div, prelude::*,
    px, rgb, rgba, uniform_list,
};

use super::{ACCENT, CARD_WIDTH, Card, MUTED, Screen, Shell, TEXT};
use crate::library::{self, Item, Library};

/// Horizontal padding of the content area and the gap between cards.
const PAGE_PADDING: f32 = 32.0;
const CARD_GAP: f32 = 20.0;
/// Cards in each Home row.
const ROW_LIMIT: usize = 20;
/// Episode matches shown in search results.
const SEARCH_EPISODE_LIMIT: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Title,
    RecentlyAdded,
    RecentlyWatched,
    Year,
}

impl Sort {
    fn label(self) -> &'static str {
        match self {
            Sort::Title => "Title",
            Sort::RecentlyAdded => "Recently added",
            Sort::RecentlyWatched => "Recently watched",
            Sort::Year => "Year",
        }
    }
}

pub const MOVIE_SORTS: [Sort; 4] = [
    Sort::Title,
    Sort::RecentlyAdded,
    Sort::RecentlyWatched,
    Sort::Year,
];
pub const SHOW_SORTS: [Sort; 3] = [Sort::Title, Sort::RecentlyAdded, Sort::RecentlyWatched];

/// A TV show, summarised from its episodes.
#[derive(Debug, Clone, PartialEq)]
pub struct ShowSummary {
    pub name: String,
    pub thumb: Option<PathBuf>,
    pub episodes: usize,
    pub unwatched: usize,
    /// Newest episode's added date.
    pub added: Option<i64>,
    pub last_played: Option<i64>,
}

impl ShowSummary {
    fn subtitle(&self) -> String {
        match self.unwatched {
            0 => format!("{} episodes · all watched", self.episodes),
            n if n == self.episodes => format!("{n} episodes"),
            n => format!("{} episodes · {n} unwatched", self.episodes),
        }
    }
}

pub fn show_summaries<'a>(items: impl Iterator<Item = &'a Item>) -> Vec<ShowSummary> {
    let mut shows: BTreeMap<&str, ShowSummary> = BTreeMap::new();
    for item in items {
        let Some(name) = item.show.as_deref() else {
            continue;
        };
        let show = shows.entry(name).or_insert_with(|| ShowSummary {
            name: name.to_string(),
            thumb: None,
            episodes: 0,
            unwatched: 0,
            added: None,
            last_played: None,
        });
        show.episodes += 1;
        show.unwatched += usize::from(!item.watched);
        show.thumb = show.thumb.take().or_else(|| item.thumb.clone());
        show.added = show.added.max(item.added);
        show.last_played = show.last_played.max(item.last_played);
    }
    shows.into_values().collect()
}

/// Newest/most recent first for dates; items without one go last.
fn newest_first(value: Option<i64>) -> (bool, Reverse<i64>) {
    (value.is_none(), Reverse(value.unwrap_or(0)))
}

pub fn sort_movies(movies: &mut [&Item], sort: Sort) {
    movies.sort_by_key(|m| m.title.to_lowercase());
    match sort {
        Sort::Title => {}
        Sort::RecentlyAdded => movies.sort_by_key(|m| newest_first(m.added)),
        Sort::RecentlyWatched => movies.sort_by_key(|m| newest_first(m.last_played)),
        Sort::Year => movies.sort_by_key(|m| newest_first(m.year.map(i64::from))),
    }
}

pub fn sort_shows(shows: &mut [ShowSummary], sort: Sort) {
    shows.sort_by_key(|s| s.name.to_lowercase());
    match sort {
        Sort::Title | Sort::Year => {}
        Sort::RecentlyAdded => shows.sort_by_key(|s| newest_first(s.added)),
        Sort::RecentlyWatched => shows.sort_by_key(|s| newest_first(s.last_played)),
    }
}

/// Every word of `query` appears in `text` (case-insensitive).
pub fn matches(text: &str, query: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// How many cards fit across the content area (beside the sidebar).
pub(super) fn columns(window: &Window) -> usize {
    let width = f32::from(window.viewport_size().width) - SIDEBAR_WIDTH - 2.0 * PAGE_PADDING;
    (((width + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1)
}

/// Width of the sidebar of sources and libraries.
pub(super) const SIDEBAR_WIDTH: f32 = 230.0;

/// Where an item comes from: a Jellyfin server (our row id), or None for
/// local folders.
pub type Origin = Option<i64>;

pub fn origin(item: &Item) -> Origin {
    match item.source {
        crate::library::Source::Jellyfin { server, .. } => Some(server),
        crate::library::Source::Local { .. } => None,
    }
}

/// One library of one source: a Jellyfin library by name, or None for
/// items in no library (local folders, or a server whose libraries
/// couldn't be listed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    pub origin: Origin,
    pub library: Option<String>,
}

/// A grid: the movies, or the shows, of one shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    pub shows: bool,
    pub shelf: Shelf,
}

impl Grid {
    pub fn of(screen: &Screen) -> Option<Grid> {
        match screen {
            Screen::Movies(shelf) => Some(Grid {
                shows: false,
                shelf: shelf.clone(),
            }),
            Screen::Shows(shelf) => Some(Grid {
                shows: true,
                shelf: shelf.clone(),
            }),
            _ => None,
        }
    }

    fn contains(&self, item: &Item) -> bool {
        item.is_episode() == self.shows
            && origin(item) == self.shelf.origin
            && item.library.as_ref().map(|l| l.name.as_str()) == self.shelf.library.as_deref()
    }

    fn id(&self) -> SharedString {
        format!(
            "{}-grid-{}-{}",
            if self.shows { "shows" } else { "movies" },
            self.shelf
                .origin
                .map_or("local".to_string(), |s| s.to_string()),
            self.shelf.library.as_deref().unwrap_or("")
        )
        .into()
    }
}

/// A source in the sidebar: its name, its page, and its libraries.
pub struct SourceEntry {
    pub origin: Origin,
    pub name: String,
    pub libraries: Vec<(String, Screen)>,
}

impl Shell {
    /// Movies in `grid`, in the chosen order.
    pub(super) fn movies(&self, grid: &Grid) -> Vec<&Item> {
        let mut movies: Vec<&Item> = self.items().filter(|i| grid.contains(i)).collect();
        sort_movies(&mut movies, self.movie_sort);
        movies
    }

    /// Shows in `grid`, in the chosen order.
    pub(super) fn shows(&self, grid: &Grid) -> Vec<ShowSummary> {
        let mut shows = show_summaries(self.items().filter(|i| grid.contains(i)));
        sort_shows(&mut shows, self.show_sort);
        shows
    }

    /// The sidebar's sources: each signed-in Jellyfin server with its
    /// libraries in the server's order, then local folders (Movies, TV
    /// Shows) if there are any.
    pub(super) fn sources(&self) -> Vec<SourceEntry> {
        struct Found<'a> {
            libraries: Vec<&'a Library>,
            /// Items in no library: movies, episodes.
            loose: (bool, bool),
        }
        let mut found: BTreeMap<Origin, Found> = BTreeMap::new();
        for item in self.items() {
            let entry = found.entry(origin(item)).or_insert(Found {
                libraries: Vec::new(),
                loose: (false, false),
            });
            match &item.library {
                Some(l) if !entry.libraries.iter().any(|known| known.name == l.name) => {
                    entry.libraries.push(l)
                }
                Some(_) => {}
                None if item.is_episode() => entry.loose.1 = true,
                None => entry.loose.0 = true,
            }
        }
        let entry = |origin: Origin, name: String, found: Option<&Found>| {
            let mut libraries: Vec<(String, Screen)> = Vec::new();
            if let Some(found) = found {
                let mut named = found.libraries.clone();
                named.sort_by_key(|l| l.order);
                for l in named {
                    let shelf = Shelf {
                        origin,
                        library: Some(l.name.clone()),
                    };
                    let screen = if l.shows {
                        Screen::Shows(shelf)
                    } else {
                        Screen::Movies(shelf)
                    };
                    libraries.push((l.name.clone(), screen));
                }
                let shelf = Shelf {
                    origin,
                    library: None,
                };
                if found.loose.0 {
                    libraries.push(("Movies".into(), Screen::Movies(shelf.clone())));
                }
                if found.loose.1 {
                    libraries.push(("TV Shows".into(), Screen::Shows(shelf)));
                }
            }
            SourceEntry {
                origin,
                name,
                libraries,
            }
        };
        let mut sources: Vec<SourceEntry> = self
            .servers
            .iter()
            .map(|server| {
                // One server signed in as two users needs telling apart.
                let twin = self
                    .servers
                    .iter()
                    .any(|s| s.id != server.id && s.name == server.name);
                let name = if twin {
                    format!("{} ({})", server.name, server.user_name)
                } else {
                    server.name.clone()
                };
                entry(Some(server.id), name, found.get(&Some(server.id)))
            })
            .collect();
        if found.contains_key(&None) || !self.roots.is_empty() {
            sources.push(entry(None, "Local folders".into(), found.get(&None)));
        }
        sources
    }

    /// Every library screen in the sidebar.
    pub(super) fn library_screens(&self) -> Vec<Screen> {
        self.sources()
            .into_iter()
            .flat_map(|s| s.libraries.into_iter().map(|(_, screen)| screen))
            .collect()
    }

    /// A source's name in the sidebar.
    pub(super) fn source_name(&self, origin: Origin) -> String {
        match origin {
            Some(id) => self
                .servers
                .iter()
                .find(|s| s.id == id)
                .map_or_else(|| "Jellyfin".into(), |s| s.name.clone()),
            None => "Local folders".into(),
        }
    }

    pub(super) fn search_query(&self, cx: &Context<Self>) -> String {
        self.search.read(cx).text().trim().to_string()
    }

    pub(super) fn open_show(&mut self, name: String, cx: &mut Context<Self>) {
        self.focus_episode = None;
        // The show's overview and backdrop come from the server, via any of
        // its episodes there.
        let episode = self
            .items()
            .find(|i| {
                i.show.as_deref() == Some(name.as_str())
                    && matches!(i.source, crate::library::Source::Jellyfin { .. })
            })
            .cloned();
        if let Some(episode) = episode {
            self.load_details(format!("show:{name}"), episode, true, cx);
        }
        // Only the season being watched starts open; the others are a
        // click away instead of a long scroll.
        let current = self.next_episode(&name).map(|e| e.season.unwrap_or(0));
        self.open_seasons
            .insert(name.clone(), current.into_iter().collect());
        self.screen = Screen::Show(name.clone());
        // Fresh each time the page opens (once its TVDB id is known).
        self.load_sonarr(name, cx);
        cx.notify();
    }

    pub(super) fn show_card(
        &self,
        prefix: &str,
        show: &ShowSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = show.name.clone();
        let card = Card {
            id: super::show_card_id(prefix, &show.name),
            thumb: show.thumb.clone(),
            title: show.name.clone(),
            subtitle: Some(show.subtitle()),
            progress: None,
            watched: show.unwatched == 0,
            highlight: false,
        };
        self.card(card, None, cx, move |this, _, cx| {
            this.open_show(name.clone(), cx)
        })
    }

    /// A titled, sideways-scrolling row, optionally with "See all".
    fn row(
        &self,
        id: impl Into<SharedString>,
        title: &str,
        cards: Vec<AnyElement>,
        see_all: Option<Screen>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id: SharedString = id.into();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .pt_6()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_4()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title.to_string()),
                    )
                    .children(see_all.map(|screen| {
                        div()
                            .id(ElementId::Name(format!("{id}-all").into()))
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(TEXT)))
                            .child("See all ›")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.set_tab(screen.clone(), cx)
                            }))
                    })),
            )
            .child({
                let scroll = self
                    .row_scroll
                    .borrow_mut()
                    .entry(id.clone())
                    .or_default()
                    .clone();
                let mut row = div()
                    .id(id)
                    .flex()
                    .gap_5()
                    .pb_2()
                    .overflow_x_scroll()
                    .track_scroll(&scroll)
                    .children(cards);
                // The mouse wheel scrolls the page, not the row under the
                // pointer; rows move with shift+wheel, a trackpad or the arrows.
                row.style().restrict_scroll_to_axis = Some(true);
                row
            })
            .into_any_element()
    }

    pub(super) fn set_tab(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if screen == Screen::Downloads {
            self.refresh_downloads(cx);
        }
        self.tab = screen.clone();
        self.screen = screen;
        cx.notify();
    }

    /// Home (`scope` None: every source) or one source's page: what to
    /// continue, then the newest in each library.
    pub(super) fn home(&self, scope: Option<Origin>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let in_scope = |item: &&Item| scope.is_none_or(|o| origin(item) == o);
        let mut rows = Vec::new();
        if let Some(origin) = scope {
            rows.push(
                div()
                    .pt_6()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(self.source_name(origin))
                    .into_any_element(),
            );
        }
        if !self.items().any(|i| in_scope(&i)) {
            rows.push(if scope.is_none() {
                self.empty_state(cx)
            } else {
                div()
                    .pt_10()
                    .text_color(rgb(MUTED))
                    .child(if self.loading_servers.is_empty() {
                        "Nothing here yet."
                    } else {
                        "Loading…"
                    })
                    .into_any_element()
            });
            return rows;
        }
        let prefix = match scope {
            None => "home".to_string(),
            Some(o) => format!("source-{}", o.map_or("local".into(), |s| s.to_string())),
        };

        // Half-watched videos, and the next episode of shows being watched,
        // most recently watched first.
        let mut continuing: Vec<(Option<i64>, &Item)> = self
            .items()
            .filter(in_scope)
            .filter(|i| i.progress().is_some())
            .map(|i| (i.last_played, i))
            .collect();
        continuing.extend(library::next_up(self.items().filter(in_scope)));
        continuing.sort_by_key(|(played, _)| Reverse(*played));
        if !continuing.is_empty() {
            let cards = continuing
                .into_iter()
                .take(super::CONTINUE_LIMIT)
                .map(|(_, i)| self.item_card(&format!("{prefix}-continue"), i, true, cx))
                .collect();
            rows.push(self.row(
                format!("{prefix}-continue-row"),
                "Continue Watching",
                cards,
                None,
                cx,
            ));
        }

        // The newest in each library, like Jellyfin's "Latest ..." rows.
        let sources: Vec<SourceEntry> = self
            .sources()
            .into_iter()
            .filter(|s| scope.is_none_or(|o| s.origin == o))
            .collect();
        // On Home with several sources, rows say which one they're from.
        let several = scope.is_none() && sources.len() > 1;
        let libraries = sources.iter().flat_map(|source| {
            source
                .libraries
                .iter()
                .map(move |(label, screen)| (source, label, screen.clone()))
        });
        for (source, label, screen) in libraries {
            let Some(grid) = Grid::of(&screen) else {
                continue;
            };
            let mut title = match (&grid.shelf.library, grid.shows) {
                (Some(_), _) => format!("Latest {label}"),
                (None, false) => format!("Recently Added {label}"),
                (None, true) => format!("Recently Updated {label}"),
            };
            if several {
                title = format!("{title} · {}", source.name);
            }
            let id = format!("{prefix}-{}", grid.id());
            let cards: Vec<AnyElement> = if grid.shows {
                let mut shows = show_summaries(self.items().filter(|i| grid.contains(i)));
                sort_shows(&mut shows, Sort::RecentlyAdded);
                shows
                    .iter()
                    .take(ROW_LIMIT)
                    .map(|s| self.show_card(&format!("recent-{id}"), s, cx))
                    .collect()
            } else {
                let mut movies: Vec<&Item> = self.items().filter(|i| grid.contains(i)).collect();
                sort_movies(&mut movies, Sort::RecentlyAdded);
                movies
                    .into_iter()
                    .take(ROW_LIMIT)
                    .map(|i| self.item_card(&format!("recent-{id}"), i, true, cx))
                    .collect()
            };
            if !cards.is_empty() {
                rows.push(self.row(id, &title, cards, Some(screen), cx));
            }
        }
        rows
    }

    fn sort_bar(&self, grid: &Grid, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let shows = grid.shows;
        let (current, options, noun) = if shows {
            (self.show_sort, &SHOW_SORTS[..], "shows")
        } else {
            (self.movie_sort, &MOVIE_SORTS[..], "movies")
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_8()
            .pt_2()
            .pb_4()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .mr_2()
                    .child(format!("{count} {noun}")),
            )
            .children(options.iter().map(|&sort| {
                div()
                    .id(ElementId::Name(format!("sort-{}", sort.label()).into()))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .text_sm()
                    .cursor_pointer()
                    .when(sort == current, |d| d.bg(rgba(0xffffff26)))
                    .hover(|d| d.bg(rgba(0xffffff1f)))
                    .child(sort.label())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if shows {
                            this.show_sort = sort;
                        } else {
                            this.movie_sort = sort;
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// A library tab: a sort bar over a grid that only builds the rows on
    /// screen.
    pub(super) fn library_grid(
        &self,
        grid: Grid,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = if grid.shows {
            show_summaries(self.items().filter(|i| grid.contains(i))).len()
        } else {
            self.items().filter(|i| grid.contains(i)).count()
        };
        if count == 0 {
            let message = if grid.shows {
                "No TV shows yet."
            } else {
                "No movies yet."
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child(message)
                .into_any_element();
        }
        let columns = columns(window);
        let rows = count.div_ceil(columns);
        let bar = self.sort_bar(&grid, count, cx);
        let list = uniform_list(
            grid.id(),
            rows,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                // Sorted once per visible batch; cards only for these rows.
                let (start, end) = (range.start * columns, (range.end * columns).min(count));
                let cards: Vec<AnyElement> = if grid.shows {
                    let shows = this.shows(&grid);
                    shows[start.min(shows.len())..end.min(shows.len())]
                        .iter()
                        .map(|s| this.show_card("all", s, cx))
                        .collect()
                } else {
                    let movies = this.movies(&grid);
                    movies[start.min(movies.len())..end.min(movies.len())]
                        .iter()
                        .map(|m| this.item_card("movie", m, true, cx))
                        .collect()
                };
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
        .track_scroll(self.grid_scroll.clone())
        .flex_1()
        .min_h(px(0.0));
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(bar)
            .child(list)
            .into_any_element()
    }

    pub(super) fn search_results(&self, query: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // Across every library.
        let mut movies: Vec<&Item> = self
            .items()
            .filter(|m| {
                !m.is_episode()
                    && (matches(&m.title, query)
                        || m.year.is_some_and(|y| query.trim() == y.to_string()))
            })
            .collect();
        sort_movies(&mut movies, self.movie_sort);
        let mut shows: Vec<ShowSummary> = show_summaries(self.items())
            .into_iter()
            .filter(|s| matches(&s.name, query))
            .collect();
        sort_shows(&mut shows, self.show_sort);
        let mut episodes: Vec<&Item> = self
            .items()
            .filter(|i| i.is_episode() && matches(&i.title, query))
            .collect();
        episodes.sort_by_key(|e| (e.show.clone(), e.watch_order()));

        let mut sections = Vec::new();
        if !movies.is_empty() {
            let cards = movies
                .iter()
                .map(|m| self.item_card("found-movie", m, true, cx))
                .collect();
            sections.push(Self::section("Movies", Self::grid(cards)));
        }
        if !shows.is_empty() {
            let cards = shows
                .iter()
                .map(|s| self.show_card("found", s, cx))
                .collect();
            sections.push(Self::section("TV Shows", Self::grid(cards)));
        }
        if !episodes.is_empty() {
            let cards = episodes
                .iter()
                .take(SEARCH_EPISODE_LIMIT)
                .map(|e| self.item_card("found-episode", e, true, cx))
                .collect();
            sections.push(Self::section("Episodes", Self::grid(cards)));
        }
        if sections.is_empty() {
            sections.push(
                div()
                    .pt_10()
                    .text_color(rgb(MUTED))
                    .child(format!("Nothing matches “{query}”."))
                    .into_any_element(),
            );
        }
        sections
    }

    /// The search field, in the header over the library.
    pub(super) fn search_field(&self) -> AnyElement {
        div()
            .w(px(320.0))
            .flex_none()
            .child(self.search.clone())
            .into_any_element()
    }

    /// The left sidebar: Home, then each source with its libraries.
    /// What's open is highlighted (a show or movie page counts as the
    /// library it was opened from).
    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = &self.tab;
        let entry =
            |id: SharedString,
             label: String,
             screen: Screen,
             indent: bool,
             heading: bool,
             cx: &mut Context<Self>| {
                let active = *current == screen;
                div()
                    .id(id)
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .when(indent, |d| d.pl_6())
                    .rounded_md()
                    .truncate()
                    .cursor_pointer()
                    .when(heading, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .text_color(rgb(if active || heading { TEXT } else { MUTED }))
                    .when(active, |d| d.bg(rgba(0xffffff1a)))
                    .when(!active, |d| d.hover(|s| s.bg(rgba(0xffffff0d))))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_tab(screen.clone(), cx)
                    }))
                    .into_any_element()
            };
        let mut children = vec![entry(
            "side-home".into(),
            "Home".into(),
            Screen::Home,
            false,
            true,
            cx,
        )];
        if !self.arr_apps().is_empty() {
            let active = self.downloads.active();
            let screen = Screen::Downloads;
            let selected = *current == screen;
            children.push(
                div()
                    .id("side-downloads")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(selected, |d| d.bg(rgba(0xffffff1a)))
                    .when(!selected, |d| d.hover(|s| s.bg(rgba(0xffffff0d))))
                    .child(div().flex_1().min_w(px(0.0)).truncate().child("Downloads"))
                    .when(active > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .px_2()
                                .rounded_full()
                                .text_xs()
                                .bg(rgb(ACCENT))
                                .child(active.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_tab(screen.clone(), cx)
                    }))
                    .into_any_element(),
            );
        }
        for source in self.sources() {
            let key = source.origin.map_or("local".to_string(), |s| s.to_string());
            children.push(div().h(px(14.0)).flex_none().into_any_element());
            children.push(entry(
                format!("side-source-{key}").into(),
                source.name.clone(),
                Screen::Source(source.origin),
                false,
                true,
                cx,
            ));
            for (label, screen) in source.libraries {
                let id = match Grid::of(&screen) {
                    Some(grid) => format!("side-{}", grid.id()),
                    None => format!("side-{key}-{label}"),
                };
                children.push(entry(id.into(), label, screen, true, false, cx));
            }
        }
        let mut sidebar = div()
            .id("sidebar")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .gap_0p5()
            .pl_6()
            .pr_2()
            .pt_2()
            .pb_6()
            .overflow_y_scroll()
            .children(children);
        sidebar.style().restrict_scroll_to_axis = Some(true);
        sidebar.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Source;

    fn item(title: &str, show: Option<&str>) -> Item {
        Item {
            library: None,
            source: Source::Jellyfin {
                server: 1,
                id: title.into(),
            },
            title: title.into(),
            year: None,
            show: show.map(String::from),
            season: show.map(|_| 1),
            episode: show.map(|_| 1),
            duration: None,
            position: 0.0,
            watched: false,
            last_played: None,
            thumb: None,
            added: None,
        }
    }

    #[test]
    fn grids_hold_one_library() {
        let library = |name: &str, shows| {
            Some(std::sync::Arc::new(Library {
                name: name.into(),
                order: 0,
                shows,
            }))
        };
        let mut movie = item("Movie", None);
        movie.library = library("Movies", false);
        let mut anime = item("Anime", None);
        anime.library = library("Anime Movies", false);
        let mut episode = item("Ep", Some("Show"));
        episode.library = library("Shows", true);
        let mut local = item("Home Video", None);
        local.source = Source::Local {
            id: 1,
            path: "D:/Home Video.mkv".into(),
        };
        // Another server's library of the same name.
        let mut other = item("Other Movie", None);
        other.library = library("Movies", false);
        other.source = Source::Jellyfin {
            server: 2,
            id: "x".into(),
        };
        let shelf = |origin, library: Option<&str>| Shelf {
            origin,
            library: library.map(String::from),
        };

        let grid = Grid::of(&Screen::Movies(shelf(Some(1), Some("Movies")))).unwrap();
        assert!(grid.contains(&movie));
        assert!(!grid.contains(&anime) && !grid.contains(&episode) && !grid.contains(&local));
        assert!(!grid.contains(&other));
        let local_grid = Grid::of(&Screen::Movies(shelf(None, None))).unwrap();
        assert!(local_grid.contains(&local) && !local_grid.contains(&movie));
        let shows = Grid::of(&Screen::Shows(shelf(Some(1), Some("Shows")))).unwrap();
        assert!(shows.contains(&episode) && !shows.contains(&movie));
        assert_eq!(Grid::of(&Screen::Home), None);
    }

    #[test]
    fn search_matches_all_words() {
        assert!(matches("The Lord of the Rings", "lord rings"));
        assert!(matches("Amélie", "amélie"));
        assert!(!matches("The Lord of the Rings", "lord hobbit"));
        assert!(matches("Anything", ""));
    }

    #[test]
    fn sorting_movies() {
        let mut a = item("Alpha", None);
        let mut b = item("beta", None);
        let c = item("Gamma", None);
        a.added = Some(10);
        b.added = Some(20);
        b.last_played = Some(5);
        a.year = Some(1999);
        b.year = Some(2010);
        let titles = |list: &[&Item]| list.iter().map(|i| i.title.clone()).collect::<Vec<_>>();

        let mut list = vec![&c, &b, &a];
        sort_movies(&mut list, Sort::Title);
        assert_eq!(titles(&list), ["Alpha", "beta", "Gamma"]);
        sort_movies(&mut list, Sort::RecentlyAdded);
        assert_eq!(titles(&list), ["beta", "Alpha", "Gamma"]);
        sort_movies(&mut list, Sort::RecentlyWatched);
        assert_eq!(titles(&list), ["beta", "Alpha", "Gamma"]);
        sort_movies(&mut list, Sort::Year);
        assert_eq!(titles(&list), ["beta", "Alpha", "Gamma"]);
    }

    #[test]
    fn shows_are_summarised_from_episodes() {
        let mut e1 = item("One", Some("Show"));
        let mut e2 = item("Two", Some("Show"));
        let e3 = item("Pilot", Some("Other"));
        e1.watched = true;
        e1.added = Some(100);
        e2.added = Some(300);
        e2.last_played = Some(50);
        let movie = item("Film", None);
        let mut shows = show_summaries([&e1, &e2, &e3, &movie].into_iter());
        assert_eq!(shows.len(), 2);
        let show = shows.iter().find(|s| s.name == "Show").unwrap();
        assert_eq!((show.episodes, show.unwatched), (2, 1));
        assert_eq!((show.added, show.last_played), (Some(300), Some(50)));
        assert_eq!(show.subtitle(), "2 episodes · 1 unwatched");
        sort_shows(&mut shows, Sort::RecentlyAdded);
        assert_eq!(shows[0].name, "Show");
        sort_shows(&mut shows, Sort::Title);
        assert_eq!(shows[0].name, "Other");
    }
}
