//! The library's own pages: Home and each source's page, the Movies and TV
//! Shows grids, search results, a show's page and a movie's. `Pages` holds
//! what they show. The shell asks it for a page's elements, and changes
//! screen or plays a video when it says so (`Event`).

use std::rc::Rc;
use std::sync::Arc;

use gpui::{Context, Entity, EventEmitter};

use super::cards::Cards;
use super::navigation::Screen;
use super::page_motion::Motion;
use super::playback::StartAt;
use super::seasons::Seasons;
use super::sonarr;
use crate::arr::downloads::Target;
use crate::jellyfin::Client;
use crate::library::{Catalog, Item, Library, Profile};
use crate::ui::menu;

mod card_menu;
mod details_page;
mod grid;
mod home;
mod item_cards;
mod search;
mod shelves;
mod show_page;

pub(super) use grid::{SIDEBAR_WIDTH, SORT_BAR_HEIGHT, columns};
pub(super) use home::Scope;
pub(super) use search::nothing_matches;
pub(super) use shelves::{Grid, Origin, Shelf, SourceEntry, library_screens, sources};

use shelves::Sort;

/// What the pages ask of the shell.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Event {
    /// Show a library tab (a row's "See all").
    TabOpened { screen: Screen },
    /// Show a show's page; `episode`: at this episode (its key), ringed.
    ShowOpened {
        name: String,
        episode: Option<String>,
    },
    /// Show a movie's page, by its key.
    DetailsOpened { key: String },
    /// Play an item, by its key.
    Played { key: String, start_at: StartAt },
    /// Show the Jellyfin sign-in form.
    ConnectAsked,
}

/// What the pages are made with: the shell's models, and what it shares
/// with every view that draws on its page.
pub(super) struct Shared {
    pub profile: Profile,
    pub library: Entity<Library>,
    pub shows: Entity<sonarr::Shows>,
    pub motion: Rc<Motion>,
    pub cards: Rc<Cards>,
}

/// The order each kind of grid is in (and its matches in a search).
struct Sorts {
    movies: Sort,
    shows: Sort,
}

/// The library's own pages.
pub(super) struct Pages {
    profile: Profile,
    library: Entity<Library>,
    /// What the library holds, as of its last change: what's drawn.
    catalog: Arc<Catalog>,
    /// What Sonarr says about shows, and what was asked of it for them.
    shows: Entity<sonarr::Shows>,
    motion: Rc<Motion>,
    cards: Rc<Cards>,
    /// The seasons open on show pages, and their folding.
    seasons: Seasons,
    sorts: Sorts,
    /// What the servers know beyond the library, for show and movie pages.
    details: details_page::Fetched,
    /// A card's right-click menu, when open.
    card_menu: menu::Shown<card_menu::CardMenu>,
}

impl EventEmitter<Event> for Pages {}

impl Pages {
    pub(super) fn new(shared: Shared, cx: &mut Context<Self>) -> Self {
        let Shared {
            profile,
            library,
            shows,
            motion,
            cards,
        } = shared;
        cx.observe(&library, |this, library, cx| {
            this.catalog = library.read(cx).catalog();
            cx.notify();
        })
        .detach();
        cx.observe(&shows, |_, _, cx| cx.notify()).detach();
        Self {
            profile,
            catalog: library.read(cx).catalog(),
            library,
            shows,
            motion,
            cards,
            seasons: Seasons::new(),
            sorts: Sorts {
                movies: Sort::Title,
                shows: Sort::RecentlyWatched,
            },
            details: details_page::Fetched::default(),
            card_menu: menu::Shown::default(),
        }
    }

    fn items(&self) -> impl Iterator<Item = &Item> {
        self.catalog.items()
    }

    fn client(&self, server: i64) -> Option<Client> {
        self.catalog.client(server, &self.profile.device_id)
    }

    /// Asks the shell to play an item.
    fn play(&mut self, key: &str, start_at: StartAt, cx: &mut Context<Self>) {
        cx.emit(Event::Played {
            key: key.to_string(),
            start_at,
        });
    }

    /// Opens a show's page (at the season, if one is meant) or a movie's.
    pub(super) fn open_target(&mut self, target: &Target, cx: &mut Context<Self>) {
        match target {
            Target::Show { name, season } => {
                self.open_show(name.clone(), cx);
                if let Some(season) = season {
                    self.seasons.open_too(name, *season as u16);
                }
            }
            Target::Movie { key } => self.open_details(key.clone(), cx),
        }
    }

    /// Marks items watched or unwatched, here and on their servers.
    fn mark_watched(&mut self, keys: &[String], watched: bool, cx: &mut Context<Self>) {
        self.library
            .update(cx, |library, cx| library.mark_watched(keys, watched, cx));
        // The cards change at once.
        self.catalog = self.library.read(cx).catalog();
        cx.notify();
    }
}
