//! The Settings screen: a sidebar of pages on the left, the chosen page on
//! the right, each page in its own file. `Settings` is a view with its own
//! state; it changes the library and the Sonarr/Radarr connections through
//! their methods, saves settings through `Preferences`, and tells the shell
//! the little it has to act on through `Event`.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, ClickEvent, Context, Entity, EventEmitter, FocusHandle, FontWeight, KeyDownEvent,
    SharedString, Window, div, prelude::*, px, rgb, rgba,
};

use super::MUTED;
use super::page_motion::Motion;
use super::posters::Light;
use super::preferences::Preferences;
use super::sliders::Sliders;
use super::switches::Switches;
use crate::arr::connections::{self, Apps, Connections};
use crate::library::{Catalog, Library, Profile};
use crate::ui::menu;

mod app_forms;
mod appearance_page;
mod controls_page;
mod library_page;
mod playback_page;
mod sign_in;
mod sound_page;

/// The width the pages' sections are laid out at.
const PAGE_WIDTH: f32 = 760.0;
const SIDEBAR_WIDTH: f32 = 200.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Library,
    Playback,
    Controls,
    Appearance,
}

impl Page {
    const ALL: [Page; 4] = [
        Page::Library,
        Page::Playback,
        Page::Controls,
        Page::Appearance,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Library => "Library",
            Page::Playback => "Playback",
            Page::Controls => "Controls",
            Page::Appearance => "Appearance",
        }
    }
}

/// What Settings asks the shell to act on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Event {
    /// The first server was signed in to: there is a library to show now.
    FirstLibraryAdded,
    /// Posters are to be drawn this bright from now on.
    PosterLightChanged { light: Light },
    /// Whether the media keys' next and previous switch episodes changed.
    MediaSkipButtonsChanged { on: bool },
}

/// What a key pressed on the Settings screen came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyOutcome {
    /// Settings acted on it.
    Handled,
    /// Escape with nothing left to close: Settings is to be left.
    Leave,
    /// The key means nothing here.
    Ignored,
}

/// What a key could act on at the moment it is pressed.
struct KeyTargets {
    quality_menu_open: bool,
    sign_in_open: bool,
    /// The open Sonarr or Radarr form that has the keyboard focus.
    app_form: Option<connections::Id>,
}

/// What a key does on the Settings screen.
#[derive(Debug, PartialEq, Eq)]
enum KeyAction {
    CloseQualityMenu,
    CloseSignIn,
    SaveApp { id: connections::Id },
    NextAppField { id: connections::Id },
    SignIn,
    NextSignInField,
    Leave,
    Nothing,
}

/// Escape closes the quality menu, then the sign-in form, then leaves
/// Settings; Enter and Tab go to the focused app form, else the sign-in form.
fn key_action(key: &str, targets: &KeyTargets) -> KeyAction {
    match (key, targets.app_form) {
        ("escape", _) if targets.quality_menu_open => KeyAction::CloseQualityMenu,
        ("escape", _) if targets.sign_in_open => KeyAction::CloseSignIn,
        ("enter", Some(id)) => KeyAction::SaveApp { id },
        ("tab", Some(id)) => KeyAction::NextAppField { id },
        ("enter", None) if targets.sign_in_open => KeyAction::SignIn,
        ("tab", None) if targets.sign_in_open => KeyAction::NextSignInField,
        ("escape", _) => KeyAction::Leave,
        _ => KeyAction::Nothing,
    }
}

/// What Settings shares with the shell that shows it.
pub(super) struct Shared {
    pub profile: Profile,
    pub library: Entity<Library>,
    pub connections: Entity<Connections>,
    /// The page's animations: Settings draws on the shell's page.
    pub motion: Rc<Motion>,
    /// The shell's focus: where the keyboard goes when no field has it.
    pub page_focus: FocusHandle,
}

/// The Settings screen below the header.
pub(super) struct Settings {
    profile: Profile,
    library: Entity<Library>,
    /// What the library holds, as of its last change.
    catalog: Arc<Catalog>,
    connections: Entity<Connections>,
    /// What the connections know, as of their last change.
    apps: Rc<Apps>,
    motion: Rc<Motion>,
    page_focus: FocusHandle,
    page: Page,
    sign_in: sign_in::SignIn,
    /// The streaming quality menu of a server, when open.
    quality_menu: menu::Shown<library_page::QualityMenu>,
    /// Controls waiting for a key.
    key_capture: controls_page::KeyCapture,
    /// The servers' Sonarr and Radarr forms.
    app_forms: HashMap<connections::Id, app_forms::AppForm>,
    switches: Switches,
    sliders: Sliders,
    poster_light: Light,
}

