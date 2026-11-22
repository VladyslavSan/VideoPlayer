//! A show's page and Sonarr: the seasons the show is missing, a Download
//! button for each, their download progress, and the next air date. For
//! shows on a Jellyfin server with Sonarr set up, and for shows Sonarr
//! keeps that Jellyfin doesn't have yet (their page from Sonarr's list).

use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rgb};

use super::{ERROR, MUTED, Screen, Shell, TEXT};
use crate::arr::{self, Client, Kind, SeasonSummary};
use crate::library::Source;

/// How often an open show page asks again while something downloads.
const POLL: Duration = Duration::from_secs(10);
/// A Download asked for this recently still counts as searching.
const SEARCHING_FOR: Duration = Duration::from_secs(15 * 60);

pub(super) enum SonarrView {
    Loading,
    /// Sonarr doesn't have the show.
    Absent,
    Failed(SharedString),
    Ready(SonarrShow),
}

pub(super) struct SonarrShow {
    pub(super) series: i64,
    seasons: Vec<SeasonSummary>,
    /// "S12E3 · Title" and when it airs.
    next: Option<(String, i64)>,
}

pub(super) enum Request {
    Sending,
    /// Asked Sonarr to search for the season.
    Sent(Instant),
    /// Sent a chosen release to the download client.
    Grabbed(Instant),
    Failed(SharedString),
}

/// How a show is found in Sonarr.
pub(super) enum SeriesRef {
    Tvdb(String),
    Id(i64),
}

/// Starts the names of shows from Sonarr's list (not in Jellyfin), which
/// key the same maps as shows in the library; no show's name has it.
const TRACKED: &str = "\u{1}sonarr:";

/// The name a show from Sonarr's list goes by here.
pub(super) fn tracked_show(server: i64, id: i64) -> String {
    format!("{TRACKED}{server}:{id}")
}

/// The server and Sonarr id in a name from [`tracked_show`].
pub(super) fn parse_tracked_show(show: &str) -> Option<(i64, i64)> {
    let (server, id) = show.strip_prefix(TRACKED)?.split_once(':')?;
    Some((server.parse().ok()?, id.parse().ok()?))
}

fn now() -> i64 {
    super::now()
}

impl Shell {
    /// The Sonarr for a show's server, and how to find the show there.
    pub(super) fn sonarr_for(&self, show: &str) -> Option<(Client, SeriesRef)> {
        if let Some((server, id)) = parse_tracked_show(show) {
            return Some((self.arr_client(server, Kind::Sonarr)?, SeriesRef::Id(id)));
        }
        let server = self.items().find_map(|i| match &i.source {
            Source::Jellyfin { server, .. } if i.show.as_deref() == Some(show) => Some(*server),
            _ => None,
        })?;
        let tvdb = self.details.get(&format!("show:{show}"))?.tvdb.clone()?;
        Some((
            self.arr_client(server, Kind::Sonarr)?,
            SeriesRef::Tvdb(tvdb),
        ))
    }

    /// Whether the show's page is the one open.
    fn showing(&self, show: &str) -> bool {
        match &self.screen {
            Screen::Show(name) => name == show,
            Screen::Tracked(server, Kind::Sonarr, id) => tracked_show(*server, *id) == show,
            _ => false,
        }
    }

