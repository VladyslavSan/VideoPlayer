//! The page of a search result: what Sonarr's or Radarr's search found.
//! If Jellyfin or the app already has it, its page there; otherwise this
//! page, from what the search knows: its picture, facts and overview, Add,
//! and for a show its seasons and episodes (from Sonarr's metadata
//! service), for a movie the other films in its series (from Radarr).

use std::collections::{BTreeMap, HashMap};

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, rgb};

use super::finder::ResultKey;
use super::{Event, Pages, subject};
use crate::arr::connections::Id;
use crate::arr::{Candidate, Collection, CollectionMovie, Kind, MetaEpisode, Subject};
use crate::shell::cards::{Card, Clicks};
use crate::shell::details::{self, Details, Hero, Overview, SHOW_HERO_HEIGHT, format_runtime};
use crate::shell::episode_list::EpisodeLine;
use crate::shell::season_block::{self, Episodes};
use crate::shell::{ERROR, MUTED, widgets};

enum Loaded<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

/// A series of films, as a server's Radarr knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CollectionKey {
    server: i64,
    /// The series' TMDB id.
    tmdb: i64,
}

#[derive(Default)]
pub(super) struct FoundPages {
    /// Shows' episodes, by TVDB id.
    episodes: HashMap<i64, Loaded<Vec<MetaEpisode>>>,
    collections: HashMap<CollectionKey, Loaded<Option<Collection>>>,
}

/// Where a result's seasons are kept open or shut.
fn seasons_key(server: i64, tvdb: i64) -> String {
    format!("\u{1}found:{server}:{tvdb}")
}

