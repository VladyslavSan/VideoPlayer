//! The page of a show or movie that Sonarr or Radarr keeps and Jellyfin
//! doesn't have (yet), opened from the app's list: its picture and
//! overview, and getting it. A show lists its seasons, each with Sonarr's
//! Download and Choose Release; a movie has Radarr's Download.

use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, rgb};

use super::{Event, Pages};
use crate::arr::connections::Id;
use crate::arr::{Art, Have, Kind, Tracked};
use crate::shell::details::{self, Details, Hero, Overview, SHOW_HERO_HEIGHT, format_runtime};
use crate::shell::season_block::{self, Episodes};
use crate::shell::sonarr::{self, tracked_show};
use crate::shell::{ERROR, MUTED, widgets};

/// A Download asked for this recently still counts as searching.
const SEARCHING_FOR: Duration = Duration::from_secs(15 * 60);

/// A movie in a server's Radarr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct MovieKey {
    server: i64,
    /// Its id in Radarr.
    id: i64,
}

/// A Download asked for on a movie's page.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum MovieRequest {
    Sending,
    /// Asked Radarr to search for it.
    Sent(Instant),
    Failed(SharedString),
}

/// What a movie's page says of Radarr, and the button that fits.
#[derive(Debug, Clone, PartialEq)]
struct MovieStatus {
    text: String,
    color: u32,
    /// The Download button's label, when there is something to ask for.
    button: Option<&'static str>,
}

/// The movie's state as Radarr has it, with what was asked here.
fn movie_status(
    item: &Tracked,
    request: Option<&MovieRequest>,
    now: Instant,
) -> Option<MovieStatus> {
    let Have::Movie {
        file,
        released,
        in_cinemas,
    } = item.have
    else {
        return None;
    };
    let not_out = if in_cinemas {
        "In cinemas"
    } else {
        "Not released yet"
    };
    let searching = matches!(
        request,
        Some(MovieRequest::Sent(at)) if now.saturating_duration_since(*at) < SEARCHING_FOR
    );
    let (text, color, button) = match request {
        _ if file => ("Downloaded".to_string(), MUTED, None),
        Some(MovieRequest::Sending) => ("Asking Radarr…".to_string(), MUTED, None),
        Some(MovieRequest::Failed(e)) => (e.to_string(), ERROR, Some("Try Again")),
        _ if searching => ("Radarr is searching for it".to_string(), MUTED, None),
        // Radarr only looks by itself once it's out, but a search asked
        // for now looks anyway: something may be out already.
        _ if !released && item.monitored => (
            format!("{not_out} · Radarr will get it when it's out"),
            MUTED,
            Some("Search Now"),
        ),
        _ if !released => (not_out.to_string(), MUTED, Some("Download")),
        _ if item.monitored => (
            "Missing · Radarr is watching for releases".to_string(),
            MUTED,
            Some("Search Again"),
        ),
        _ => ("Not downloaded".to_string(), MUTED, Some("Download")),
    };
    Some(MovieStatus {
        text,
        color,
        button,
    })
}