    /// Asks Sonarr about the show (again), if it has Sonarr. Keeps asking
    /// every few seconds while the page is open and something downloads.
    pub(super) fn load_sonarr(&mut self, show: String, cx: &mut Context<Self>) {
        let Some((client, series)) = self.sonarr_for(&show) else {
            return;
        };
        if !self.sonarr_loading.insert(show.clone()) {
            return;
        }
        self.sonarr
            .entry(show.clone())
            .or_insert(SonarrView::Loading);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let series = match series {
                        SeriesRef::Tvdb(tvdb) => match client.series_by_tvdb(&tvdb)? {
                            Some(series) => series,
                            None => return Ok(None),
                        },
                        SeriesRef::Id(id) => client.series(id)?,
                    };
                    let episodes = client.episodes(series.id)?;
                    let queue = client.queue(series.id)?;
                    let now = now();
                    let seasons = arr::summarize(&series, &episodes, &queue, now);
                    let next = arr::next_airing(&episodes, now).map(|(e, at)| {
                        let label = format!("S{}E{}", e.season_number, e.episode_number);
                        let label = match e.title.as_deref().filter(|t| !t.is_empty()) {
                            Some(title) => format!("{label} · {title}"),
                            None => label,
                        };
                        (label, at)
                    });
                    anyhow::Ok(Some(SonarrShow {
                        series: series.id,
                        seasons,
                        next,
                    }))
                })
                .await;
            this.update(cx, |this, cx| {
                this.sonarr_loading.remove(&show);
                let view = match result {
                    Ok(Some(found)) => SonarrView::Ready(found),
                    Ok(None) => SonarrView::Absent,
                    Err(e) => {
                        eprintln!("[sonarr] {show}: {e:#}");
                        SonarrView::Failed(format!("Couldn't ask Sonarr: {e:#}").into())
                    }
                };
                let busy = match &view {
                    SonarrView::Ready(found) => {
                        found.seasons.iter().any(|s| s.downloading > 0)
                            || this.sonarr_requests.iter().any(|((name, _), request)| {
                                name == &show
                                    && matches!(request, Request::Sent(at) | Request::Grabbed(at) if at.elapsed() < SEARCHING_FOR)
                            })
                    }
                    _ => false,
                };
                this.sonarr.insert(show.clone(), view);
                cx.notify();
                if busy {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(POLL).await;
                        this.update(cx, |this, cx| {
                            if this.showing(&show) && this.player.is_none() {
                                this.load_sonarr(show, cx);
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
            })
            .ok();
        })
        .detach();
    }

    fn download_season(&mut self, show: String, season: u32, cx: &mut Context<Self>) {
        let Some((client, _)) = self.sonarr_for(&show) else {
            return;
        };
        let Some(SonarrView::Ready(found)) = self.sonarr.get(&show) else {
            return;
        };
        let series = found.series;
        let episodes = found
            .seasons
            .iter()
            .find(|s| s.number == season)
            .map(|s| s.episode_ids.clone())
            .unwrap_or_default();
        self.sonarr_requests
            .insert((show.clone(), season), Request::Sending);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.download_season(series, season, &episodes) })
                .await;
            this.update(cx, |this, cx| {
                let request = match result {
                    Ok(()) => {
                        eprintln!("[sonarr] {show}: searching for season {season}");
                        Request::Sent(Instant::now())
                    }
                    Err(e) => {
                        eprintln!("[sonarr] {show} season {season}: {e:#}");
                        Request::Failed(format!("Sonarr said no: {e:#}").into())
                    }
                };
                this.sonarr_requests.insert((show.clone(), season), request);
                cx.notify();
                // Picks up the monitoring and, soon, the downloads.
                this.load_sonarr(show, cx);
            })
            .ok();
        })
        .detach();
    }

    /// The seasons Sonarr has something to say about: missing episodes,
    /// downloads, or a Download pressed.
    pub(super) fn sonarr_seasons(&self, show: &str) -> Vec<&SeasonSummary> {
        let Some(SonarrView::Ready(found)) = self.sonarr.get(show) else {
            return Vec::new();
        };
        if self.sonarr_for(show).is_none() {
            return Vec::new();
        }
        found
            .seasons
            .iter()
            .filter(|s| {
                s.wanted()
                    || self
                        .sonarr_requests
                        .contains_key(&(show.to_string(), s.number))
            })
            .collect()
    }

    /// Every season Sonarr knows of (for a show Jellyfin doesn't have).
    pub(super) fn sonarr_all_seasons(&self, show: &str) -> Vec<&SeasonSummary> {
        match self.sonarr.get(show) {
            Some(SonarrView::Ready(found)) => found.seasons.iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Lines under the season list: Sonarr's state, and the next air date.
    pub(super) fn sonarr_notes(&self, show: &str) -> Vec<AnyElement> {
        if self.sonarr_for(show).is_none() {
            return Vec::new();
        }
        let note = |text: String, color: u32| {
            div()
                .text_sm()
                .text_color(rgb(color))
                .child(text)
                .into_any_element()
        };
        match self.sonarr.get(show) {
            None => Vec::new(),
            Some(SonarrView::Loading) => vec![note("Checking Sonarr…".into(), MUTED)],
            Some(SonarrView::Absent) => vec![note(
                "Sonarr doesn't have this show, so it can't download more of it.".into(),
                MUTED,
            )],
            Some(SonarrView::Failed(e)) => vec![note(e.to_string(), ERROR)],
            Some(SonarrView::Ready(found)) => found
                .next
                .iter()
                .map(|(label, at)| {
                    note(
                        format!("Next episode: {label}, airs {}", arr::format_date(*at)),
                        MUTED,
                    )
                })
                .collect(),
        }
    }

    /// What Sonarr says about a season, in a season's row: the text, its
    /// colour, and the buttons (Choose Release, Download).
    pub(super) fn sonarr_season_controls(
        &self,
        show: &str,
        season: &SeasonSummary,
        cx: &mut Context<Self>,
    ) -> (String, u32, Vec<AnyElement>) {
        let request = self.sonarr_requests.get(&(show.to_string(), season.number));
        let episodes = |n: usize| {
            if n == 1 {
                "1 episode".to_string()
            } else {
                format!("{n} episodes")
            }
        };
        let missing = if season.downloaded == 0 {
            format!("{} not downloaded", episodes(season.missing))
        } else {
            format!("{} of {} missing", season.missing, episodes(season.aired))
        };
        let (status, color, button) = if season.downloading > 0 {
            let progress = season
                .progress
                .map(|p| format!(" · {:.0}%", p * 100.0))
                .unwrap_or_default();
            (
                format!("Downloading {}{progress}", episodes(season.downloading)),
                TEXT,
                None,
            )
        } else {
            match request {
                Some(Request::Sending) => ("Asking Sonarr…".to_string(), MUTED, None),
                Some(Request::Failed(e)) => (e.to_string(), ERROR, Some("Try Again")),
                Some(Request::Sent(at)) if at.elapsed() < SEARCHING_FOR && season.missing > 0 => (
                    format!("{missing} · Sonarr is searching for it"),
                    MUTED,
                    None,
                ),
                Some(Request::Grabbed(at))
                    if at.elapsed() < SEARCHING_FOR && season.missing > 0 =>
                {
                    (
                        format!("{missing} · sent to the download client"),
                        MUTED,
                        None,
                    )
                }
                _ if season.aired == 0 && season.monitored => (
                    "Not aired yet · Sonarr will get it when it airs".to_string(),
                    MUTED,
                    None,
                ),
                _ if season.aired == 0 => ("Not aired yet".to_string(), MUTED, Some("Download")),
                _ if season.missing == 0 => ("Downloaded".to_string(), MUTED, None),
                _ if season.monitored => (
                    format!("{missing} · Sonarr is watching for releases"),
                    MUTED,
                    Some("Search Again"),
                ),
                _ => (missing, MUTED, Some("Download")),
            }
        };
        let number = season.number;
        let choosing = self
            .release_panels
            .contains_key(&(show.to_string(), number));
        let mut buttons = Vec::new();
        // An open list can always be closed again.
        if choosing || (season.missing > 0 && season.downloading == 0) {
            let show = show.to_string();
            buttons.push(self.button(
                SharedString::from(format!("sonarr-choose-{number}")),
                if choosing {
                    "Hide Releases"
                } else {
                    "Choose Release"
                },
                cx,
                move |this, _, cx| this.toggle_releases(show.clone(), number, cx),
            ));
        }
        if let Some(label) = button {
            let show = show.to_string();
            buttons.push(self.button(
                SharedString::from(format!("sonarr-season-{number}")),
                label,
                cx,
                move |this, _, cx| this.download_season(show.clone(), number, cx),
            ));
        }
        (status, color, buttons)
    }
}
