//! Search results from the library: the movies, shows and episodes a
//! query matches, across every library.

use gpui::{AnyElement, Context, div, prelude::*, rgb};

use super::Pages;
use super::item_cards::Titled;
use super::shelves::{self, ShowSummary};
use crate::library::Item;
use crate::shell::{MUTED, widgets};

/// Episode matches shown in search results.
const SEARCH_EPISODE_LIMIT: usize = 40;

/// The note for a search the library has nothing for. `found_elsewhere`:
/// Sonarr or Radarr have matches, shown under it.
pub(in crate::shell) fn nothing_matches(query: &str, found_elsewhere: bool) -> AnyElement {
    let text = if found_elsewhere {
        format!("Nothing in your library matches “{query}”.")
    } else {
        format!("Nothing matches “{query}”.")
    };
    div()
        .pt_10()
        .text_color(rgb(MUTED))
        .child(text)
        .into_any_element()
}

impl Pages {
    /// What the library has for `query`, a section for each kind. Empty
    /// when it has nothing.
    pub(in crate::shell) fn search_matches(
        &self,
        query: &str,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Across every library.
        let mut movies: Vec<&Item> = self
            .items()
            .filter(|m| {
                !m.is_episode()
                    && (shelves::matches(&m.title, query)
                        || m.year.is_some_and(|y| query.trim() == y.to_string()))
            })
            .collect();
        shelves::sort_movies(&mut movies, self.sorts.movies);
        let mut shows: Vec<ShowSummary> = shelves::show_summaries(self.items())
            .into_iter()
            .filter(|s| shelves::matches(&s.name, query))
            .collect();
        shelves::sort_shows(&mut shows, self.sorts.shows);
        let mut episodes: Vec<&Item> = self
            .items()
            .filter(|i| i.is_episode() && shelves::matches(&i.title, query))
            .collect();
        episodes.sort_by_key(|e| (e.show.clone(), e.watch_order()));

        let mut sections = Vec::new();
        if !movies.is_empty() {
            let cards = movies
                .iter()
                .map(|m| self.item_card("found-movie", m, Titled::WithShow, cx))
                .collect();
            sections.push(widgets::section("Movies", widgets::grid(cards)));
        }
        if !shows.is_empty() {
            let cards = shows
                .iter()
                .map(|s| self.show_card("found", s, cx))
                .collect();
            sections.push(widgets::section("TV Shows", widgets::grid(cards)));
        }
        if !episodes.is_empty() {
            let cards = episodes
                .iter()
                .take(SEARCH_EPISODE_LIMIT)
                .map(|e| self.item_card("found-episode", e, Titled::WithShow, cx))
                .collect();
            sections.push(widgets::section("Episodes", widgets::grid(cards)));
        }
        sections
    }
}
