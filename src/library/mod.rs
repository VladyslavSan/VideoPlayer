//! The media library: local folders and Jellyfin servers, what's in them,
//! and what has been watched. `Library` is the model that owns it all and
//! keeps it in step with the servers; views draw from a `Catalog` snapshot
//! and ask the `Library` for changes.

mod catalog;
pub mod db;
mod folders;
mod item;
mod live;
mod outbox;
pub mod parse;
mod profile;
mod remote;
mod reports;
pub mod scan;
mod sync;
pub mod thumbs;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use gpui::{Context, EventEmitter, SharedString};

use crate::jellyfin::Server;
use crate::secrets;

pub use catalog::Catalog;
pub use db::Db;
pub use item::{
    Item, MIN_RESUME_SECONDS, Section, Source, SubtitlePref, SubtitleTuning, TrackPrefs,
    WATCHED_FRACTION, next_up,
};
pub use outbox::Change;
pub use profile::{Paths, Profile};
pub use reports::{AfterReport, RemoteItem, Report};
pub use sync::Stop;

/// The time now, in Unix seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

/// What the library tells whoever subscribes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The servers signed in to may have changed; every fetch says so too.
    ServersChanged,
}

/// Everything there is to watch, kept in step with its folders and servers.
pub struct Library {
    profile: Profile,
    catalog: Arc<Catalog>,
    /// The one notice the header shows: what went wrong last.
    error: Option<SharedString>,
    scanning: folders::Scanning,
    fetching: remote::Fetching,
    live: live::Live,
    play_states: sync::PlayStates,
    /// Changes the servers haven't got yet.
    outbox: outbox::Outbox,
    reports: reports::Reports,
}

impl EventEmitter<Event> for Library {}

impl Library {
    /// The library as it was left: the database's items, each server's
    /// cached library, and the changes still to send.
    pub fn new(profile: Profile, cx: &mut Context<Self>) -> Self {
        let mut library = Self {
            profile,
            catalog: Arc::default(),
            error: None,
            scanning: folders::Scanning::default(),
            fetching: remote::Fetching::default(),
            live: live::Live::default(),
            play_states: sync::PlayStates::default(),
            outbox: outbox::Outbox::default(),
            reports: reports::Reports::default(),
        };
        library.reload(cx);
        library.load_cached_remote(cx);
        library.load_outbox(cx);
        library
    }

    /// What the library holds now. Cheap: views keep one to draw from, and
    /// take it again when the library notifies.
    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog.clone()
    }

    /// The catalog, to change. Views hold the old one until they hear of
    /// it, so this copies it: change it once per update, not per item.
    fn catalog_mut(&mut self, cx: &mut Context<Self>) -> &mut Catalog {
        cx.notify();
        Arc::make_mut(&mut self.catalog)
    }

    /// Re-reads local items, folders and servers from the database.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let db = &self.profile.db;
        let loaded = (|| anyhow::Ok((db.items()?, db.roots()?, db.servers()?)))();
        let (local, roots, mut servers) = match loaded {
            Ok(loaded) => loaded,
            Err(e) => {
                self.error = Some(format!("Couldn't read the library: {e}").into());
                return;
            }
        };
        if let Err(e) = attach_tokens(&self.profile, &mut servers) {
            self.error = Some(format!("Couldn't read saved sign-ins: {e:#}").into());
        }
        let held = &self.catalog;
        if held.local() == local && held.roots() == roots && held.servers() == servers {
            return;
        }
        let catalog = self.catalog_mut(cx);
        catalog.local = local;
        catalog.roots = roots;
        catalog.servers = servers;
    }

    /// The app has opened: brings the library up to date in the background.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
        self.generate_thumbnails(cx);
    }

    /// Looks through the folders and fetches the servers' libraries again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.rescan(cx);
        self.refresh_remote(cx);
    }

    /// What went wrong last, for the header.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Shows `error` in the header, in place of any other.
    pub fn set_error(&mut self, error: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(error.into());
        cx.notify();
    }

    pub fn clear_error(&mut self, cx: &mut Context<Self>) {
        if self.error.take().is_some() {
            cx.notify();
        }
    }

    /// Whether the folders are being scanned or a server's library fetched.
    pub fn is_loading(&self) -> bool {
        self.scanning.scanning || self.is_fetching()
    }

    /// Whether a server's library is being fetched.
    pub fn is_fetching(&self) -> bool {
        !self.fetching.loading.is_empty()
    }

    /// What the library is busy with, for the header.
    pub fn status(&self) -> Option<SharedString> {
        let progress = Progress {
            scanning: self.scanning.scanning,
            fetching: self.is_fetching(),
            artwork_left: self.fetching.artwork_left,
            thumbnails_left: self.scanning.thumbnails_left,
            // Only worth saying while the server can't be reached.
            undelivered: if self.reports.is_failing() {
                self.outbox.len()
            } else {
                0
            },
        };
        progress.status().map(Into::into)
    }
}

/// Fills in each server's token from the credential store. A token that
/// an older version left in the database is moved there first.
fn attach_tokens(profile: &Profile, servers: &mut [Server]) -> Result<()> {
    for server in servers {
        let account = server.token_account(&profile.device_id);
        if server.token.is_empty() {
            server.token = secrets::load(&account)?.unwrap_or_default();
        } else {
            secrets::save(&account, &server.token)?;
            profile.db.clear_token(server.id)?;
        }
    }
    Ok(())
}

/// How far the library's background work has come.
#[derive(Debug, Clone, Copy, Default)]
struct Progress {
    scanning: bool,
    fetching: bool,
    artwork_left: usize,
    thumbnails_left: usize,
    /// Changes waiting for a server that can't be reached.
    undelivered: usize,
}

impl Progress {
    /// The one line the header shows: the first of these under way.
    fn status(self) -> Option<String> {
        if self.scanning {
            Some("Scanning…".into())
        } else if self.fetching {
            Some("Loading Jellyfin library…".into())
        } else if self.artwork_left > 0 {
            Some(format!("Downloading artwork ({} left)", self.artwork_left))
        } else if self.thumbnails_left > 0 {
            Some(format!(
                "Generating thumbnails ({} left)",
                self.thumbnails_left
            ))
        } else if self.undelivered > 0 {
            let count = self.undelivered;
            let plural = if count == 1 { "" } else { "s" };
            Some(format!(
                "Offline: {count} change{plural} to send when the server answers"
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_names_the_first_thing_under_way() {
        let mut progress = Progress {
            scanning: true,
            fetching: true,
            artwork_left: 3,
            thumbnails_left: 2,
            undelivered: 1,
        };
        assert_eq!(progress.status().as_deref(), Some("Scanning…"));
        progress.scanning = false;
        assert_eq!(
            progress.status().as_deref(),
            Some("Loading Jellyfin library…")
        );
        progress.fetching = false;
        assert_eq!(
            progress.status().as_deref(),
            Some("Downloading artwork (3 left)")
        );
        progress.artwork_left = 0;
        assert_eq!(
            progress.status().as_deref(),
            Some("Generating thumbnails (2 left)")
        );
    }

    #[test]
    fn the_header_counts_the_changes_waiting_for_the_server() {
        let waiting = |undelivered| Progress {
            undelivered,
            ..Progress::default()
        };
        assert_eq!(
            waiting(1).status().as_deref(),
            Some("Offline: 1 change to send when the server answers")
        );
        assert_eq!(
            waiting(4).status().as_deref(),
            Some("Offline: 4 changes to send when the server answers")
        );
        assert_eq!(waiting(0).status(), None);
    }
}