impl EventEmitter<Event> for Settings {}

impl Settings {
    pub(super) fn new(shared: Shared, cx: &mut Context<Self>) -> Self {
        let Shared {
            profile,
            library,
            connections,
            motion,
            page_focus,
        } = shared;
        cx.observe(&library, |this, _, cx| this.library_changed(cx))
            .detach();
        cx.observe(&connections, |this, _, cx| this.connections_changed(cx))
            .detach();
        cx.subscribe(&connections, |this, _, event, cx| {
            this.on_connections_event(event, cx)
        })
        .detach();
        let catalog = library.read(cx).catalog();
        let apps = connections.read(cx).apps();
        let poster_light = Preferences::of(&profile.db).poster_light();
        let mut settings = Self {
            profile,
            library,
            catalog,
            connections,
            apps,
            motion,
            page_focus,
            page: Page::default(),
            sign_in: sign_in::SignIn::new(cx),
            quality_menu: menu::Shown::default(),
            key_capture: controls_page::KeyCapture::default(),
            app_forms: HashMap::new(),
            switches: Switches::default(),
            sliders: Sliders::default(),
            poster_light,
        };
        settings.sync_app_forms(cx);
        settings
    }

    /// Settings is being opened, on the page it last showed: nothing is left
    /// waiting for a key or open over it, and keys come to the page.
    pub(super) fn opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.key_capture = controls_page::KeyCapture::default();
        self.quality_menu.close();
        window.focus(&self.page_focus);
        cx.notify();
    }

    /// A key pressed while Settings shows; the shell asks `capture_key` first.
    pub(super) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> KeyOutcome {
        let targets = KeyTargets {
            quality_menu_open: self.quality_menu.current().is_some(),
            sign_in_open: self.sign_in.is_open(),
            app_form: self.focused_app_form(window, cx),
        };
        match key_action(event.keystroke.key.as_str(), &targets) {
            KeyAction::CloseQualityMenu => self.close_quality_menu(cx),
            KeyAction::CloseSignIn => self.close_sign_in(window, cx),
            KeyAction::SaveApp { id } => self.save_app(id, cx),
            KeyAction::NextAppField { id } => self.focus_next_app_field(id, window, cx),
            KeyAction::SignIn => self.connect(window, cx),
            KeyAction::NextSignInField => self.focus_next_sign_in_field(window, cx),
            KeyAction::Leave => return KeyOutcome::Leave,
            KeyAction::Nothing => return KeyOutcome::Ignored,
        }
        KeyOutcome::Handled
    }

    /// The settings saved in the library database.
    fn preferences(&self) -> Preferences<'_> {
        Preferences::of(&self.profile.db)
    }

    /// Asks the library to change, then takes its catalog as it is now.
    fn update_library<R>(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Library, &mut Context<Library>) -> R,
    ) -> R {
        let result = self.library.update(cx, change);
        self.catalog = self.library.read(cx).catalog();
        result
    }

    fn library_changed(&mut self, cx: &mut Context<Self>) {
        self.catalog = self.library.read(cx).catalog();
        cx.notify();
    }

    fn connections_changed(&mut self, cx: &mut Context<Self>) {
        self.apps = self.connections.read(cx).apps();
        self.sync_app_forms(cx);
        cx.notify();
    }

    fn on_connections_event(&mut self, event: &connections::Event, cx: &mut Context<Self>) {
        match *event {
            connections::Event::Synced => {}
            connections::Event::Saved { id } => {
                self.apps = self.connections.read(cx).apps();
                self.app_saved(id, cx);
            }
        }
    }

    /// Shows another page, which fades in like another screen.
    fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != page {
            self.motion.restart_page_fade();
        }
        self.page = page;
        self.opened(window, cx);
    }

    /// The pages to choose from, the one showing picked out.
    fn sidebar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let tabs = Page::ALL.into_iter().map(|page| {
            let active = page == self.page;
            div()
                .id(SharedString::from(format!("settings-tab-{}", page.label())))
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .when(active, |tab| {
                    tab.bg(rgba(0xffffff14)).font_weight(FontWeight::SEMIBOLD)
                })
                .when(!active, |tab| {
                    tab.text_color(rgb(MUTED))
                        .hover(|style| style.bg(rgba(0xffffff0a)))
                })
                .child(page.label())
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.show_page(page, window, cx)
                }))
        });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .gap_1()
            .pt_6()
            .children(tabs)
    }

    /// The sections of the page showing.
    fn sections(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match self.page {
            Page::Library => self.library_page(window, cx),
            Page::Playback => self.playback_page(cx),
            Page::Controls => self.controls_page(cx),
            Page::Appearance => self.appearance_page(cx),
        }
    }

    /// The page showing, scrolling by itself beside the sidebar.
    fn page(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        // The sections mustn't shrink to fit a window shorter than the page.
        let sections = self
            .sections(window, cx)
            .into_iter()
            .map(|section| div().flex_none().child(section));
        // A width, not just a cap: sections are measured at this width, so
        // rows that wrap are given the height they are drawn at.
        let column = div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(PAGE_WIDTH))
            .max_w_full()
            .children(sections);
        let mut page = div()
            .id(SharedString::from(format!(
                "settings-{}",
                self.page.label()
            )))
            .flex_1()
            .min_w(px(0.0))
            .overflow_y_scroll()
            .pb_10()
            .child(column);
        page.style().restrict_scroll_to_axis = Some(true);
        page
    }
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_10()
            .px_8()
            .child(self.sidebar(cx))
            .child(self.page(window, cx))
    }
}

