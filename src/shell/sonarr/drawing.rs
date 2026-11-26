//! What a page draws of Sonarr: the seasons it has something to say
//! about, its notes under a show's seasons, and each season's words and
//! buttons. A `Drawing` is taken when a page is drawn; its buttons ask
//! the `Shows` it came from.

use std::rc::Rc;
use std::time::Instant;

use gpui::{AnyElement, App, Context, Entity, SharedString, div, prelude::*, rgb};

use super::{Answer, SeasonKey, Shows, State, status};
use crate::arr::connections::{Apps, Id};
use crate::arr::{self, Kind, SeasonSummary};
use crate::shell::episode_list::EpisodeLine;
use crate::shell::page_motion::Motion;
use crate::shell::season_block::SonarrSay;
use crate::shell::{ERROR, MUTED, widgets};

/// What Sonarr says about the shows, as of when a page is drawn.
pub(in crate::shell) struct Drawing {
    pub(super) shows: Entity<Shows>,
    pub(super) state: Rc<State>,
    apps: Rc<Apps>,
}

impl Drawing {
    pub(in crate::shell) fn of(shows: &Entity<Shows>, cx: &App) -> Self {
        let model = shows.read(cx);
        Self {
            shows: shows.clone(),
            state: model.state.clone(),
            apps: model.connections.read(cx).apps(),
        }
    }

    /// Whether the show's server has a Sonarr the show can be asked of.
    fn has_sonarr(&self, show: &str) -> bool {
        self.state.series(show).is_some_and(|(server, _)| {
            let sonarr = Id {
                server,
                kind: Kind::Sonarr,
            };
            self.apps.client(sonarr).is_some()
        })
    }

    /// The seasons Sonarr has something to say about: missing episodes,
    /// downloads, or a Download pressed.
    pub(in crate::shell) fn wanted_seasons(&self, show: &str) -> Vec<&SeasonSummary> {
        let Some(Answer::Ready(found)) = self.state.answers.get(show) else {
            return Vec::new();
        };
        if !self.has_sonarr(show) {
            return Vec::new();
        }
        found
            .seasons
            .iter()
            .filter(|s| {
                let key = SeasonKey {
                    show: show.to_string(),
                    season: s.number,
                };
                s.wanted() || self.state.requests.contains_key(&key)
            })
            .collect()
    }

    /// A season's episodes as Sonarr knows them.
    pub(in crate::shell) fn episodes(&self, show: &str, season: u16) -> Vec<EpisodeLine> {
        match self.state.answers.get(show) {
            Some(Answer::Ready(found)) => found
                .episodes
                .get(&u32::from(season))
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Every season Sonarr knows of (for a show Jellyfin doesn't have).
    pub(in crate::shell) fn all_seasons(&self, show: &str) -> Vec<&SeasonSummary> {
        match self.state.answers.get(show) {
            Some(Answer::Ready(found)) => found.seasons.iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Lines under the season list: Sonarr's state, and the next air date.
    pub(in crate::shell) fn notes(&self, show: &str, motion: &Motion) -> Vec<AnyElement> {
        if !self.has_sonarr(show) {
            return Vec::new();
        }
        let note = |text: String, color: u32| div().text_sm().text_color(rgb(color)).child(text);
        match self.state.answers.get(show) {
            None => Vec::new(),
            Some(Answer::Loading) => vec![motion.waiting(
                format!("sonarr-checking-{show}"),
                note("Checking Sonarr…".into(), MUTED),
            )],
            Some(Answer::Absent) => vec![motion.arriving(
                format!("sonarr-absent-{show}"),
                note(
                    "Sonarr doesn't have this show, so it can't download more of it.".into(),
                    MUTED,
                ),
            )],
            Some(Answer::Failed(e)) => {
                vec![motion.arriving(format!("sonarr-failed-{show}"), note(e.to_string(), ERROR))]
            }
            Some(Answer::Ready(found)) => found
                .next
                .iter()
                .map(|next| {
                    let airs = arr::format_date(next.airs);
                    motion.arriving(
                        format!("sonarr-next-{show}"),
                        note(format!("Next episode: {}, airs {airs}", next.label), MUTED),
                    )
                })
                .collect(),
        }
    }

    /// What Sonarr says about a season, in a season's row: the text, its
    /// colour, the buttons (Choose Release, Download), and its release
    /// list when open.
    pub(in crate::shell) fn season_say<V: 'static>(
        &self,
        show: &str,
        season: &SeasonSummary,
        cx: &mut Context<V>,
    ) -> SonarrSay {
        let number = season.number;
        let key = SeasonKey {
            show: show.to_string(),
            season: number,
        };
        let request = self.state.requests.get(&key);
        let status = status::season_status(season, request, Instant::now());
        let choosing = self.is_choosing(show, number);
        let mut buttons = Vec::new();
        // An open list can always be closed again.
        if choosing || (season.missing > 0 && season.downloading == 0) {
            let (shows, show) = (self.shows.clone(), show.to_string());
            buttons.push(widgets::button(
                SharedString::from(format!("sonarr-choose-{number}")),
                if choosing {
                    "Hide Releases"
                } else {
                    "Choose Release"
                },
                cx,
                move |_, _, cx| {
                    let show = show.clone();
                    shows.update(cx, |shows, cx| shows.toggle_releases(show, number, cx));
                },
            ));
        }
        if let Some(label) = status.button {
            let (shows, show) = (self.shows.clone(), show.to_string());
            buttons.push(widgets::button(
                SharedString::from(format!("sonarr-season-{number}")),
                label,
                cx,
                move |_, _, cx| {
                    let show = show.clone();
                    shows.update(cx, |shows, cx| shows.download_season(show, number, cx));
                },
            ));
        }
        SonarrSay {
            status: status.text,
            color: status.color,
            buttons,
            panel: self.release_panel(show, number, cx),
        }
    }
}
