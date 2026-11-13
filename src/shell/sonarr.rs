//! A show's page and Sonarr: the seasons the show is missing, a Download
//! button for each, their download progress, and the next air date. Only
//! for shows on a Jellyfin server with Sonarr set up.

use std::time::{Duration, Instant};

use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*, px, rgb, rgba};

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
    series: i64,
    seasons: Vec<SeasonSummary>,
    /// "S12E3 · Title" and when it airs.
    next: Option<(String, i64)>,
}

pub(super) enum Request {
    Sending,
    Sent(Instant),
    Failed(SharedString),
}

fn now() -> i64 {
    super::now()
}

impl Shell {
    /// The Sonarr for a show's server, and the show's TVDB id.
    fn sonarr_for(&self, show: &str) -> Option<(Client, String)> {
        let server = self.items().find_map(|i| match &i.source {
            Source::Jellyfin { server, .. } if i.show.as_deref() == Some(show) => Some(*server),
            _ => None,
        })?;
        let tvdb = self.details.get(&format!("show:{show}"))?.tvdb.clone()?;
        Some((self.arr_client(server, Kind::Sonarr)?, tvdb))
    }

    /// Asks Sonarr about the show (again), if it has Sonarr. Keeps asking
    /// every few seconds while the page is open and something downloads.
    pub(super) fn load_sonarr(&mut self, show: String, cx: &mut Context<Self>) {
        let Some((client, tvdb)) = self.sonarr_for(&show) else {
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
                    let Some(series) = client.series_by_tvdb(&tvdb)? else {
                        return Ok(None);
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
                                    && matches!(request, Request::Sent(at) if at.elapsed() < SEARCHING_FOR)
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
                            if this.screen == Screen::Show(show.clone()) && this.player.is_none() {
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

    /// Under a show's seasons: what Sonarr could still get.
    pub(super) fn sonarr_section(&self, show: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.sonarr_for(show)?;
        let note = |text: String, color: u32| {
            div()
                .text_sm()
                .text_color(rgb(color))
                .child(text)
                .into_any_element()
        };
        let body: AnyElement = match self.sonarr.get(show)? {
            SonarrView::Loading => note("Checking Sonarr…".into(), MUTED),
            SonarrView::Absent => note(
                "Sonarr doesn't have this show, so it can't download more of it.".into(),
                MUTED,
            ),
            SonarrView::Failed(e) => note(e.to_string(), ERROR),
            SonarrView::Ready(found) => {
                let mut lines: Vec<AnyElement> = found
                    .seasons
                    .iter()
                    .filter(|s| {
                        s.wanted()
                            || self
                                .sonarr_requests
                                .contains_key(&(show.to_string(), s.number))
                    })
                    .map(|season| self.season_line(show, season, cx))
                    .collect();
                if lines.is_empty() {
                    lines.push(note(
                        "Every episode that has aired is downloaded.".into(),
                        MUTED,
                    ));
                }
                if let Some((label, at)) = &found.next {
                    lines.push(note(
                        format!("Next episode: {label}, airs {}", arr::format_date(*at)),
                        MUTED,
                    ));
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(lines)
                    .into_any_element()
            }
        };
        Some(Self::section("More from Sonarr", body))
    }

    fn season_line(
        &self,
        show: &str,
        season: &SeasonSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let show = show.to_string();
        div()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_3()
            .rounded_lg()
            .bg(rgba(0xffffff0a))
            .child(
                div()
                    .w(px(96.0))
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("Season {number}")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_sm()
                    .text_color(rgb(color))
                    .child(status),
            )
            .children(button.map(|label| {
                self.button(
                    SharedString::from(format!("sonarr-season-{number}")),
                    label,
                    cx,
                    move |this, _, cx| this.download_season(show.clone(), number, cx),
                )
            }))
            .into_any_element()
    }
}
