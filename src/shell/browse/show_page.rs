//! A show's page: its backdrop, facts and overview with a button for the
//! episode to watch next, then its seasons, each folding open to its
//! episodes. Seasons Sonarr could still get are listed among them.

use std::collections::BTreeMap;

use gpui::{AnyElement, Context, Window, div, prelude::*, rgb};

use super::card_menu::MenuTarget;
use super::details_page::{self, Subject};
use super::item_cards::Titled;
use super::{Event, Pages, StartAt, grid};
use crate::library::{self, Item, Source};
use crate::shell::details::{self, Hero, Overview, SHOW_HERO_HEIGHT};
use crate::shell::navigation::Navigation;
use crate::shell::season_block::{self, Episodes};
use crate::shell::{MUTED, sonarr, widgets};

/// The episode a show's page offers: the next one up, else one in
/// progress, else the first unwatched one, else the first.
fn next_episode<'a>(items: impl Iterator<Item = &'a Item>, show: &str) -> Option<&'a Item> {
    let mut ordered: Vec<&Item> = items.filter(|i| i.show.as_deref() == Some(show)).collect();
    ordered.sort_by_key(|e| e.watch_order());
    library::next_up(ordered.iter().copied())
        .into_iter()
        .map(|(_, e)| e)
        .next()
        .or_else(|| {
            ordered
                .iter()
                .copied()
                .filter(|e| e.progress().is_some())
                .max_by_key(|e| e.last_played)
        })
        .or_else(|| ordered.iter().copied().find(|e| !e.watched))
        .or_else(|| ordered.first().copied())
}

impl Pages {
    /// Asks the shell for a show's page, with the season being watched
    /// open, and the show's server and Sonarr for what they know of it.
    pub(super) fn open_show(&mut self, name: String, cx: &mut Context<Self>) {
        self.prepare_show(&name, cx);
        cx.emit(Event::ShowOpened {
            name,
            episode: None,
        });
        cx.notify();
    }

    /// The show page of an episode's show, scrolled to that episode, which
    /// is ringed.
    pub(super) fn open_show_at(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((show, season)) = self
            .catalog
            .item(key)
            .and_then(|i| Some((i.show.clone()?, i.season.unwrap_or(0))))
        else {
            return;
        };
        self.prepare_show(&show, cx);
        // The episode's season is open, so it can be shown.
        self.seasons.open_too(&show, season);
        self.cards.forget_highlighted();
        cx.emit(Event::ShowOpened {
            name: show,
            episode: Some(key.to_string()),
        });
        cx.notify();
    }

    /// What a show's page needs before it opens.
    fn prepare_show(&mut self, name: &str, cx: &mut Context<Self>) {
        // The show's overview and backdrop come from the server, via any of
        // its episodes there.
        let episode = self
            .items()
            .find(|i| {
                i.show.as_deref() == Some(name) && matches!(i.source, Source::Jellyfin { .. })
            })
            .cloned();
        if let Some(episode) = episode {
            self.load_details(details_page::show_key(name), &episode, Subject::Show, cx);
        }
        // Only the season being watched starts open; the others are a
        // click away instead of a long scroll.
        let current = next_episode(self.items(), name).map(|e| e.season.unwrap_or(0));
        self.seasons.open_only(name, current);
        // Fresh each time the page opens (once its TVDB id is known).
        self.load_sonarr(name.to_string(), cx);
    }

    /// Asks Sonarr about a library show (again), once its TVDB id is known.
    pub(super) fn load_sonarr(&mut self, show: String, cx: &mut Context<Self>) {
        let server = self.items().find_map(|i| match &i.source {
            Source::Jellyfin { server, .. } if i.show.as_deref() == Some(show.as_str()) => {
                Some(*server)
            }
            _ => None,
        });
        let details = self.details.get(&details_page::show_key(&show));
        let tvdb = details.and_then(|details| details.tvdb.clone());
        let (Some(server), Some(tvdb)) = (server, tvdb) else {
            return;
        };
        let place = sonarr::Place { server, tvdb };
        self.shows
            .update(cx, |shows, cx| shows.load(show, place, cx));
    }

    /// A show's page: its header, then its seasons.
    pub(in crate::shell) fn show_page(
        &mut self,
        show: &str,
        navigation: &Navigation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.seasons.set_columns(grid::columns(window));
        let header = self.show_header(show, window, cx);
        let list = self.season_list(show, navigation.episode(), cx);
        vec![header, widgets::section("Seasons", list)]
    }

