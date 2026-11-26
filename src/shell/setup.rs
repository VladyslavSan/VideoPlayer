//! Putting the shell together: the models its pages draw from, and each of
//! its views, with what the shell hears from it. `Shell::new` makes them in
//! order.

use std::rc::Rc;

use gpui::{AppContext as _, Context, Entity, FocusHandle, Window};

use super::cards::Cards;
use super::page_motion::Motion;
use super::playback::NowPlaying;
use super::settings::{self, Settings};
use super::{Shell, arr, browse, downloads, sonarr};
use crate::arr::connections::Connections;
use crate::arr::downloads::Downloads;
use crate::library::{Library, Profile};

/// The models the shell's pages draw from.
pub(super) struct Models {
    pub library: Entity<Library>,
    pub connections: Entity<Connections>,
    pub downloads: Entity<Downloads>,
    pub shows: Entity<sonarr::Shows>,
}

impl Models {
    /// Makes the models. The shell redraws when any of them changes.
    pub(super) fn new(profile: &Profile, cx: &mut Context<Shell>) -> Self {
        let library = cx.new(|cx| Library::new(profile.clone(), cx));
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        let connections = cx.new(|cx| Connections::new(profile.clone(), library.clone(), cx));
        cx.observe(&connections, |this, connections, cx| {
            this.apps = connections.read(cx).apps();
            cx.notify();
        })
        .detach();
        let downloads = cx.new(|cx| Downloads::new(connections.clone(), library.clone(), cx));
        cx.observe(&downloads, |_, _, cx| cx.notify()).detach();
        let shows = cx.new(|_| sonarr::Shows::new(connections.clone()));
        cx.observe(&shows, |_, _, cx| cx.notify()).detach();
        Self {
            library,
            connections,
            downloads,
            shows,
        }
    }
}

/// What the shell's views are made from.
pub(super) struct Parts {
    pub profile: Profile,
    pub models: Models,
    pub motion: Rc<Motion>,
    pub cards: Rc<Cards>,
    /// Where the keyboard goes on the shell's pages.
    pub focus: FocusHandle,
}

impl Parts {
    pub(super) fn downloads_page(&self, cx: &mut Context<Shell>) -> Entity<downloads::Page> {
        let Models {
            library,
            connections,
            downloads,
            ..
        } = &self.models;
        let (library, connections, downloads) =
            (library.clone(), connections.clone(), downloads.clone());
        let page = cx.new(|_| downloads::Page::new(downloads, connections, library));
        cx.subscribe(&page, |this, _, event, cx| {
            this.on_downloads_page_event(event, cx)
        })
        .detach();
        page
    }

    pub(super) fn arr_pages(&self, cx: &mut Context<Shell>) -> Entity<arr::Pages> {
        let shared = arr::Shared {
            profile: self.profile.clone(),
            library: self.models.library.clone(),
            connections: self.models.connections.clone(),
            downloads: self.models.downloads.clone(),
            shows: self.models.shows.clone(),
            motion: self.motion.clone(),
            cards: self.cards.clone(),
        };
        let pages = cx.new(|cx| arr::Pages::new(shared, cx));
        cx.observe(&pages, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&pages, |this, _, event, cx| this.on_arr_event(event, cx))
            .detach();
        pages
    }

    pub(super) fn library_pages(
        &self,
        window: &mut Window,
        cx: &mut Context<Shell>,
    ) -> Entity<browse::Pages> {
        let shared = browse::Shared {
            profile: self.profile.clone(),
            library: self.models.library.clone(),
            shows: self.models.shows.clone(),
            motion: self.motion.clone(),
            cards: self.cards.clone(),
        };
        let pages = cx.new(|cx| browse::Pages::new(shared, cx));
        cx.observe(&pages, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&pages, window, |this, _, event, window, cx| {
            this.on_library_event(event, window, cx)
        })
        .detach();
        pages
    }

    pub(super) fn settings(&self, cx: &mut Context<Shell>) -> Entity<Settings> {
        let shared = settings::Shared {
            profile: self.profile.clone(),
            library: self.models.library.clone(),
            connections: self.models.connections.clone(),
            motion: self.motion.clone(),
            page_focus: self.focus.clone(),
        };
        let settings = cx.new(|cx| Settings::new(shared, cx));
        cx.subscribe(&settings, |this, _, event, cx| {
            this.on_settings_event(event, cx)
        })
        .detach();
        settings
    }

    pub(super) fn playback(
        &self,
        window: &mut Window,
        cx: &mut Context<Shell>,
    ) -> Entity<NowPlaying> {
        let (profile, library) = (self.profile.clone(), self.models.library.clone());
        let playback = cx.new(|_| NowPlaying::new(profile, library));
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&playback, window, |this, _, event, window, cx| {
            this.on_playback_event(event, window, cx)
        })
        .detach();
        playback
    }
}
