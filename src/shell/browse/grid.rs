//! A library tab: every movie or show of one library in a grid that only
//! builds the rows on screen (so large libraries stay fast), under a bar
//! that picks the order.

use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, ElementId, SharedString, UniformListScrollHandle, Window, div,
    prelude::*, px, rgb, rgba, uniform_list,
};

use super::Pages;
use super::item_cards::{self, Titled};
use super::shelves::{self, Grid, MOVIE_SORTS, SHOW_SORTS, ShowSummary, Sort};
use crate::library::Item;
use crate::shell::MUTED;
use crate::shell::cards;
use crate::shell::header::{BAR_JOIN_OPACITY, FROST_EDGE, FROSTED, HEADER_HEIGHT, bar_background};

/// Horizontal padding of the content area and the gap between cards.
const PAGE_PADDING: f32 = 32.0;
const CARD_GAP: f32 = 20.0;
/// The sort choices over a library grid, see-through like the top bar.
pub(in crate::shell) const SORT_BAR_HEIGHT: f32 = 56.0;
/// Width of the sidebar of sources and libraries.
pub(in crate::shell) const SIDEBAR_WIDTH: f32 = 230.0;
/// What the ids of a grid's cards start with: its movies', its shows'.
const MOVIE_CARDS: &str = "movie";
const SHOW_CARDS: &str = "all";

/// A grid as laid out: how many cards it has, and how many go across.
#[derive(Debug, Clone, Copy)]
struct Layout {
    count: usize,
    columns: usize,
}

/// How many cards fit across the content area (beside the sidebar).
pub(in crate::shell) fn columns(window: &Window) -> usize {
    let width = f32::from(window.viewport_size().width) - SIDEBAR_WIDTH - 2.0 * PAGE_PADDING;
    (((width + CARD_GAP) / (cards::WIDTH + CARD_GAP)).floor() as usize).max(1)
}

impl Pages {
    /// Movies in `grid`, in the chosen order.
    fn movies(&self, grid: &Grid) -> Vec<&Item> {
        let mut movies: Vec<&Item> = self.items().filter(|i| grid.contains(i)).collect();
        shelves::sort_movies(&mut movies, self.sorts.movies);
        movies
    }

    /// Shows in `grid`, in the chosen order.
    fn shows(&self, grid: &Grid) -> Vec<ShowSummary> {
        let mut shows = shelves::show_summaries(self.items().filter(|i| grid.contains(i)));
        shelves::sort_shows(&mut shows, self.sorts.shows);
        shows
    }

    /// The ids of a grid's cards, in the order they're laid out.
    pub(in crate::shell) fn grid_card_ids(&self, grid: &Grid) -> Vec<SharedString> {
        if grid.shows {
            let shows = self.shows(grid);
            let ids = shows
                .iter()
                .map(|show| item_cards::show_card_id(SHOW_CARDS, &show.name));
            ids.collect()
        } else {
            let movies = self.movies(grid);
            let ids = movies
                .iter()
                .map(|movie| item_cards::item_card_id(MOVIE_CARDS, movie));
            ids.collect()
        }
    }

    /// Puts the grids of `grid`'s kind (movies, or shows) in another order.
    fn set_sort(&mut self, grid: &Grid, sort: Sort, cx: &mut Context<Self>) {
        if grid.shows {
            self.sorts.shows = sort;
        } else {
            self.sorts.movies = sort;
        }
        cx.notify();
    }

    fn sort_bar(&self, grid: &Grid, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let (current, options, noun) = if grid.shows {
            (self.sorts.shows, &SHOW_SORTS[..], "shows")
        } else {
            (self.sorts.movies, &MOVIE_SORTS[..], "movies")
        };
        div()
            .id("sort-bar")
            .absolute()
            .top(px(HEADER_HEIGHT))
            .left_0()
            .right_0()
            .h(px(SORT_BAR_HEIGHT))
            .bg(bar_background(0.0, BAR_JOIN_OPACITY, 0.0))
            .when(FROSTED, |bar| {
                bar.border_b_1().border_color(rgba(FROST_EDGE))
            })
            .occlude()
            .flex()
            .items_center()
            .gap_2()
            .px_8()
            .pb_2()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .mr_2()
                    .child(format!("{count} {noun}")),
            )
            .children(options.iter().map(|&sort| {
                let grid = grid.clone();
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
                        this.set_sort(&grid, sort, cx)
                    }))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// The cards of rows `rows` of a grid, a row each.
    fn grid_rows(
        &self,
        grid: &Grid,
        rows: Range<usize>,
        layout: Layout,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Sorted once per visible batch; cards only for these rows.
        let Layout { count, columns } = layout;
        let (start, end) = (rows.start * columns, (rows.end * columns).min(count));
        let cards: Vec<AnyElement> = if grid.shows {
            let shows = self.shows(grid);
            shows[start.min(shows.len())..end.min(shows.len())]
                .iter()
                .map(|s| self.show_card(SHOW_CARDS, s, cx))
                .collect()
        } else {
            let movies = self.movies(grid);
            movies[start.min(movies.len())..end.min(movies.len())]
                .iter()
                .map(|m| self.item_card(MOVIE_CARDS, m, Titled::WithShow, cx))
                .collect()
        };
        let mut cards = cards.into_iter();
        rows.map(|_| {
            div()
                .flex()
                .gap_5()
                .px_8()
                .pb_5()
                .children(cards.by_ref().take(columns))
                .into_any_element()
        })
        .collect()
    }

    /// A library tab: a sort bar over a grid that only builds the rows on
    /// screen. `scroll` is where the grid is scrolled.
    pub(in crate::shell) fn library_grid(
        &self,
        grid: Grid,
        scroll: &UniformListScrollHandle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = if grid.shows {
            shelves::show_summaries(self.items().filter(|i| grid.contains(i))).len()
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
                this.grid_rows(&grid, range, Layout { count, columns }, cx)
            }),
        )
        .track_scroll(scroll.clone())
        .flex_1()
        .min_h(px(0.0))
        .pt(px(HEADER_HEIGHT + SORT_BAR_HEIGHT));
        div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(list)
            // With frosted bars, drawn after the frost (which comes after
            // the page), like the header.
            .child(if FROSTED {
                gpui::deferred(bar).into_any_element()
            } else {
                bar
            })
            .into_any_element()
    }
}
