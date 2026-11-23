//! The page of a show or movie that Sonarr or Radarr keeps and Jellyfin
//! doesn't have (yet), opened from the app's list: its picture and
//! overview, and getting it. A show lists its seasons, each with Sonarr's
//! Download and Choose Release; a movie has Radarr's Download.

use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, rgb};

use super::details::{Details, SHOW_HERO_HEIGHT, format_runtime};
use super::sonarr::tracked_show;
use super::{ERROR, MUTED, Screen, Shell};
use crate::arr::{Art, Have, Kind, Tracked};

/// A Download asked for this recently still counts as searching.
const SEARCHING_FOR: Duration = Duration::from_secs(15 * 60);

pub(super) enum MovieRequest {
    Sending,
    /// Asked Radarr to search for it.
    Sent(Instant),
    Failed(SharedString),
}

impl Shell {
    /// The show or movie, from the app's list.
    fn tracked_item(&self, server: i64, kind: Kind, id: i64) -> Option<Tracked> {
        let list = self.arr_pages.list((server, kind))?;
        list.iter().find(|t| t.id == id).cloned()
    }

    pub(super) fn open_tracked(
        &mut self,
        server: i64,
        kind: Kind,
        id: i64,
        cx: &mut Context<Self>,
    ) {
        self.screen = Screen::Tracked(server, kind, id);
        if kind == Kind::Sonarr {
            // Fresh each time the page opens.
            self.load_sonarr(tracked_show(server, id), cx);
        }
        cx.notify();
    }

    pub(super) fn tracked_page(
        &mut self,
        server: i64,
        kind: Kind,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if self.arr_pages.list((server, kind)).is_none() {
            // Opened from a search before the app's list was here.
            return vec![self.waiting(
                "tracked-loading",
                div().pt_6().text_color(rgb(MUTED)).child("Loading…"),
            )];
        }
        let Some(item) = self.tracked_item(server, kind, id) else {
            return vec![
                div()
                    .pt_6()
                    .text_color(rgb(MUTED))
                    .child(format!("{} doesn't have this anymore.", kind.name()))
                    .into_any_element(),
            ];
        };
        let key = (server, kind);
        // The card's picture, and the full-size fanart over it once it's here.
        let card = item
            .art
            .as_ref()
            .and_then(|(art, _)| self.arr_art(key, &item, *art, cx));
        let images: Vec<_> = card
            .into_iter()
            .chain(self.arr_art(key, &item, Art::Backdrop, cx))
            .collect();
        let images = Self::ready_pictures(images, window, cx);
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
        let hero = self.hero(SHOW_HERO_HEIGHT, images, item.title.clone(), facts, None);

        let mut actions = Vec::new();
        if kind == Kind::Radarr {
            actions.extend(self.movie_controls(server, &item, cx));
        }
        // The app's own page, for what this one doesn't do.
        if let Some(url) = self.tracked_web_page(server, kind, &item) {
            actions.push(Self::action_button(
                "tracked-web",
                format!("Open in {}", kind.name()),
                false,
                cx,
                move |_, _, cx| cx.open_url(&url),
            ));
        }
        let details = Details {
            overview: item.overview.clone(),
            ..Details::default()
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
            .children(self.overview(Some(&details), false, None))
            .into_any_element();

        let mut page = vec![header];
        if kind == Kind::Sonarr {
            page.push(self.tracked_seasons(server, id, cx));
        }
        page
    }

    fn tracked_web_page(&self, server: i64, kind: Kind, item: &Tracked) -> Option<String> {
        let slug = item.slug.as_ref()?;
        let path = match kind {
            Kind::Sonarr => format!("/series/{slug}"),
            Kind::Radarr => format!("/movie/{slug}"),
        };
        Some(self.arr_client(server, kind)?.web_page(&path))
    }

    /// Every season, with what Sonarr says about it and its buttons.
    fn tracked_seasons(&self, server: i64, id: i64, cx: &mut Context<Self>) -> AnyElement {
        let show = tracked_show(server, id);
        let blocks: Vec<AnyElement> = self
            .sonarr_all_seasons(&show)
            .into_iter()
            .filter_map(|season| {
                let number = u16::try_from(season.number).ok()?;
                let listed = self.sonarr_episodes(&show, number);
                Some(self.season_block(&show, number, Vec::new(), Some(season), listed, cx))
            })
            .collect();
        let list = div()
            .flex()
            .flex_col()
            .gap_2()
            .children(blocks)
            .children(self.sonarr_notes(&show));
        Self::section("Seasons", list)
    }

    /// The movie's state and its Download button.
    fn movie_controls(
        &self,
        server: i64,
        item: &Tracked,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Have::Movie { file, released } = item.have else {
            return Vec::new();
        };
        let request = self.movie_requests.get(&(server, item.id));
        let searching =
            matches!(request, Some(MovieRequest::Sent(at)) if at.elapsed() < SEARCHING_FOR);
        let (status, color, button) = match request {
            _ if file => ("Downloaded".to_string(), MUTED, None),
            Some(MovieRequest::Sending) => ("Asking Radarr…".to_string(), MUTED, None),
            Some(MovieRequest::Failed(e)) => (e.to_string(), ERROR, Some("Try Again")),
            _ if searching => ("Radarr is searching for it".to_string(), MUTED, None),
            _ if !released && item.monitored => (
                "Not released yet · Radarr will get it when it's out".to_string(),
                MUTED,
                None,
            ),
            _ if !released => ("Not released yet".to_string(), MUTED, Some("Download")),
            _ if item.monitored => (
                "Missing · Radarr is watching for releases".to_string(),
                MUTED,
                Some("Search Again"),
            ),
            _ => ("Not downloaded".to_string(), MUTED, Some("Download")),
        };
        let mut controls = Vec::new();
        if let Some(label) = button {
            let id = item.id;
            controls.push(Self::action_button(
                "movie-download",
                label,
                true,
                cx,
                move |this, _, cx| this.download_movie(server, id, cx),
            ));
        }
        controls.push(
            div()
                .text_sm()
                .text_color(rgb(color))
                .child(status)
                .into_any_element(),
        );
        controls
    }

    fn download_movie(&mut self, server: i64, id: i64, cx: &mut Context<Self>) {
        let Some(client) = self.arr_client(server, Kind::Radarr) else {
            return;
        };
        self.movie_requests
            .insert((server, id), MovieRequest::Sending);
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
                this.movie_requests.insert((server, id), request);
                // Picks up the monitoring, and the download in Activity.
                this.load_arr_list((server, Kind::Radarr), true, cx);
                this.refresh_downloads(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
