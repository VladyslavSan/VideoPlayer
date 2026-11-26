//! The Sonarr and Radarr pages: an app's own page (everything it tracks,
//! what it's downloading, adding to it by name), the page of a show or
//! movie it tracks that the library lacks, and the page of a search
//! result. `Pages` holds what they show. The shell asks it for a page's
//! elements, and changes screen when it says so (`Event`).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{AppContext as _, Context, Entity, EventEmitter};

use super::cards::Cards;
use super::page_motion::Motion;
use super::season_block;
use super::seasons::Seasons;
use super::sonarr;
use crate::arr::connections::{Apps, Connections, Id};
use crate::arr::downloads::{Downloads, Target};
use crate::arr::{Client, Kind, Subject};
use crate::library::{Catalog, Library, Profile};
use crate::ui::text_input::TextInput;

mod finder;
mod found_page;
pub(super) mod sidebar;
mod tracked_list;
mod tracked_page;

/// What the pages ask of the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Event {
    /// Show the page of something `app` tracks that the library lacks.
    TrackedOpened { app: Id, id: i64 },
    /// Show the page of a search result of `app`'s, by its TVDB or TMDB id.
    FoundOpened { app: Id, key: i64 },
    /// Show a show's or a movie's page in the library.
    LibraryOpened { target: Target },
    /// The name field on an app's page changed.
    NameTyped,
}

/// What the pages are made with: the shell's models, and what it shares
/// with every view that draws on its page.
pub(super) struct Shared {
    pub profile: Profile,
    pub library: Entity<Library>,
    pub connections: Entity<Connections>,
    pub downloads: Entity<Downloads>,
    pub shows: Entity<sonarr::Shows>,
    pub motion: Rc<Motion>,
    pub cards: Rc<Cards>,
}

/// The Sonarr and Radarr pages.
pub(super) struct Pages {
    profile: Profile,
    /// What the library holds, as of its last change.
    catalog: Arc<Catalog>,
    /// The apps' connections, as of their last change.
    apps: Rc<Apps>,
    downloads: Entity<Downloads>,
    shows: Entity<sonarr::Shows>,
    motion: Rc<Motion>,
    cards: Rc<Cards>,
    /// The seasons open on these pages, and their folding.
    seasons: Seasons,
    /// The name fields on the apps' pages (in `Kind::ALL` order).
    names: [Entity<TextInput>; 2],
    /// What the apps track, and how their pages show it.
    lists: tracked_list::Lists,
    finder: finder::Finder,
    /// What search results' pages know beyond the search.
    found: found_page::FoundPages,
    /// Downloads asked for on a movie's page.
    movie_requests: HashMap<tracked_page::MovieKey, tracked_page::MovieRequest>,
}

impl EventEmitter<Event> for Pages {}

impl Pages {
    pub(super) fn new(shared: Shared, cx: &mut Context<Self>) -> Self {
        let Shared {
            profile,
            library,
            connections,
            downloads,
            shows,
            motion,
            cards,
        } = shared;
        cx.observe(&library, |this, library, cx| {
            this.catalog = library.read(cx).catalog();
            cx.notify();
        })
        .detach();
        cx.observe(&connections, |this, connections, cx| {
            this.apps = connections.read(cx).apps();
            cx.notify();
        })
        .detach();
        cx.observe(&downloads, |_, _, cx| cx.notify()).detach();
        cx.observe(&shows, |_, _, cx| cx.notify()).detach();
        let names = Kind::ALL.map(|kind| {
            let input =
                cx.new(|cx| TextInput::new(format!("Name of the {}", noun(kind)), false, cx));
            cx.observe(&input, |_, _, cx| {
                cx.emit(Event::NameTyped);
                cx.notify();
            })
            .detach();
            input
        });
        Self {
            profile,
            catalog: library.read(cx).catalog(),
            apps: connections.read(cx).apps(),
            downloads,
            shows,
            motion,
            cards,
            seasons: Seasons::new(),
            names,
            lists: tracked_list::Lists::default(),
            finder: finder::Finder::default(),
            found: found_page::FoundPages::default(),
            movie_requests: HashMap::new(),
        }
    }

    /// The name field on an app's page.
    fn name_field(&self, kind: Kind) -> &Entity<TextInput> {
        &self.names[match kind {
            Kind::Sonarr => 0,
            Kind::Radarr => 1,
        }]
    }

    /// Empties the name field on an app's page.
    pub(super) fn clear_name(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.name_field(kind)
            .clone()
            .update(cx, |input, cx| input.set_text("", cx));
    }

    /// A client for a set-up app.
    fn client(&self, app: Id) -> Option<Client> {
        self.apps.client(app)
    }

    /// A server's name, where more than one is signed in.
    fn server_name(&self, server: i64) -> String {
        self.catalog
            .server(server)
            .map_or_else(|| "Jellyfin".into(), |server| server.name.clone())
    }

    /// The show or movie in the server's library that `subject` is about.
    fn library_target(&self, server: i64, subject: &Subject, cx: &Context<Self>) -> Option<Target> {
        self.downloads.read(cx).target(server, subject, cx)
    }

    /// Asks the shell for a show's or a movie's page in the library.
    fn open_target(&mut self, target: &Target, cx: &mut Context<Self>) {
        cx.emit(Event::LibraryOpened {
            target: target.clone(),
        });
    }

    /// Asks Sonarr and Radarr what they're downloading, now.
    fn refresh_downloads(&mut self, cx: &mut Context<Self>) {
        self.downloads
            .update(cx, |downloads, cx| downloads.refresh(cx));
    }

    /// What this view lends to the seasons it draws.
    fn season_host(&self) -> season_block::Host<'_, Self> {
        season_block::Host {
            seasons: &self.seasons,
            motion: &self.motion,
            toggle: |this, show, number| this.seasons.toggle(show, number),
        }
    }
}

/// What an app keeps: shows or movies.
fn noun(kind: Kind) -> &'static str {
    match kind {
        Kind::Sonarr => "show",
        Kind::Radarr => "movie",
    }
}

/// What an app would call a show or movie with this title and year.
fn subject(kind: Kind, title: &str, year: Option<i32>) -> Subject {
    match kind {
        Kind::Sonarr => Subject::Episodes {
            show: title.to_string(),
            year,
            episodes: Vec::new(),
        },
        Kind::Radarr => Subject::Movie {
            title: title.to_string(),
            year,
        },
    }
}

/// FNV-1a: a stable name for a cached file.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sonarr_subject_is_a_show_and_a_radarr_one_a_movie() {
        assert_eq!(
            subject(Kind::Sonarr, "Dark", Some(2017)),
            Subject::Episodes {
                show: "Dark".into(),
                year: Some(2017),
                episodes: Vec::new(),
            }
        );
        assert_eq!(
            subject(Kind::Radarr, "Heat", None),
            Subject::Movie {
                title: "Heat".into(),
                year: None,
            }
        );
    }

    #[test]
    fn cached_pictures_are_named_by_a_stable_hash_of_their_address() {
        assert_eq!(fnv(""), 0xcbf29ce484222325);
        assert_eq!(fnv("a"), 0xaf63dc4c8601ec8c);
        assert_ne!(
            fnv("/MediaCover/115/fanart.jpg?lastWrite=1"),
            fnv("/MediaCover/115/fanart.jpg?lastWrite=2")
        );
    }
}