/// The facts under the title on its page: year, state, length, genres.
fn facts(item: &Tracked) -> Vec<String> {
    let mut facts = Vec::new();
    facts.extend(item.year.map(|y| y.to_string()));
    facts.extend(item.state.clone());
    facts.extend(
        item.runtime_minutes
            .map(|m| format_runtime(f64::from(m) * 60.0)),
    );
    if !item.genres.is_empty() {
        facts.push(
            item.genres
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    facts
}

impl Pages {
    /// The show or movie, from the app's list.
    fn tracked_item(&self, app: Id, id: i64) -> Option<Tracked> {
        let list = self.lists.list(app)?;
        list.iter().find(|t| t.id == id).cloned()
    }

    /// Opens the page of something `app` tracks that the library lacks.
    pub(super) fn open_tracked(&mut self, app: Id, id: i64, cx: &mut Context<Self>) {
        cx.emit(Event::TrackedOpened { app, id });
        if app.kind == Kind::Sonarr {
            // Fresh each time the page opens.
            self.shows
                .update(cx, |shows, cx| shows.load_tracked(app.server, id, cx));
        }
        cx.notify();
    }

    /// The page of the show or movie `app` tracks under this id.
    pub(in crate::shell) fn tracked_page(
        &mut self,
        app: Id,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Id { server, kind } = app;
        if self.lists.list(app).is_none() {
            // Opened from a search before the app's list was here.
            return vec![self.motion.waiting(
                "tracked-loading",
                div().pt_6().text_color(rgb(MUTED)).child("Loading…"),
            )];
        }
        let Some(item) = self.tracked_item(app, id) else {
            return vec![
                div()
                    .pt_6()
                    .text_color(rgb(MUTED))
                    .child(format!("{} doesn't have this anymore.", kind.name()))
                    .into_any_element(),
            ];
        };
        // The card's picture, and the full-size fanart over it once it's here.
        let card = item
            .art
            .as_ref()
            .and_then(|(art, _)| self.art(app, &item, *art, cx));
        let images: Vec<_> = card
            .into_iter()
            .chain(self.art(app, &item, Art::Backdrop, cx))
            .collect();
        let images = details::ready_pictures(images, window, cx);
        let facts = facts(&item);
        let hero = Hero {
            height: SHOW_HERO_HEIGHT,
            images,
            title: item.title.clone(),
            facts,
            tagline: None,
        };
        let hero = details::hero(&self.motion, hero);

        let mut actions = Vec::new();
        if kind == Kind::Radarr {
            actions.extend(self.movie_controls(server, &item, cx));
        }
        // The app's own page, for what this one doesn't do.
        if let Some(url) = self.tracked_web_page(app, &item) {
            actions.push(widgets::action_button(
                "tracked-web",
                format!("Open in {}", kind.name()),
                cx,
                move |_, _, cx| cx.open_url(&url),
            ));
        }
        let about = Details {
            overview: item.overview.clone(),
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
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_4()
                    .pt_5()
                    .children(actions),
            )
            .children(details::overview(&self.motion, about))
            .into_any_element();

        let mut page = vec![header];
        if kind == Kind::Sonarr {
            page.push(self.tracked_seasons(server, id, cx));
        }
        page
    }

    fn tracked_web_page(&self, app: Id, item: &Tracked) -> Option<String> {
        let slug = item.slug.as_ref()?;
        let path = match app.kind {
            Kind::Sonarr => format!("/series/{slug}"),
            Kind::Radarr => format!("/movie/{slug}"),
        };
        Some(self.client(app)?.web_page(&path))
    }

    /// Every season, with what Sonarr says about it and its buttons.
    fn tracked_seasons(&self, server: i64, id: i64, cx: &mut Context<Self>) -> AnyElement {
        let show = tracked_show(server, id);
        let sonarr = sonarr::Drawing::of(&self.shows, cx);
        let blocks: Vec<AnyElement> = sonarr
            .all_seasons(&show)
            .into_iter()
            .filter_map(|season| {
                let number = u16::try_from(season.number).ok()?;
                let season = season_block::Season {
                    show: &show,
                    number,
                    episodes: Episodes::Lines(sonarr.episodes(&show, number)),
                    sonarr: Some(sonarr.season_say(&show, season, cx)),
                };
                Some(season_block::block(&self.season_host(), season, cx))
            })
            .collect();
        let list = div()
            .flex()
            .flex_col()
            .gap_2()
            .children(blocks)
            .children(sonarr.notes(&show, &self.motion));
        widgets::section("Seasons", list)
    }

    /// The movie's state and its Download button.
    fn movie_controls(
        &self,
        server: i64,
        item: &Tracked,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let key = MovieKey {
            server,
            id: item.id,
        };
        let request = self.movie_requests.get(&key);
        let Some(status) = movie_status(item, request, Instant::now()) else {
            return Vec::new();
        };
        let mut controls = Vec::new();
        if let Some(label) = status.button {
            let id = item.id;
            controls.push(widgets::primary_action_button(
                "movie-download",
                label,
                cx,
                move |this, _, cx| this.download_movie(server, id, cx),
            ));
        }
        controls.push(
            div()
                .text_sm()
                .text_color(rgb(status.color))
                .child(status.text)
                .into_any_element(),
        );
        controls
    }

    fn download_movie(&mut self, server: i64, id: i64, cx: &mut Context<Self>) {
        let radarr = Id {
            server,
            kind: Kind::Radarr,
        };
        let Some(client) = self.client(radarr) else {
            return;
        };
        let key = MovieKey { server, id };
        self.movie_requests.insert(key, MovieRequest::Sending);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.download_movie(id) })
                .await;
            this.update(cx, |this, cx| {
                let request = match result {
                    Ok(()) => {
                        eprintln!("[radarr] movie {id}: searching");
                        MovieRequest::Sent(Instant::now())
                    }
                    Err(e) => {
                        eprintln!("[radarr] movie {id}: {e:#}");
                        MovieRequest::Failed(format!("Radarr said no: {e:#}").into())
                    }
                };
                this.movie_requests.insert(key, request);
                // Picks up the monitoring, and the download in Activity.
                this.load_list(radarr, true, cx);
                this.refresh_downloads(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie(file: bool, released: bool, monitored: bool) -> Tracked {
        Tracked {
            id: 7,
            title: "Heat".into(),
            sort_title: "heat".into(),
            year: Some(1995),
            monitored,
            have: Have::Movie {
                file,
                released,
                in_cinemas: false,
            },
            art: None,
            slug: None,
            overview: None,
            genres: Vec::new(),
            runtime_minutes: None,
            state: None,
        }
    }

    fn said(
        item: &Tracked,
        request: Option<&MovieRequest>,
        now: Instant,
    ) -> (String, Option<&'static str>) {
        let status = movie_status(item, request, now).expect("a movie");
        (status.text, status.button)
    }

    #[test]
    fn a_downloaded_movie_has_nothing_to_ask_for() {
        let now = Instant::now();
        let failed = MovieRequest::Failed("Radarr said no".into());
        let (text, button) = said(&movie(true, true, true), Some(&failed), now);
        assert_eq!((text.as_str(), button), ("Downloaded", None));
    }

    #[test]
    fn the_facts_are_year_state_length_and_three_genres() {
        let item = Tracked {
            state: Some("Released".into()),
            runtime_minutes: Some(170),
            genres: ["Crime", "Drama", "Action", "Thriller"]
                .map(String::from)
                .into(),
            ..movie(true, true, true)
        };
        assert_eq!(
            facts(&item),
            ["1995", "Released", "2 h 50 min", "Crime, Drama, Action"]
        );
        assert!(
            facts(&Tracked {
                year: None,
                ..movie(true, true, true)
            })
            .is_empty()
        );
    }

    #[test]
    fn a_missing_movie_offers_a_download_or_another_search() {
        let now = Instant::now();
        let (text, button) = said(&movie(false, true, false), None, now);
        assert_eq!(
            (text.as_str(), button),
            ("Not downloaded", Some("Download"))
        );
        let (text, button) = said(&movie(false, true, true), None, now);
        assert_eq!(text, "Missing · Radarr is watching for releases");
        assert_eq!(button, Some("Search Again"));
    }

    #[test]
    fn a_movie_not_out_yet_can_still_be_searched_for() {
        let now = Instant::now();
        let (text, button) = said(&movie(false, false, true), None, now);
        assert_eq!(text, "Not released yet · Radarr will get it when it's out");
        assert_eq!(button, Some("Search Now"));
        let (text, button) = said(&movie(false, false, false), None, now);
        assert_eq!(
            (text.as_str(), button),
            ("Not released yet", Some("Download"))
        );
        let in_cinemas = Tracked {
            have: Have::Movie {
                file: false,
                released: false,
                in_cinemas: true,
            },
            ..movie(false, false, false)
        };
        assert_eq!(said(&in_cinemas, None, now).0, "In cinemas");
    }

    #[test]
    fn a_download_counts_as_searching_for_fifteen_minutes() {
        let asked = Instant::now();
        let sent = MovieRequest::Sent(asked);
        let item = movie(false, true, true);
        let soon = asked + Duration::from_secs(14 * 60);
        let (text, button) = said(&item, Some(&sent), soon);
        assert_eq!(
            (text.as_str(), button),
            ("Radarr is searching for it", None)
        );
        let later = asked + Duration::from_secs(15 * 60);
        assert_eq!(said(&item, Some(&sent), later).1, Some("Search Again"));
    }

    #[test]
    fn a_pending_or_failed_download_says_so() {
        let now = Instant::now();
        let item = movie(false, true, false);
        let (text, button) = said(&item, Some(&MovieRequest::Sending), now);
        assert_eq!((text.as_str(), button), ("Asking Radarr…", None));
        let failed = MovieRequest::Failed("Radarr said no: timeout".into());
        let (text, button) = said(&item, Some(&failed), now);
        assert_eq!(text, "Radarr said no: timeout");
        assert_eq!(button, Some("Try Again"));
    }

    #[test]
    fn a_show_has_no_movie_status() {
        let show = Tracked {
            have: Have::Episodes {
                files: 1,
                wanted: 2,
                total: 3,
            },
            ..movie(false, true, true)
        };
        assert_eq!(movie_status(&show, None, Instant::now()), None);
    }
}
