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

use super::{CARD_WIDTH, Card, MUTED, Screen, Shell, TEXT};
use crate::library::{self, Item};

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

/// How many cards fit across the content area.
fn columns(window: &Window) -> usize {
    let width = f32::from(window.viewport_size().width) - 2.0 * PAGE_PADDING;
    (((width + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grid {
    Movies,
    Shows,
}

impl Shell {
    pub(super) fn movies(&self) -> Vec<&Item> {
        let mut movies: Vec<&Item> = self.items().filter(|i| !i.is_episode()).collect();
        sort_movies(&mut movies, self.movie_sort);
        movies
    }

    pub(super) fn shows(&self) -> Vec<ShowSummary> {
        let mut shows = show_summaries(self.items());
        sort_shows(&mut shows, self.show_sort);
        shows
    }

    pub(super) fn search_query(&self, cx: &Context<Self>) -> String {
        self.search.read(cx).text().trim().to_string()
    }

    fn open_show(&mut self, name: String, cx: &mut Context<Self>) {
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
        self.screen = Screen::Show(name);
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
            id: SharedString::from(format!("{prefix}-show-{}", show.name)).into(),
            thumb: show.thumb.clone(),
            title: show.name.clone(),
            subtitle: Some(show.subtitle()),
            progress: None,
            watched: show.unwatched == 0,
        };
        self.card(card, cx, move |this, _, cx| {
            this.open_show(name.clone(), cx)
        })
    }

    /// A titled, sideways-scrolling row, optionally with "See all".
    fn row(
        &self,
        id: &'static str,
        title: &str,
        cards: Vec<AnyElement>,
        see_all: Option<Screen>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            .child(
                div()
                    .id(id)
                    .flex()
                    .gap_5()
                    .pb_2()
                    .overflow_x_scroll()
                    .children(cards),
            )
            .into_any_element()
    }

    pub(super) fn set_tab(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.tab = screen.clone();
        self.screen = screen;
        cx.notify();
    }

    pub(super) fn home(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.items().next().is_none() {
            return vec![self.empty_state(cx)];
        }
        let mut rows = Vec::new();

        // Half-watched videos, and the next episode of shows being watched,
        // most recently watched first.
        let mut continuing: Vec<(Option<i64>, &Item)> = self
            .items()
            .filter(|i| i.progress().is_some())
            .map(|i| (i.last_played, i))
            .collect();
        continuing.extend(library::next_up(self.items()));
        continuing.sort_by_key(|(played, _)| Reverse(*played));
        if !continuing.is_empty() {
            let cards = continuing
                .into_iter()
                .take(super::CONTINUE_LIMIT)
                .map(|(_, i)| self.item_card("continue", i, true, cx))
                .collect();
            rows.push(self.row("continue-row", "Continue Watching", cards, None, cx));
        }

        let mut movies: Vec<&Item> = self.items().filter(|i| !i.is_episode()).collect();
        if !movies.is_empty() {
            sort_movies(&mut movies, Sort::RecentlyAdded);
            let cards = movies
                .into_iter()
                .take(ROW_LIMIT)
                .map(|i| self.item_card("recent-movie", i, true, cx))
                .collect();
            rows.push(self.row(
                "recent-movies",
                "Recently Added Movies",
                cards,
                Some(Screen::Movies),
                cx,
            ));
        }

        let mut shows = show_summaries(self.items());
        if !shows.is_empty() {
            sort_shows(&mut shows, Sort::RecentlyAdded);
            let cards = shows
                .iter()
                .take(ROW_LIMIT)
                .map(|s| self.show_card("recent", s, cx))
                .collect();
            rows.push(self.row(
                "recent-shows",
                "Recently Updated Shows",
                cards,
                Some(Screen::Shows),
                cx,
            ));
        }
        rows
    }

    fn sort_bar(&self, grid: Grid, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let (current, options, noun) = match grid {
            Grid::Movies => (self.movie_sort, &MOVIE_SORTS[..], "movies"),
            Grid::Shows => (self.show_sort, &SHOW_SORTS[..], "shows"),
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
                        match grid {
                            Grid::Movies => this.movie_sort = sort,
                            Grid::Shows => this.show_sort = sort,
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// The Movies or TV Shows tab: a sort bar over a grid that only builds
    /// the rows on screen.
    pub(super) fn library_grid(
        &self,
        grid: Grid,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = match grid {
            Grid::Movies => self.items().filter(|i| !i.is_episode()).count(),
            Grid::Shows => show_summaries(self.items()).len(),
        };
        if count == 0 {
            let message = match grid {
                Grid::Movies => "No movies yet.",
                Grid::Shows => "No TV shows yet.",
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
        let list = uniform_list(
            match grid {
                Grid::Movies => "movies-grid",
                Grid::Shows => "shows-grid",
            },
            rows,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                // Sorted once per visible batch; cards only for these rows.
                let movies;
                let shows;
                let (start, end) = (range.start * columns, (range.end * columns).min(count));
                let cards: Vec<AnyElement> = match grid {
                    Grid::Movies => {
                        movies = this.movies();
                        movies[start.min(movies.len())..end.min(movies.len())]
                            .iter()
                            .map(|m| this.item_card("movie", m, true, cx))
                            .collect()
                    }
                    Grid::Shows => {
                        shows = this.shows();
                        shows[start.min(shows.len())..end.min(shows.len())]
                            .iter()
                            .map(|s| this.show_card("all", s, cx))
                            .collect()
                    }
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
        .flex_1()
        .min_h(px(0.0));
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.sort_bar(grid, count, cx))
            .child(list)
            .into_any_element()
    }

    pub(super) fn search_results(&self, query: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let movies: Vec<&Item> = self
            .movies()
            .into_iter()
            .filter(|m| {
                matches(&m.title, query) || m.year.is_some_and(|y| query.trim() == y.to_string())
            })
            .collect();
        let shows: Vec<ShowSummary> = self
            .shows()
            .into_iter()
            .filter(|s| matches(&s.name, query))
            .collect();
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

    /// Home / Movies / TV Shows, and the search field.
    pub(super) fn tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab =
            |id: &'static str, label: &'static str, screen: Screen, cx: &mut Context<Self>| {
                let active = self.screen == screen;
                div()
                    .id(id)
                    .px_4()
                    .py_1p5()
                    .rounded_full()
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .text_color(rgb(if active { TEXT } else { MUTED }))
                    .when(active, |d| d.bg(rgba(0xffffff1f)))
                    .hover(|d| d.text_color(rgb(TEXT)))
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_tab(screen.clone(), cx)
                    }))
                    .into_any_element()
            };
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(tab("tab-home", "Home", Screen::Home, cx))
            .child(tab("tab-movies", "Movies", Screen::Movies, cx))
            .child(tab("tab-shows", "TV Shows", Screen::Shows, cx))
            .child(div().w(px(260.0)).ml_4().child(self.search.clone()))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Source;

    fn item(title: &str, show: Option<&str>) -> Item {
        Item {
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
