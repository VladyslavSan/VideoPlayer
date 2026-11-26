//! The app's window: which page is showing and the fade from one to the
//! next, the bar and sidebar around the pages, the keyboard, and the open
//! video in place of it all. The pages and the player are views of their
//! own; the shell arranges them and does what they ask through events.

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, PathPromptOptions, Window, div, prelude::*, px,
    rgb,
};

use crate::arr::Kind;
use crate::arr::connections::{Apps, Id};
use crate::arr::downloads::Downloads;
use crate::library::{Library, Paths, Profile};
use crate::ui::text_input::TextInput;

mod arr;
mod browse;
mod cards;
mod details;
mod downloads;
mod episode_list;
mod header;
mod keynav;
mod keys;
mod navigation;
mod page_motion;
mod playback;
mod posters;
mod preferences;
mod season_block;
mod seasons;
mod settings;
mod setup;
mod sidebar;
mod sliders;
mod sonarr;
mod switches;
mod test_captures;
mod transition;
mod widgets;
use browse::{Grid, Scope};
use cards::Cards;
use header::HEADER_HEIGHT;
use navigation::{Navigation, Screen};
use page_motion::Motion;
use playback::{NowPlaying, StartAt};
use preferences::Preferences;
use settings::Settings;
use setup::{Models, Parts};

const BG: u32 = 0x0b0d12;
const TEXT: u32 = 0xf4f5f7;
const MUTED: u32 = 0x8f98a8;
const ACCENT: u32 = 0x4c8dff;
const ERROR: u32 = 0xff7a7a;

/// Removes a folder of pictures from Sonarr and Radarr that nothing reads.
fn remove_old_pictures(old_pictures: PathBuf, cx: &mut App) {
    if old_pictures.exists() {
        cx.background_executor()
            .spawn(async move {
                let _ = std::fs::remove_dir_all(old_pictures);
            })
            .detach();
    }
}

/// Asks for folders and adds the ones picked to the library.
fn pick_folders(library: Entity<Library>, cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: true,
        prompt: Some("Add to Library".into()),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(folders))) = picked.await else {
            return;
        };
        library
            .update(cx, |library, cx| library.add_folders(&folders, cx))
            .ok();
    })
    .detach();
}

/// The root view: it arranges the pages and the player, and holds handles
/// to them and to the models they draw from, not their state.
pub struct Shell {
    /// The screen showing, its scrolling, and the page being left.
    navigation: Navigation,
    /// Where the keyboard goes on the pages.
    focus: FocusHandle,
    /// The search field in the bar at the top.
    search: Entity<TextInput>,
    /// What animates on the page being drawn; shared with what draws on it.
    motion: Rc<Motion>,
    /// The page's cards, for the arrow keys; shared with what draws them.
    cards: Rc<Cards>,
    /// Everything there is to watch, and its syncing with the servers.
    library: Entity<Library>,
    /// The servers' Sonarr and Radarr connections, as of their last change.
    apps: Rc<Apps>,
    /// What Sonarr says about shows, and what was asked of it for them.
    shows: Entity<sonarr::Shows>,
    /// What Sonarr and Radarr are downloading, kept current.
    downloads: Entity<Downloads>,
    /// The library's own pages.
    library_pages: Entity<browse::Pages>,
    /// The Sonarr and Radarr pages.
    arr: Entity<arr::Pages>,
    /// The Downloads page.
    downloads_page: Entity<downloads::Page>,
    /// The Settings screen.
    settings: Entity<Settings>,
    /// The video that's open, if one is.
    playback: Entity<NowPlaying>,
}