fn capitalised(text: &str) -> String {
    let mut c = text.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

impl Pages {
    /// Opens a search result's page: in the library if it's there, the
    /// app's page if the app has it, else what the search found.
    pub(super) fn open_candidate(&mut self, app: Id, candidate: Candidate, cx: &mut Context<Self>) {
        let Id { server, kind } = app;
        if let Some(id) = candidate.existing {
            let subject = subject(kind, &candidate.title, candidate.year);
            if let Some(target) = self.library_target(server, &subject, cx) {
                self.open_target(&target, cx);
                return;
            }
            // Its page reads the app's list.
            self.load_list(app, false, cx);
            self.open_tracked(app, id, cx);
            return;
        }
        for url in [candidate.fanart.clone(), candidate.poster.clone()]
            .into_iter()
            .flatten()
        {
            self.load_poster(url, cx);
        }
        match kind {
            Kind::Sonarr => self.load_found_episodes(candidate.key, cx),
            Kind::Radarr => {
                if let Some((collection, _)) = candidate.collection {
                    self.load_collection(server, collection, cx);
                }
            }
        }
        let key = candidate.key;
        self.keep_candidate(ResultKey { app, key }, candidate);
        cx.emit(Event::FoundOpened { app, key });
        cx.notify();
    }

    fn load_found_episodes(&mut self, tvdb: i64, cx: &mut Context<Self>) {
        if matches!(
            self.found.episodes.get(&tvdb),
            Some(Loaded::Loading | Loaded::Ready(_))
        ) {
            return;
        }
        self.found.episodes.insert(tvdb, Loaded::Loading);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::arr::show_episodes(tvdb) })
                .await;
            this.update(cx, |this, cx| {
                let state = match result {
                    Ok(list) => Loaded::Ready(list),
                    Err(e) => {
                        eprintln!("[sonarr] episodes of {tvdb}: {e:#}");
                        Loaded::Failed("Couldn't get its episodes.".into())
                    }
                };
                this.found.episodes.insert(tvdb, state);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_collection(&mut self, server: i64, tmdb: i64, cx: &mut Context<Self>) {
        let key = CollectionKey { server, tmdb };
        if matches!(
            self.found.collections.get(&key),
            Some(Loaded::Loading | Loaded::Ready(_))
        ) {
            return;
        }
        let Some(client) = self.client(radarr(server)) else {
            return;
        };
        self.found.collections.insert(key, Loaded::Loading);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.collection(tmdb) })
                .await;
            this.update(cx, |this, cx| {
                let state = match result {
                    Ok(found) => {
                        for poster in found
                            .iter()
                            .flat_map(|c| &c.movies)
                            .filter_map(|m| m.poster.clone())
                        {
                            this.load_poster(poster, cx);
                        }
                        Loaded::Ready(found)
                    }
                    Err(e) => {
                        eprintln!("[radarr] collection {tmdb}: {e:#}");
                        Loaded::Failed("Couldn't get the other films.".into())
                    }
                };
                this.found.collections.insert(key, state);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A film of the series: its page wherever it is, looked up by Radarr
    /// if it's not in the library.
    fn open_collection_movie(
        &mut self,
        server: i64,
        movie: CollectionMovie,
        cx: &mut Context<Self>,
    ) {
        let subject = Subject::Movie {
            title: movie.title.clone(),
            year: movie.year,
        };
        if let Some(target) = self.library_target(server, &subject, cx) {
            self.open_target(&target, cx);
            return;
        }
        let Some(client) = self.client(radarr(server)) else {
            return;
        };
        let tmdb = movie.tmdb;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.lookup_movie(tmdb) })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(Some(candidate)) => this.open_candidate(radarr(server), candidate, cx),
                Ok(None) => eprintln!("[radarr] movie {tmdb}: not found"),
                Err(e) => eprintln!("[radarr] movie {tmdb}: {e:#}"),
            })
            .ok();
        })
        .detach();
    }

    /// The page of the search result of `app`'s with this TVDB or TMDB id.
    pub(in crate::shell) fn found_page(
        &mut self,
        app: Id,
        key: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Id { server, kind } = app;
        let result = ResultKey { app, key };
        let Some(candidate) = self.kept_candidate(result).cloned() else {
            return vec![
                div()
                    .pt_6()
                    .text_color(rgb(MUTED))
                    .child("Search again to see this.")
                    .into_any_element(),
            ];
        };
        // The wide picture once it's here; the poster if there's none.
        let picture = candidate
            .fanart
            .as_deref()
            .or(candidate.poster.as_deref())
            .and_then(|url| self.poster_file(url));
        let images = details::ready_pictures(picture.into_iter().collect(), window, cx);
        let mut facts: Vec<String> = Vec::new();
        facts.extend(candidate.year.map(|y| y.to_string()));
        facts.extend(candidate.network.clone());
        facts.extend(candidate.status.as_deref().map(capitalised));
        facts.extend(candidate.certification.clone());
        facts.extend(
            candidate
                .runtime
                .map(|m| format_runtime(f64::from(m) * 60.0)),
        );
        facts.extend(candidate.rating.map(|r| format!("★ {r:.1}")));
        if !candidate.genres.is_empty() {
            facts.push(
                candidate
                    .genres
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        let hero = Hero {
            height: SHOW_HERO_HEIGHT,
            images,
            title: candidate.title.clone(),
            facts,
            tagline: None,
        };
        let hero = details::hero(&self.motion, hero);

        let when = match (kind, &candidate.released, &candidate.ended) {
            (Kind::Sonarr, Some(from), Some(to)) => {
                Some(format!("Aired {} – {}", day(from), day(to)))
            }
            (Kind::Sonarr, Some(from), None) => Some(format!("First aired {}", day(from))),
            (Kind::Radarr, Some(on), _) => Some(format!("In cinemas {}", day(on))),
            _ => None,
        };
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_4()
            .pt_5()
            .child(self.candidate_action(app, &candidate, cx))
            .children(when.map(|w| div().text_sm().text_color(rgb(MUTED)).child(w)));
        let about = Details {
            overview: candidate.overview.clone(),
            ..Details::default()
        };
        let about = Overview {
            details: Some(&about),
            ..Overview::default()
        };
        let header = div()
            .flex()
            .flex_col()
            .child(hero)
            .child(actions)
            .children(
                self.add_form_for(result, &candidate, cx)
                    .map(|form| div().pt_4().child(form)),
            )
            .children(details::overview(&self.motion, about))
            .into_any_element();

        let mut page = vec![header];
        match kind {
            Kind::Sonarr => page.push(self.found_seasons(server, key, cx)),
            Kind::Radarr => page.extend(self.found_collection(server, &candidate, cx)),
        }
        page
    }

    /// A show's seasons, each folding open to its episodes.
    fn found_seasons(&self, server: i64, tvdb: i64, cx: &mut Context<Self>) -> AnyElement {
        let note = |text: &str, color: u32| {
            div()
                .text_sm()
                .text_color(rgb(color))
                .child(text.to_string())
        };
        let body = match self.found.episodes.get(&tvdb) {
            None | Some(Loaded::Loading) => self
                .motion
                .waiting("found-episodes", note("Loading episodes…", MUTED)),
            Some(Loaded::Failed(e)) => self
                .motion
                .arriving("found-episodes-failed", note(e, ERROR)),
            Some(Loaded::Ready(list)) if list.is_empty() => self.motion.arriving(
                "found-episodes-none",
                note("No episodes listed yet.", MUTED),
            ),
            Some(Loaded::Ready(list)) => {
                let mut seasons: BTreeMap<u32, Vec<EpisodeLine>> = BTreeMap::new();
                for e in list {
                    seasons
                        .entry(e.season)
                        .or_default()
                        .push(EpisodeLine::from_meta(e));
                }
                // Specials after the seasons.
                let mut numbers: Vec<u32> = seasons.keys().copied().collect();
                numbers.sort_by_key(|n| (*n == 0, *n));
                let show = seasons_key(server, tvdb);
                let blocks: Vec<AnyElement> = numbers
                    .into_iter()
                    .filter_map(|n| {
                        let lines = seasons.remove(&n)?;
                        let number = u16::try_from(n).ok()?;
                        let season = season_block::Season {
                            show: &show,
                            number,
                            episodes: Episodes::Lines(lines),
                            sonarr: None,
                        };
                        Some(season_block::block(&self.season_host(), season, cx))
                    })
                    .collect();
                self.motion.arriving(
                    "found-seasons",
                    div().flex().flex_col().gap_2().children(blocks),
                )
            }
        };
        widgets::section("Seasons", body)
    }

    /// The other films of a movie's series.
    fn found_collection(
        &self,
        server: i64,
        candidate: &Candidate,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (id, title) = candidate.collection.clone()?;
        let note = |text: String, color: u32| div().text_sm().text_color(rgb(color)).child(text);
        let tmdb = id;
        let body = match self.found.collections.get(&CollectionKey { server, tmdb }) {
            None | Some(Loaded::Loading) => self.motion.waiting(
                "found-collection",
                note("Loading the series…".into(), MUTED),
            ),
            Some(Loaded::Failed(e)) => self
                .motion
                .arriving("found-collection-failed", note(e.to_string(), ERROR)),
            // Radarr knows a series once one of its films is added.
            Some(Loaded::Ready(None)) => self.motion.arriving(
                "found-collection-unknown",
                note(
                    "Radarr lists a series' films once one of them is added.".into(),
                    MUTED,
                ),
            ),
            Some(Loaded::Ready(Some(collection))) => {
                let cards: Vec<AnyElement> = collection
                    .movies
                    .iter()
                    .map(|movie| self.collection_card(server, candidate.key, movie, cx))
                    .collect();
                self.motion
                    .arriving("found-collection", div().child(widgets::grid(cards)))
            }
        };
        Some(widgets::section(&title, body))
    }

    fn collection_card(
        &self,
        server: i64,
        current: i64,
        movie: &CollectionMovie,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let subject = Subject::Movie {
            title: movie.title.clone(),
            year: movie.year,
        };
        let in_library = self.library_target(server, &subject, cx).is_some();
        let mut subtitle: Vec<String> = Vec::new();
        subtitle.extend(movie.year.map(|y| y.to_string()));
        if in_library {
            subtitle.push("In your library".into());
        } else if movie.in_radarr {
            subtitle.push("In Radarr".into());
        }
        let card = Card {
            id: format!("collection-{server}-{}", movie.tmdb).into(),
            thumb: movie
                .poster
                .as_deref()
                .and_then(|url| self.poster_file(url)),
            title: movie.title.clone(),
            subtitle: (!subtitle.is_empty()).then(|| subtitle.join(" · ")),
            progress: None,
            watched: false,
            // The one whose page this is.
            highlight: movie.tmdb == current,
        };
        let movie = movie.clone();
        let clicks = Clicks::new(move |this: &mut Self, _, cx| {
            if movie.tmdb != current {
                this.open_collection_movie(server, movie.clone(), cx);
            }
        });
        self.cards.card(card, clicks, cx)
    }
}

/// A server's Radarr.
fn radarr(server: i64) -> Id {
    Id {
        server,
        kind: Kind::Radarr,
    }
}

/// `2026-12-15` as the app writes dates.
fn day(date: &str) -> String {
    crate::arr::parse_utc(&format!("{date}T00:00:00Z"))
        .map(crate::arr::format_date)
        .unwrap_or_else(|| date.to_string())
}
