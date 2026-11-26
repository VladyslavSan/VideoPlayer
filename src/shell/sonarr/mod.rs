//! What Sonarr says about shows: the seasons a show is missing, a Download
//! for each, their download progress, and the next air date. For shows on
//! a Jellyfin server with Sonarr set up, and for shows Sonarr keeps that
//! Jellyfin doesn't have yet. `Shows` asks Sonarr and keeps the answers;
//! a page draws them from a `Drawing`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{Context, Entity, SharedString};

use super::episode_list::EpisodeLine;
use crate::arr::connections::{Connections, Id};
use crate::arr::{self, Client, Kind, SeasonSummary};

mod drawing;
mod releases;
mod status;

pub(super) use drawing::Drawing;
use releases::ReleasePanel;
use status::Request;

/// How often an open show page asks again while something downloads.
const POLL: Duration = Duration::from_secs(10);

/// What Sonarr answered about a show.
#[derive(Clone)]
enum Answer {
    Loading,
    /// Sonarr doesn't have the show.
    Absent,
    Failed(SharedString),
    Ready(SonarrShow),
}

/// A show as Sonarr has it.
#[derive(Clone)]
struct SonarrShow {
    series: i64,
    seasons: Vec<SeasonSummary>,
    next: Option<NextEpisode>,
    /// Every episode, by season.
    episodes: BTreeMap<u32, Vec<EpisodeLine>>,
}

/// The next episode to air.
#[derive(Clone)]
struct NextEpisode {
    /// "S12E3 · Title".
    label: String,
    /// When it airs, in Unix seconds.
    airs: i64,
}

/// Where a library show is in Sonarr: its server's Sonarr, by TVDB id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Place {
    pub server: i64,
    pub tvdb: String,
}

/// How a show is found in its server's Sonarr.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SeriesRef {
    Tvdb(String),
    Id(i64),
}

/// A season of a show, by the show's name here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SeasonKey {
    show: String,
    season: u32,
}

/// Starts the names of shows from Sonarr's list (not in Jellyfin), which
/// key the same maps as shows in the library; no show's name has it.
const TRACKED: &str = "\u{1}sonarr:";

/// The name a show from Sonarr's list goes by here.
pub(super) fn tracked_show(server: i64, id: i64) -> String {
    format!("{TRACKED}{server}:{id}")
}

/// The server and Sonarr id in a name from [`tracked_show`].
fn parse_tracked_show(show: &str) -> Option<(i64, i64)> {
    let (server, id) = show.strip_prefix(TRACKED)?.split_once(':')?;
    Some((server.parse().ok()?, id.parse().ok()?))
}

/// Everything known about the shows, as a page draws it.
#[derive(Clone, Default)]
struct State {
    answers: HashMap<String, Answer>,
    /// Where each library show asked about is in Sonarr.
    places: HashMap<String, Place>,
    /// The Download buttons pressed.
    requests: HashMap<SeasonKey, Request>,
    /// Open release lists.
    panels: HashMap<SeasonKey, ReleasePanel>,
}

impl State {
    /// The server whose Sonarr has (or would have) the show, and how the
    /// show is found there.
    fn series(&self, show: &str) -> Option<(i64, SeriesRef)> {
        if let Some((server, id)) = parse_tracked_show(show) {
            return Some((server, SeriesRef::Id(id)));
        }
        let place = self.places.get(show)?;
        Some((place.server, SeriesRef::Tvdb(place.tvdb.clone())))
    }

    /// The requests made on a show's seasons.
    fn requests_on<'a>(&'a self, show: &'a str) -> impl Iterator<Item = &'a Request> {
        let requests = self.requests.iter();
        requests
            .filter(move |(key, _)| key.show == show)
            .map(|(_, request)| request)
    }
}

/// What Sonarr says about shows, and what was asked of it here.
pub(super) struct Shows {
    connections: Entity<Connections>,
    /// Shared with the page being drawn; copied only if changed meanwhile.
    state: Rc<State>,
    /// The shows being asked about.
    asking: HashSet<String>,
    /// The show whose page is open, while no video plays: it's asked
    /// about again as long as something downloads.
    open: Option<String>,
}

impl Shows {
    pub(super) fn new(connections: Entity<Connections>) -> Self {
        Self {
            connections,
            state: Rc::default(),
            asking: HashSet::new(),
            open: None,
        }
    }

    fn state_mut(&mut self) -> &mut State {
        Rc::make_mut(&mut self.state)
    }

    /// Says which show's page is open, if one is and no video plays.
    pub(super) fn set_open(&mut self, show: Option<String>) {
        self.open = show;
    }

    /// Asks Sonarr about a library show (again), `place` being where it is
    /// there. See [`Self::ask`].
    pub(super) fn load(&mut self, show: String, place: Place, cx: &mut Context<Self>) {
        self.state_mut().places.insert(show.clone(), place);
        self.ask(show, cx);
    }

    /// Asks Sonarr about a show from its own list (again).
    pub(super) fn load_tracked(&mut self, server: i64, id: i64, cx: &mut Context<Self>) {
        self.ask(tracked_show(server, id), cx);
    }

    /// The Sonarr for a show's server, and how to find the show there.
    fn sonarr_for(&self, show: &str, cx: &Context<Self>) -> Option<(Client, SeriesRef)> {
        let (server, series) = self.state.series(show)?;
        let sonarr = Id {
            server,
            kind: Kind::Sonarr,
        };
        Some((self.connections.read(cx).apps().client(sonarr)?, series))
    }