/// A muted explanation under a section title.
fn note(text: &'static str) -> gpui::Div {
    div().text_sm().text_color(rgb(MUTED)).child(text)
}

/// One entry in a list: what it is on the left, its buttons on the right.
fn row(label: impl IntoElement) -> gpui::Div {
    line(label).rounded_lg().bg(rgba(0xffffff0a))
}

/// A row's layout without its background, for a line inside a card.
fn line(label: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .px_4()
        .py_3()
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::Kind;

    const NOTHING_OPEN: KeyTargets = KeyTargets {
        quality_menu_open: false,
        sign_in_open: false,
        app_form: None,
    };

    fn sonarr() -> connections::Id {
        connections::Id {
            server: 1,
            kind: Kind::Sonarr,
        }
    }

    #[test]
    fn escape_closes_the_menu_then_the_form_then_leaves() {
        let mut targets = KeyTargets {
            quality_menu_open: true,
            sign_in_open: true,
            app_form: Some(sonarr()),
        };
        assert_eq!(key_action("escape", &targets), KeyAction::CloseQualityMenu);
        targets.quality_menu_open = false;
        assert_eq!(key_action("escape", &targets), KeyAction::CloseSignIn);
        targets.sign_in_open = false;
        assert_eq!(key_action("escape", &targets), KeyAction::Leave);
        assert_eq!(key_action("escape", &NOTHING_OPEN), KeyAction::Leave);
    }

    #[test]
    fn enter_and_tab_go_to_the_focused_app_form_first() {
        let id = sonarr();
        let targets = KeyTargets {
            quality_menu_open: false,
            sign_in_open: true,
            app_form: Some(id),
        };
        assert_eq!(key_action("enter", &targets), KeyAction::SaveApp { id });
        assert_eq!(key_action("tab", &targets), KeyAction::NextAppField { id });
    }

    #[test]
    fn enter_and_tab_work_the_open_sign_in_form() {
        let targets = KeyTargets {
            sign_in_open: true,
            ..NOTHING_OPEN
        };
        assert_eq!(key_action("enter", &targets), KeyAction::SignIn);
        assert_eq!(key_action("tab", &targets), KeyAction::NextSignInField);
    }

    #[test]
    fn other_keys_mean_nothing_here() {
        assert_eq!(key_action("enter", &NOTHING_OPEN), KeyAction::Nothing);
        assert_eq!(key_action("tab", &NOTHING_OPEN), KeyAction::Nothing);
        assert_eq!(key_action("backspace", &NOTHING_OPEN), KeyAction::Nothing);
    }
}
