//! The Downloads page: what Sonarr and Radarr are downloading for each
//! Jellyfin server, how far along, what's stuck and why, and what they
//! recently added. `Page` draws what the `Downloads` model holds; a click
//! on a line asks the shell (`Event`) to open its show or movie.

use gpui::{AnyElement, Context, Entity, EventEmitter, FontWeight, div, prelude::*};

use super::{Screen, Shell};
use crate::arr::Kind;
use crate::arr::connections::Connections;
use crate::arr::downloads::{Attention, Downloads, Target};
use crate::library::Library;

mod rows;

pub(super) use rows::Activity;

/// What the page asks of the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Event {
    /// A line was clicked: `target` is its show or movie in the library.
    Opened { target: Target },
}

/// The Downloads page. It keeps nothing of its own: the model has it all.
pub(super) struct Page {
    downloads: Entity<Downloads>,
    connections: Entity<Connections>,
    library: Entity<Library>,
}

impl EventEmitter<Event> for Page {}

impl Page {
    pub(super) fn new(
        downloads: Entity<Downloads>,
        connections: Entity<Connections>,
        library: Entity<Library>,
    ) -> Self {
        Self {
            downloads,
            connections,
            library,
        }
    }

    /// The page's sections, for the shell to lay out as a scrolling page.
    pub(super) fn page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut page = vec![
            div()
                .pt_6()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .child("Downloads")
                .into_any_element(),
        ];
        let apps = self.connections.read(cx).apps().connected_ids();
        let mut servers: Vec<i64> = apps.iter().map(|id| id.server).collect();
        servers.dedup();
        let activity = Activity::of(&self.downloads, cx);
        for &server in &servers {
            let kinds: Vec<Kind> = apps
                .iter()
                .filter(|id| id.server == server)
                .map(|id| id.kind)
                .collect();
            if servers.len() > 1 {
                page.push(
                    div()
                        .pt_8()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.server_name(server, cx))
                        .into_any_element(),
                );
            }
            page.extend(activity.server_sections(server, &kinds, cx, Self::open));
        }
        page
    }

    /// The name a server's part of the page goes under.
    fn server_name(&self, server: i64, cx: &Context<Self>) -> String {
        let catalog = self.library.read(cx).catalog();
        catalog
            .server(server)
            .map_or_else(|| "Jellyfin".into(), |server| server.name.clone())
    }

    fn open(&mut self, target: &Target, cx: &mut Context<Self>) {
        cx.emit(Event::Opened {
            target: target.clone(),
        });
    }
}

/// The shell's side: it says who is looking, and opens what a line leads to.
impl Shell {
    /// Tells the downloads how closely they're followed from this window.
    pub(super) fn tell_downloads_attention(&mut self, cx: &mut Context<Self>) {
        let attention = if self.playback.read(cx).is_open() {
            Attention::Watching
        } else if matches!(
            self.navigation.screen(),
            Screen::Downloads | Screen::Arr(..)
        ) {
            Attention::Looking
        } else {
            Attention::Away
        };
        self.downloads
            .update(cx, |downloads, _| downloads.set_attention(attention));
    }

    pub(super) fn on_downloads_page_event(&mut self, event: &Event, cx: &mut Context<Self>) {
        match event {
            Event::Opened { target } => self.open_target(target, cx),
        }
    }

    /// Opens a show's page (at the season, if one is meant) or a movie's.
    pub(super) fn open_target(&mut self, target: &Target, cx: &mut Context<Self>) {
        self.library_pages
            .update(cx, |pages, cx| pages.open_target(target, cx));
    }
}