impl Shell {
    pub fn new(
        paths: Paths,
        source: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        crate::net::init(paths.db.with_file_name("addresses.json"));
        let profile = Profile::open(paths)?;
        let poster_light = Preferences::of(&profile.db).poster_light();
        let models = Models::new(&profile, cx);
        // Whatever changes here may change who is looking at the downloads,
        // and which show's page is open.
        cx.observe_self(|this, cx| {
            this.tell_downloads_attention(cx);
            this.tell_open_show(cx);
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window);
        let search = cx.new(|cx| TextInput::new("Search", false, cx));
        // Re-render results as the query changes.
        cx.observe(&search, |this, _, cx| {
            this.search_changed(cx);
            cx.notify();
        })
        .detach();
        let motion: Rc<Motion> = Rc::default();
        let cards = Rc::new(Cards::new(motion.clone(), poster_light));
        let old_pictures = profile.paths.thumbs.join("arr");
        let parts = Parts {
            profile,
            models,
            motion,
            cards,
            focus,
        };
        let downloads_page = parts.downloads_page(cx);
        let arr = parts.arr_pages(cx);
        let library_pages = parts.library_pages(window, cx);
        let settings = parts.settings(cx);
        let playback = parts.playback(window, cx);
        let models = parts.models;
        let shell = Self {
            apps: models.connections.read(cx).apps(),
            library: models.library,
            shows: models.shows,
            downloads: models.downloads,
            navigation: Navigation::new(),
            search,
            focus: parts.focus,
            motion: parts.motion,
            cards: parts.cards,
            library_pages,
            arr,
            downloads_page,
            settings,
            playback,
        };
        shell.watch_window(window, cx);
        if let Some(dir) = crate::test_window::capture_dir() {
            shell.serve_test_captures(dir, window, cx);
        }
        shell.library.update(cx, |library, cx| library.start(cx));
        remove_old_pictures(old_pictures, cx);
        if let Some(file) = source {
            let playback = shell.playback.clone();
            playback.update(cx, |playback, cx| playback.play_file(file, window, cx));
        }
        Ok(shell)
    }

    /// Hears the window: its close button, and the viewer coming back to it.
    fn watch_window(&self, window: &mut Window, cx: &mut Context<Self>) {
        // Closing the window while a video is open still reports it.
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            this.update(cx, |this, cx| this.close_requested(cx))
                .unwrap_or(true)
        });
        // Coming back to the app picks up changes made elsewhere (watched on
        // the TV, marked watched on the web), in case some weren't pushed.
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() || this.playback.read(cx).is_open() {
                return;
            }
            this.library.update(cx, |library, cx| library.came_back(cx));
        })
        .detach();
    }

    /// What the Sonarr and Radarr pages ask for: another page.
    fn on_arr_event(&mut self, event: &arr::Event, cx: &mut Context<Self>) {
        match event {
            arr::Event::TrackedOpened { app, id } => {
                let screen = Screen::Tracked(app.server, app.kind, *id);
                self.navigation.open(screen);
            }
            arr::Event::FoundOpened { app, key } => {
                let screen = Screen::Found(app.server, app.kind, *key);
                self.navigation.open(screen);
            }
            arr::Event::LibraryOpened { target } => self.open_target(target, cx),
            arr::Event::NameTyped => self.search_changed(cx),
        }
        cx.notify();
    }

    fn on_settings_event(&mut self, event: &settings::Event, cx: &mut Context<Self>) {
        match *event {
            settings::Event::FirstLibraryAdded => self.navigation.open(Screen::Home),
            settings::Event::PosterLightChanged { light } => self.cards.set_light(light),
            settings::Event::MediaSkipButtonsChanged { on } => {
                self.playback.read(cx).set_media_skip_buttons(on);
            }
        }
        cx.notify();
    }

    /// Shows Settings, on the page it last showed.
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigation.open(Screen::Settings);
        self.settings
            .update(cx, |settings, cx| settings.opened(window, cx));
        cx.notify();
    }

    /// Shows Settings → Library with the sign-in form open.
    fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigation.open(Screen::Settings);
        self.settings
            .update(cx, |settings, cx| settings.open_sign_in(window, cx));
        cx.notify();
    }

    /// Goes back from a page (or Settings) to the tab it was opened from.
    fn leave_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigation.back_to_tab();
        window.focus(&self.focus);
        cx.notify();
    }

    /// What the library's pages ask for: another page, or a video.
    fn on_library_event(
        &mut self,
        event: &browse::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            browse::Event::TabOpened { screen } => self.set_tab(screen.clone(), cx),
            browse::Event::ShowOpened { name, episode } => {
                self.navigation.forget_episode();
                self.navigation.open(Screen::Show(name.clone()));
                if let Some(key) = episode {
                    self.navigation.open_at_episode(key.clone());
                }
            }
            browse::Event::DetailsOpened { key } => {
                self.navigation.open(Screen::Details(key.clone()));
            }
            browse::Event::Played { key, start_at } => match start_at {
                StartAt::ResumePoint => self.play(key, window, cx),
                StartAt::Beginning => self.play_from_start(key, window, cx),
            },
            browse::Event::ConnectAsked => self.open_connect(window, cx),
        }
        cx.notify();
    }

    /// The text in the search field, trimmed.
    fn search_query(&self, cx: &Context<Self>) -> String {
        self.search.read(cx).text().trim().to_string()
    }

    /// After each change to the search field, or to the name field on a
    /// Sonarr or Radarr page: has the name looked up in the apps.
    fn search_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.search_query(cx);
        let open = match *self.navigation.screen() {
            Screen::Arr(server, kind) => Some(Id { server, kind }),
            _ => None,
        };
        self.arr
            .update(cx, |arr, cx| arr.search_changed(query, open, cx));
    }

    /// The search's results: the library's, then Sonarr's and Radarr's.
    fn search_results(&self, query: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let pages = &self.library_pages;
        let mut sections = pages.update(cx, |pages, cx| pages.search_matches(query, cx));
        let found = self.arr.update(cx, |arr, cx| arr.search_sections(cx));
        if sections.is_empty() {
            sections.push(browse::nothing_matches(query, !found.is_empty()));
        }
        sections.extend(found);
        sections
    }

    /// Tells Sonarr's shows which show's page is open, if one is and no
    /// video plays: that one is kept current.
    fn tell_open_show(&mut self, cx: &mut Context<Self>) {
        let open = match self.navigation.screen() {
            _ if self.playback.read(cx).is_open() => None,
            Screen::Show(name) => Some(name.clone()),
            Screen::Tracked(server, Kind::Sonarr, id) => Some(sonarr::tracked_show(*server, *id)),
            _ => None,
        };
        self.shows.update(cx, |shows, _| shows.set_open(open));
    }

    /// Plays an item from where it was left off.
    fn play(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |playback, cx| playback.play(key, window, cx));
    }

    fn play_from_start(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.playback.update(cx, |playback, cx| {
            playback.play_saved(key, StartAt::Beginning, window, cx)
        });
    }

    /// The player closed: the library is back, with the keyboard.
    fn on_playback_event(
        &mut self,
        event: &playback::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            playback::Event::Closed => {
                self.focus.focus(window);
                cx.notify();
            }
        }
    }

    /// The window's close button (see `NowPlaying::close_requested`).
    fn close_requested(&mut self, cx: &mut Context<Self>) -> bool {
        self.playback
            .update(cx, |playback, cx| playback.close_requested(cx))
    }

    /// A screen's page, below the header.
    fn page(&self, screen: &Screen, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let query = self.search_query(cx);
        let (pages, navigation) = (&self.library_pages, &self.navigation);
        let body: Result<Vec<AnyElement>, AnyElement> = match screen.clone() {
            _ if screen.is_tab() && !query.is_empty() => Ok(self.search_results(&query, cx)),
            Screen::Home => Ok(pages.update(cx, |pages, cx| {
                pages.home(Scope::Everything, navigation, cx)
            })),
            Screen::Source(origin) => {
                let scope = Scope::Source(origin);
                Ok(pages.update(cx, |pages, cx| pages.home(scope, navigation, cx)))
            }
            ref screen @ (Screen::Movies(_) | Screen::Shows(_)) => {
                let grid = Grid::of(screen).expect("a library tab");
                let scroll = navigation.grid_scroll();
                Err(pages.update(cx, |pages, cx| pages.library_grid(grid, scroll, window, cx)))
            }
            Screen::Show(name) => Ok(pages.update(cx, |pages, cx| {
                pages.show_page(&name, navigation, window, cx)
            })),
            Screen::Details(key) => {
                Ok(pages.update(cx, |pages, cx| pages.details_page(&key, window, cx)))
            }
            Screen::Tracked(server, kind, id) => {
                let app = Id { server, kind };
                let arr = &self.arr;
                Ok(arr.update(cx, |arr, cx| arr.tracked_page(app, id, window, cx)))
            }
            Screen::Found(server, kind, key) => {
                let app = Id { server, kind };
                let arr = &self.arr;
                Ok(arr.update(cx, |arr, cx| arr.found_page(app, key, window, cx)))
            }
            Screen::Downloads => Ok(self.downloads_page.update(cx, |page, cx| page.page(cx))),
            Screen::Arr(server, kind) => {
                let app = Id { server, kind };
                let scroll = self.navigation.content_scroll();
                let arr = &self.arr;
                Err(arr.update(cx, |arr, cx| arr.list_page(app, scroll, window, cx)))
            }
            Screen::Settings => Err(self.settings.clone().into_any_element()),
        };
        // Sections scroll as one page; the grids scroll (and virtualize) themselves.
        match body {
            Ok(sections) => {
                let mut page = div()
                    .id("content")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(self.navigation.content_scroll())
                    .px_8()
                    .pt(px(HEADER_HEIGHT))
                    .pb_10()
                    .children(sections);
                // Sideways scrolling (shift+wheel, trackpad) is for the rows.
                page.style().restrict_scroll_to_axis = Some(true);
                page.into_any_element()
            }
            // Settings doesn't scroll as a whole: it just starts below the bar.
            Err(view) if *screen == Screen::Settings => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .pt(px(HEADER_HEIGHT))
                .child(view)
                .into_any_element(),
            Err(grid) => grid,
        }
    }

    /// Whether the screen showing no longer exists: a library tab whose
    /// library went, or a page of a Sonarr or Radarr no longer set up.
    fn screen_is_gone(&self, cx: &App) -> bool {
        let catalog = self.library.read(cx).catalog();
        match self.navigation.screen() {
            Screen::Movies(_) | Screen::Shows(_) => {
                !browse::library_screens(&catalog).contains(self.navigation.screen())
            }
            Screen::Source(origin) => !browse::sources(&catalog)
                .iter()
                .any(|source| source.origin == *origin),
            Screen::Downloads => self.apps.connected_ids().is_empty(),
            Screen::Arr(server, kind)
            | Screen::Tracked(server, kind, _)
            | Screen::Found(server, kind, _) => {
                let app = Id {
                    server: *server,
                    kind: *kind,
                };
                self.apps.client(app).is_none()
            }
            _ => false,
        }
    }

    /// The page with the sidebar on its left. Settings has its own sidebar.
    fn beside_sidebar(&self, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        if *self.navigation.screen() == Screen::Settings {
            return content;
        }
        div()
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .child(self.sidebar(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(content),
            )
            .into_any_element()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(player) = self.playback.read(cx).view() {
            return div().size_full().child(player.clone()).into_any_element();
        }
        if self.screen_is_gone(cx) {
            self.navigation.open_tab(Screen::Home);
        }
        self.update_scroll(window, cx);
        self.cards.keep_animating(window);
        // Refilled as this frame's cards are laid out.
        self.cards.start_frame();
        self.navigation.cards_noted();
        let content = self.page(&self.navigation.screen().clone(), window, cx);
        let leaving = self.leaving_page(window, cx);
        let content = self.crossfade(content, leaving);
        let main = self.beside_sidebar(content, cx);
        let pages = self.library_pages.clone();
        let card_menu = pages.update(cx, |pages, cx| pages.card_menu_view(window, cx));
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .child(main)
            // Last, so it's drawn over the page.
            .children(self.frost(cx))
            .child(self.header(cx))
            .children(card_menu)
            .into_any_element()
    }
}