    /// The show's seasons, in order. `ringed`: the key of the episode the
    /// page was opened at.
    fn season_list(&self, show: &str, ringed: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let mut seasons: BTreeMap<u16, Vec<&Item>> = BTreeMap::new();
        for item in self.items().filter(|i| i.show.as_deref() == Some(show)) {
            seasons
                .entry(item.season.unwrap_or(0))
                .or_default()
                .push(item);
        }
        // Seasons Sonarr could still get join the list in order.
        let drawing = sonarr::Drawing::of(&self.shows, cx);
        let sonarr: BTreeMap<u16, &crate::arr::SeasonSummary> = drawing
            .wanted_seasons(show)
            .into_iter()
            .filter_map(|s| Some((u16::try_from(s.number).ok()?, s)))
            .collect();
        let mut numbers: Vec<u16> = seasons.keys().chain(sonarr.keys()).copied().collect();
        numbers.sort_unstable();
        numbers.dedup();
        let blocks: Vec<AnyElement> = numbers
            .into_iter()
            .map(|number| {
                let episodes = seasons.remove(&number).unwrap_or_default();
                let episodes = if episodes.is_empty() {
                    Episodes::Lines(drawing.episodes(show, number))
                } else {
                    Episodes::Cards(self.episode_cards(show, number, episodes, ringed))
                };
                let season = season_block::Season {
                    show,
                    number,
                    episodes,
                    sonarr: sonarr.get(&number).map(|s| drawing.season_say(show, s, cx)),
                };
                season_block::block(&self.season_host(), season, cx)
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(blocks)
            .children(drawing.notes(show, &self.motion))
            .into_any_element()
    }

    /// What this view lends to the seasons it draws.
    fn season_host(&self) -> season_block::Host<'_, Self> {
        season_block::Host {
            seasons: &self.seasons,
            motion: &self.motion,
            toggle: |this, show, number| this.seasons.toggle(show, number),
        }
    }

    /// A season's episodes in the library, in order, as cards.
    fn episode_cards<'a>(
        &'a self,
        show: &str,
        number: u16,
        mut episodes: Vec<&'a Item>,
        ringed: Option<&'a str>,
    ) -> season_block::EpisodeCards<'a, Self> {
        episodes.sort_by_key(|e| e.episode);
        let menu_target = MenuTarget::Season {
            show: show.to_string(),
            number,
        };
        season_block::EpisodeCards {
            count: episodes.len(),
            unwatched: episodes.iter().filter(|e| !e.watched).count(),
            draw: Box::new(move |cx| {
                let cards = episodes.into_iter();
                cards
                    .map(|e| self.item_card("episode", e, Titled::InSeason { ringed }, cx))
                    .collect()
            }),
            menu: Box::new(move |this, at, cx| this.open_card_menu(menu_target.clone(), at, cx)),
        }
    }

    /// The top of a show's page: backdrop, facts, overview, and a button for
    /// the episode to watch next.
    fn show_header(&self, show: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = details_page::show_key(show);
        let details = self.details.get(&key);
        let loading = self.details.is_loading(&key);
        let episodes: Vec<&Item> = self
            .items()
            .filter(|i| i.show.as_deref() == Some(show))
            .collect();
        let image = match details.and_then(|d| d.backdrop.clone()) {
            Some(backdrop) => Some(backdrop),
            None if loading => None,
            None => episodes.iter().find_map(|e| e.thumb.clone()),
        };
        let mut facts = vec![format!("{} episodes", episodes.len())];
        facts.extend(details_page::facts(None, None, details));
        let hero = Hero {
            height: SHOW_HERO_HEIGHT,
            images: details::ready_pictures(image.into_iter().collect(), window, cx),
            title: show.to_string(),
            facts,
            tagline: None,
        };
        let about = Overview {
            details,
            loading,
            lines: Some(3),
        };
        let next = next_episode(self.items(), show);
        div()
            .flex()
            .flex_col()
            .child(details::hero(&self.motion, hero))
            .children(next.map(|episode| self.next_button(episode, cx)))
            .children(details::overview(&self.motion, about))
            .into_any_element()
    }

    /// The button that plays (or resumes) the episode to watch next.
    fn next_button(&self, episode: &Item, cx: &mut Context<Self>) -> AnyElement {
        let key = episode.key();
        let verb = if episode.progress().is_some() {
            "Resume"
        } else {
            "Play"
        };
        let label = match episode.episode_label() {
            Some(l) => format!("▶  {verb} {l}"),
            None => format!("▶  {verb} {}", episode.title),
        };
        div()
            .flex()
            .items_center()
            .gap_4()
            .pt_5()
            .child(widgets::primary_action_button(
                "show-play",
                label,
                cx,
                move |this, _, cx| this.play(&key, StartAt::ResumePoint, cx),
            ))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(episode.title.clone()),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(season: u16, number: u16) -> Item {
        Item {
            library: None,
            source: Source::Jellyfin {
                server: 1,
                id: format!("s{season}e{number}"),
            },
            title: format!("Episode {number}"),
            year: None,
            show: Some("Dark".into()),
            season: Some(season),
            episode: Some(number),
            duration: Some(3000.0),
            position: 0.0,
            watched: false,
            last_played: None,
            thumb: None,
            added: None,
        }
    }

    #[test]
    fn a_show_not_begun_starts_at_its_first_episode() {
        let items = [episode(0, 1), episode(2, 1), episode(1, 2), episode(1, 1)];
        let next = next_episode(items.iter(), "Dark").expect("an episode");
        assert_eq!((next.season, next.episode), (Some(1), Some(1)));
        assert!(next_episode(items.iter(), "Other").is_none());
    }

    #[test]
    fn a_show_left_midway_resumes_what_was_last_played() {
        let mut older = episode(1, 1);
        older.position = 600.0;
        older.last_played = Some(10);
        let mut newer = episode(1, 3);
        newer.position = 900.0;
        newer.last_played = Some(20);
        let items = [older, episode(1, 2), newer];
        let next = next_episode(items.iter(), "Dark").expect("an episode");
        assert_eq!(next.episode, Some(3));
    }

    #[test]
    fn a_show_all_watched_offers_its_first_episode_again() {
        let mut items = [episode(1, 1), episode(1, 2)];
        for item in &mut items {
            item.watched = true;
        }
        let next = next_episode(items.iter(), "Dark").expect("an episode");
        assert_eq!(next.episode, Some(1));
    }
}
