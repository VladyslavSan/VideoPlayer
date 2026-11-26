//! What Sonarr and Radarr are downloading for each Jellyfin server and what
//! they recently added. `Downloads` asks the set-up apps: every few seconds
//! while a page shows it, now and then otherwise (for the sidebar's count).
//! It also finds the library's show or movie a download is of.

use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

use gpui::{App, Context, Entity};

use crate::arr::Subject;
use crate::arr::connections::{self, ConnectedApp, Connections, Id};
use crate::library::Library;

mod attention;
mod feeds;
mod titles;

pub use attention::Attention;
use feeds::Answer;
pub use feeds::{AppFeed, Entry, Feed, Feeds, Quiet, latest, quiet};
use titles::TitleCache;
pub use titles::{Target, TitleIndex};

/// How many history lines are asked for.
const RECENT_LIMIT: usize = 20;

/// The set-up apps' queues and histories, kept current.
pub struct Downloads {
    connections: Entity<Connections>,
    library: Entity<Library>,
    /// What the apps last said; copied if it changes while a page holds it.
    feeds: Rc<Feeds>,
    /// The library's titles, for finding what a download is of.
    titles: TitleCache,
    attention: Attention,
    polling: bool,
}

impl Downloads {
    /// Follows the apps of `connections` from when the first one is set up.
    pub fn new(
        connections: Entity<Connections>,
        library: Entity<Library>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.subscribe(&connections, |this, _, event, cx| {
            this.on_connections_event(event, cx)
        })
        .detach();
        Self {
            connections,
            library,
            feeds: Rc::default(),
            titles: TitleCache::default(),
            attention: Attention::default(),
            polling: false,
        }
    }

    /// What the apps last said, to draw from.
    pub fn feeds(&self) -> Rc<Feeds> {
        self.feeds.clone()
    }

    /// Downloads in progress, for the sidebar.
    pub fn active(&self) -> usize {
        self.feeds.active()
    }

    /// One app's downloads in progress.
    pub fn active_for(&self, id: Id) -> usize {
        self.feeds.active_for(id)
    }

    /// Says who follows the downloads now, which sets how often they're asked.
    pub fn set_attention(&mut self, attention: Attention) {
        self.attention = attention;
    }

    /// The library's titles as of now, for many lookups in a row.
    pub fn titles(&self, cx: &App) -> Rc<TitleIndex> {
        self.titles.index(&self.library.read(cx).catalog())
    }

    /// The show or movie in the server's library a line is about.
    pub fn target(&self, server: i64, subject: &Subject, cx: &App) -> Option<Target> {
        self.titles(cx).target(server, subject)
    }

    fn on_connections_event(&mut self, event: &connections::Event, cx: &mut Context<Self>) {
        match *event {
            connections::Event::Synced => self.start_poll(cx),
            connections::Event::Saved { .. } => {
                self.start_poll(cx);
                // Already polling for another app: this one's count
                // shouldn't wait for the next round.
                self.refresh(cx);
            }
        }
    }

    /// Keeps the downloads current from now on, once any app is set up.
    fn start_poll(&mut self, cx: &mut Context<Self>) {
        if self.polling || self.connections.read(cx).apps().connected().is_empty() {
            return;
        }
        self.polling = true;
        self.refresh(cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(attention::LOOKING).await;
                let alive = this.update(cx, |this, cx| {
                    let since_fetched = this.feeds.since_fetched(Instant::now());
                    if attention::is_due(this.attention, since_fetched) {
                        this.refresh(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Asks every set-up app for its queue (and, now and then, history).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let apps = self.connections.read(cx).apps().connected();
        let connected: Vec<Id> = apps.iter().map(|app| app.id).collect();
        Rc::make_mut(&mut self.feeds).start_round(&connected, Instant::now());
        for app in apps {
            if Rc::make_mut(&mut self.feeds).start_loading(app.id) {
                self.ask(app, cx);
            }
        }
    }

    /// Asks one app, off the UI thread, and takes its answer.
    fn ask(&mut self, app: ConnectedApp, cx: &mut Context<Self>) {
        let ConnectedApp { id, client } = app;
        let previous = self.feeds.previous(id);
        cx.spawn(async move |this, cx| {
            let (downloads, recent) = cx
                .background_executor()
                .spawn(async move {
                    let downloads = client.downloads()?;
                    let ids: HashSet<String> = downloads.iter().map(|d| d.id.clone()).collect();
                    let stale = previous
                        .is_none_or(|previous| previous.history_is_stale(&ids, Instant::now()));
                    let recent = stale.then(|| client.recent(&ids, RECENT_LIMIT));
                    anyhow::Ok((downloads, recent))
                })
                .await
                .map_or_else(|e| (Err(e), None), |(d, r)| (Ok(d), r));
            this.update(cx, |this, cx| {
                let answer = Answer { downloads, recent };
                Rc::make_mut(&mut this.feeds).answered(id, answer, Instant::now());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