    /// Asks Sonarr about the show, if it has Sonarr. Keeps asking every
    /// few seconds while its page is open and something downloads.
    fn ask(&mut self, show: String, cx: &mut Context<Self>) {
        let Some((client, series)) = self.sonarr_for(&show, cx) else {
            return;
        };
        if !self.asking.insert(show.clone()) {
            return;
        }
        let answers = &mut self.state_mut().answers;
        answers.entry(show.clone()).or_insert(Answer::Loading);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { fetch(&client, series) })
                .await;
            this.update(cx, |this, cx| {
                this.asking.remove(&show);
                let answer = match result {
                    Ok(Some(found)) => Answer::Ready(found),
                    Ok(None) => Answer::Absent,
                    Err(e) => {
                        eprintln!("[sonarr] {show}: {e:#}");
                        Answer::Failed(format!("Couldn't ask Sonarr: {e:#}").into())
                    }
                };
                let busy = match &answer {
                    Answer::Ready(found) => {
                        let requests = this.state.requests_on(&show);
                        status::is_busy(&found.seasons, requests, Instant::now())
                    }
                    _ => false,
                };
                this.state_mut().answers.insert(show.clone(), answer);
                cx.notify();
                if busy {
                    this.ask_again_soon(show, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks about the show once more in a few seconds, if its page is
    /// still the one open then.
    fn ask_again_soon(&mut self, show: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(POLL).await;
            this.update(cx, |this, cx| {
                if this.open.as_deref() == Some(show.as_str()) {
                    this.ask(show, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks Sonarr to search for a season's episodes.
    pub(super) fn download_season(&mut self, show: String, season: u32, cx: &mut Context<Self>) {
        let Some((client, _)) = self.sonarr_for(&show, cx) else {
            return;
        };
        let Some(Answer::Ready(found)) = self.state.answers.get(&show) else {
            return;
        };
        let series = found.series;
        let episodes = found
            .seasons
            .iter()
            .find(|s| s.number == season)
            .map(|s| s.episode_ids.clone())
            .unwrap_or_default();
        let key = SeasonKey { show, season };
        self.state_mut()
            .requests
            .insert(key.clone(), Request::Sending);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.download_season(series, season, &episodes) })
                .await;
            this.update(cx, |this, cx| {
                let show = key.show.clone();
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
                this.state_mut().requests.insert(key, request);
                cx.notify();
                // Picks up the monitoring and, soon, the downloads.
                this.ask(show, cx);
            })
            .ok();
        })
        .detach();
    }
}

/// Asks Sonarr for the show, its episodes and its queue. None: Sonarr
/// doesn't have it.
fn fetch(client: &Client, series: SeriesRef) -> anyhow::Result<Option<SonarrShow>> {
    let series = match series {
        SeriesRef::Tvdb(tvdb) => match client.series_by_tvdb(&tvdb)? {
            Some(series) => series,
            None => return Ok(None),
        },
        SeriesRef::Id(id) => client.series(id)?,
    };
    let episodes = client.episodes(series.id)?;
    let queue = client.queue(series.id)?;
    let now = crate::library::now();
    let seasons = arr::summarize(&series, &episodes, &queue, now);
    let next = arr::next_airing(&episodes, now).map(|(e, at)| {
        let label = format!("S{}E{}", e.season_number, e.episode_number);
        let label = match e.title.as_deref().filter(|t| !t.is_empty()) {
            Some(title) => format!("{label} · {title}"),
            None => label,
        };
        NextEpisode { label, airs: at }
    });
    let mut by_season: BTreeMap<u32, Vec<_>> = BTreeMap::new();
    for e in &episodes {
        by_season
            .entry(e.season_number)
            .or_default()
            .push(EpisodeLine::from_sonarr(e));
    }
    for list in by_season.values_mut() {
        list.sort_by_key(|e| e.number);
    }
    Ok(Some(SonarrShow {
        series: series.id,
        seasons,
        next,
        episodes: by_season,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tracked_show_s_name_holds_its_server_and_sonarr_id() {
        let name = tracked_show(3, 115);
        assert_eq!(parse_tracked_show(&name), Some((3, 115)));
        assert_eq!(parse_tracked_show("Dark"), None);
        assert_eq!(parse_tracked_show("\u{1}sonarr:3"), None);
    }

    #[test]
    fn a_tracked_show_is_found_by_id_and_a_library_show_by_its_place() {
        let mut state = State::default();
        assert_eq!(
            state.series(&tracked_show(3, 115)),
            Some((3, SeriesRef::Id(115)))
        );
        // A library show isn't in Sonarr until its TVDB id is known.
        assert_eq!(state.series("Dark"), None);
        let place = Place {
            server: 2,
            tvdb: "334824".into(),
        };
        state.places.insert("Dark".into(), place);
        assert_eq!(
            state.series("Dark"),
            Some((2, SeriesRef::Tvdb("334824".into())))
        );
    }

    #[test]
    fn requests_are_told_apart_by_show() {
        let mut state = State::default();
        let key = |show: &str, season| SeasonKey {
            show: show.into(),
            season,
        };
        state.requests.insert(key("Dark", 1), Request::Sending);
        state.requests.insert(key("Dark", 2), Request::Sending);
        state.requests.insert(key("Another", 1), Request::Sending);
        assert_eq!(state.requests_on("Dark").count(), 2);
        assert_eq!(state.requests_on("Missing").count(), 0);
    }
}
