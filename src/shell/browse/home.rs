//! Home and each source's page: what to continue watching, then the newest
//! in each library, in rows that scroll sideways. An empty library says how
//! to fill it.

use std::cmp::Reverse;

use gpui::{
    AnyElement, ClickEvent, Context, ElementId, FontWeight, ScrollHandle, SharedString, div,
    prelude::*, px, rgb,
};

use super::item_cards::Titled;
use super::shelves::{self, Grid, Origin, Sort, SourceEntry};
use super::{Event, Pages};
use crate::library::{self, Item};
use crate::shell::navigation::{Navigation, Screen};
use crate::shell::{MUTED, TEXT, widgets};

/// Cards in each row.
const ROW_LIMIT: usize = 20;
/// Cards in the Continue Watching row.
const CONTINUE_LIMIT: usize = 12;

/// Which sources a page of rows covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::shell) enum Scope {
    /// Home: every source together.
    Everything,
    /// One source's page.
    Source(Origin),
}

impl Scope {
    fn holds(self, item: &Item) -> bool {
        match self {
            Scope::Everything => true,
            Scope::Source(origin) => shelves::origin(item) == origin,
        }
    }

    /// What the ids of the page's rows and cards start with.
    fn prefix(self) -> String {
        match self {
            Scope::Everything => "home".to_string(),
            Scope::Source(origin) => {
                format!(
                    "source-{}",
                    origin.map_or("local".into(), |s| s.to_string())
                )
            }
        }
    }
}

/// A titled row of cards that scrolls sideways.
struct Row {
    id: SharedString,
    title: String,
    cards: Vec<AnyElement>,
    /// The library tab "See all" leads to, if the row has one.
    see_all: Option<Screen>,
    scroll: ScrollHandle,
}

impl Pages {
    /// Home or one source's page: what to continue, then the newest in
    /// each library.
    pub(in crate::shell) fn home(
        &self,
        scope: Scope,
        navigation: &Navigation,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        if let Scope::Source(origin) = scope {
            rows.push(
                div()
                    .pt_6()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(shelves::source_name(&self.catalog, origin))
                    .into_any_element(),
            );
        }
        if !self.items().any(|i| scope.holds(i)) {
            rows.push(self.nothing_here(scope, cx));
            return rows;
        }
        rows.extend(self.continue_row(scope, navigation, cx));
        rows.extend(self.latest_rows(scope, navigation, cx));
        rows
    }

    /// What a page with nothing on it says instead.
    fn nothing_here(&self, scope: Scope, cx: &mut Context<Self>) -> AnyElement {
        if scope == Scope::Everything {
            return self.empty_state(cx);
        }
        let note = div().pt_10().text_color(rgb(MUTED));
        if !self.library.read(cx).is_fetching() {
            self.motion
                .arriving("nothing-here", note.child("Nothing here yet."))
        } else {
            self.motion
                .waiting("library-loading", note.child("Loading…"))
        }
    }

    /// Half-watched videos, and the next episode of shows being watched,
    /// most recently watched first.
    fn continue_row(
        &self,
        scope: Scope,
        navigation: &Navigation,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prefix = scope.prefix();
        let mut continuing: Vec<(Option<i64>, &Item)> = self
            .items()
            .filter(|i| scope.holds(i))
            .filter(|i| i.progress().is_some())
            .map(|i| (i.last_played, i))
            .collect();
        continuing.extend(library::next_up(self.items().filter(|i| scope.holds(i))));
        continuing.sort_by_key(|(played, _)| Reverse(*played));
        if continuing.is_empty() {
            return None;
        }
        let cards = continuing
            .into_iter()
            .take(CONTINUE_LIMIT)
            .map(|(_, i)| self.item_card(&format!("{prefix}-continue"), i, Titled::WithShow, cx))
            .collect();
        let id: SharedString = format!("{prefix}-continue-row").into();
        let row = Row {
            scroll: navigation.row_scroll(&id),
            id,
            title: "Continue Watching".to_string(),
            cards,
            see_all: None,
        };
        Some(self.row(row, cx))
    }

    /// The newest in each library, like Jellyfin's "Latest ..." rows.
    fn latest_rows(
        &self,
        scope: Scope,
        navigation: &Navigation,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let prefix = scope.prefix();
        let sources: Vec<SourceEntry> = shelves::sources(&self.catalog)
            .into_iter()
            .filter(|s| match scope {
                Scope::Everything => true,
                Scope::Source(origin) => s.origin == origin,
            })
            .collect();
        // On Home with several sources, rows say which one they're from.
        let several = scope == Scope::Everything && sources.len() > 1;
        let libraries = sources.iter().flat_map(|source| {
            source
                .libraries
                .iter()
                .map(move |(label, screen)| (source, label, screen.clone()))
        });
        let mut rows = Vec::new();
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
            let id: SharedString = format!("{prefix}-{}", grid.id()).into();
            let cards = self.latest_cards(&grid, &format!("recent-{id}"), cx);
            if !cards.is_empty() {
                let row = Row {
                    scroll: navigation.row_scroll(&id),
                    id,
                    title,
                    cards,
                    see_all: Some(screen),
                };
                rows.push(self.row(row, cx));
            }
        }
        rows
    }

    /// The newest of a library, as cards whose ids start with `prefix`.
    fn latest_cards(&self, grid: &Grid, prefix: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if grid.shows {
            let mut shows = shelves::show_summaries(self.items().filter(|i| grid.contains(i)));
            shelves::sort_shows(&mut shows, Sort::RecentlyAdded);
            shows
                .iter()
                .take(ROW_LIMIT)
                .map(|s| self.show_card(prefix, s, cx))
                .collect()
        } else {
            let mut movies: Vec<&Item> = self.items().filter(|i| grid.contains(i)).collect();
            shelves::sort_movies(&mut movies, Sort::RecentlyAdded);
            movies
                .into_iter()
                .take(ROW_LIMIT)
                .map(|i| self.item_card(prefix, i, Titled::WithShow, cx))
                .collect()
        }
    }

    /// A titled, sideways-scrolling row, optionally with "See all".
    fn row(&self, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let Row {
            id,
            title,
            cards,
            see_all,
            scroll,
        } = row;
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
                            .child(title),
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
                                this.open_tab(screen.clone(), cx)
                            }))
                    })),
            )
            .child({
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

    /// Asks the shell for a library tab.
    fn open_tab(&mut self, screen: Screen, cx: &mut Context<Self>) {
        cx.emit(Event::TabOpened { screen });
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let message = if self.library.read(cx).is_loading() {
            "Loading your library…"
        } else if !self.catalog.has_sources() {
            "Connect a Jellyfin server or add a folder with movies or TV shows."
        } else {
            "No videos found yet."
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .pt(px(160.0))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Your library is empty"),
            )
            .child(div().text_color(rgb(MUTED)).child(message))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(widgets::button(
                        "jellyfin-empty",
                        "Connect Jellyfin",
                        cx,
                        |_, _, cx| cx.emit(Event::ConnectAsked),
                    ))
                    .child(widgets::button(
                        "add-empty",
                        "Add Folder",
                        cx,
                        |this, _, cx| crate::shell::pick_folders(this.library.clone(), cx),
                    )),
            )
            .into_any_element()
    }
}
