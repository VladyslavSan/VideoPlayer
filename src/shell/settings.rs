//! The Settings screen: a sidebar of pages on the left, the chosen page on
//! the right. Library holds the Jellyfin servers and local folders;
//! Playback holds player preferences, kept in the library database;
//! Appearance holds how bright posters are (`posters.rs`).

use gpui::{
    Animation, AnimationExt, AnyElement, ClickEvent, Context, Focusable, FontWeight, SharedString,
    Window, div, prelude::*, px, rgb, rgba,
};

use super::{ACCENT, ERROR, MUTED, Screen, Shell};
use crate::video::{SoundProfile, SoundSettings};

/// Whether headphone and media-key next/previous switch episodes.
const MEDIA_SKIP_SETTING: &str = "media_skip_buttons";
/// Whether the player's sound filter is on. (Saved under night mode's
/// keys, from before it was more than that.)
const SOUND_ON_SETTING: &str = "night_mode";
/// Which sound filter: the preset or custom tuning picked, and the custom
/// tunings (`SoundSettings`, as JSON).
const SOUND_SETTINGS: &str = "night_mode_profiles";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum SettingsPage {
    #[default]
    Library,
    Playback,
    Controls,
    Appearance,
}

impl SettingsPage {
    const ALL: [SettingsPage; 4] = [
        SettingsPage::Library,
        SettingsPage::Playback,
        SettingsPage::Controls,
        SettingsPage::Appearance,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsPage::Library => "Library",
            SettingsPage::Playback => "Playback",
            SettingsPage::Controls => "Controls",
            SettingsPage::Appearance => "Appearance",
        }
    }
}

impl Shell {
    pub(super) fn media_skip_buttons(&self) -> bool {
        self.flag(MEDIA_SKIP_SETTING, true)
    }

    fn set_media_skip_buttons(&mut self, on: bool) {
        self.set_flag(MEDIA_SKIP_SETTING, on);
        if let Some(media) = &self.media {
            media.set_skip_buttons(on);
        }
    }

    /// Whether videos start with the sound filter on. The player switches
    /// it too, and saves what it was left at.
    pub(super) fn sound_on(&self) -> bool {
        self.flag(SOUND_ON_SETTING, false)
    }

    /// The player picked a sound filter or switched it off.
    pub(super) fn sound_chosen(&self, on: bool, profile: SoundProfile) {
        self.set_flag(SOUND_ON_SETTING, on);
        let mut settings = self.sound_settings();
        if settings.profile != profile {
            settings.profile = profile;
            self.set_sound_settings(settings);
        }
    }

    pub(super) fn sound_settings(&self) -> SoundSettings {
        self.db
            .setting(SOUND_SETTINGS)
            .ok()
            .flatten()
            .map(|json| SoundSettings::from_json(&json))
            .unwrap_or_default()
    }

    pub(super) fn set_sound_settings(&self, settings: SoundSettings) {
        if let Err(e) = self.db.set_setting(SOUND_SETTINGS, &settings.to_json()) {
            eprintln!("[settings] saving the sound filter: {e:#}");
        }
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        match self.db.setting(key).ok().flatten().as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => default,
        }
    }

    fn set_flag(&self, key: &str, on: bool) {
        if let Err(e) = self.db.set_setting(key, if on { "1" } else { "0" }) {
            eprintln!("[settings] saving {key}: {e:#}");
        }
    }

    pub(super) fn open_settings(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.screen == Screen::Settings && self.settings_page != page {
            // Another Settings page fades in like another screen.
            self.page_generation += 1;
        }
        self.screen = Screen::Settings;
        self.settings_page = page;
        self.key_capture = Default::default();
        self.quality_menu.close();
        window.focus(&self.focus);
        cx.notify();
    }

    /// Settings → Library with the sign-in form open.
    pub(super) fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(SettingsPage::Library, window, cx);
        self.adding_server = true;
        self.form.error = None;
        window.focus(&self.form.url.focus_handle(cx));
        if self.form.found.is_empty() && !self.form.searching {
            self.discover(cx);
        }
    }

    fn close_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding_server = false;
        self.form.error = None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn remove_folder(&mut self, root: &std::path::Path, cx: &mut Context<Self>) {
        if let Err(e) = self.db.remove_root(root) {
            self.error = Some(format!("Couldn't remove {}: {e:#}", root.display()).into());
        }
        self.reload();
        cx.notify();
    }

    /// The whole Settings screen below the header.
    pub(super) fn settings_view(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let sidebar = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(200.0))
            .gap_1()
            .pt_6()
            .children(SettingsPage::ALL.into_iter().map(|page| {
                let active = page == self.settings_page;
                div()
                    .id(SharedString::from(format!("settings-tab-{}", page.label())))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(rgba(0xffffff14)).font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!active, |d| {
                        d.text_color(rgb(MUTED)).hover(|s| s.bg(rgba(0xffffff0a)))
                    })
                    .child(page.label())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.open_settings(page, window, cx)
                    }))
            }));
        let sections = match self.settings_page {
            SettingsPage::Library => self.library_settings(window, cx),
            SettingsPage::Playback => self.playback_settings(cx),
            SettingsPage::Controls => self.controls_settings(cx),
            SettingsPage::Appearance => self.appearance_settings(cx),
        };
        let mut page = div()
            .id(SharedString::from(format!(
                "settings-{}",
                self.settings_page.label()
            )))
            .flex_1()
            .min_w(px(0.0))
            .overflow_y_scroll()
            .pb_10()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // A width, not just a cap: with only `max_w`, sections
                    // were measured at the page's full width (descriptions
                    // on one line) but drawn at 760 (wrapped), so a section
                    // with wrapped rows ran into the next one.
                    .w(px(760.0))
                    .max_w_full()
                    // Taller than the window, the page scrolls; the
                    // sections mustn't shrink to fit it.
                    .children(
                        sections
                            .into_iter()
                            .map(|section| div().flex_none().child(section)),
                    ),
            );
        page.style().restrict_scroll_to_axis = Some(true);
        div()
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .gap_10()
            .px_8()
            .child(sidebar)
            .child(page)
            .into_any_element()
    }

    fn library_settings(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let servers: Vec<AnyElement> = self
            .servers
            .iter()
            .map(|server| {
                let id = server.id;
                let top = line(
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(server.name.clone()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(MUTED))
                                .truncate()
                                .child(format!("{} · {}", server.user_name, server.url)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .child(self.quality_picker(id, window, cx))
                        .child(self.button(
                            SharedString::from(format!("sign-out-{id}")),
                            "Sign Out",
                            cx,
                            move |this, _, cx| this.sign_out(id, cx),
                        )),
                );
                div()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .bg(rgba(0xffffff0a))
                    .child(top)
                    .child(self.server_apps(server, cx))
                    .into_any_element()
            })
            .collect();
        let servers_body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(note(
                "Movies and shows on these servers appear in your library. You can sign in to \
                 several servers, or to one server as different users. Each server can have \
                 its own Sonarr and Radarr, for requesting what it doesn't have yet.",
            ))
            .children(servers)
            .child(if self.adding_server {
                self.sign_in_form(cx)
            } else {
                div()
                    .flex()
                    .pt_1()
                    .child(
                        self.button("add-server", "Add Server", cx, |this, window, cx| {
                            this.open_connect(window, cx)
                        }),
                    )
                    .into_any_element()
            });

        let folders: Vec<AnyElement> = self
            .roots
            .iter()
            .enumerate()
            .map(|(i, root)| {
                let path = root.clone();
                row(div()
                    .min_w(px(0.0))
                    .truncate()
                    .child(root.display().to_string()))
                .child(self.button(
                    SharedString::from(format!("remove-folder-{i}")),
                    "Remove",
                    cx,
                    move |this, _, cx| this.remove_folder(&path, cx),
                ))
                .into_any_element()
            })
            .collect();
        let folders_body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(note(
                "Videos in these folders, and folders inside them, appear in your library. \
                 Removing a folder takes its videos and their progress out of the library; \
                 the files stay on disk.",
            ))
            .children(folders)
            .child(div().flex().pt_1().child(self.button(
                "add-folder",
                "Add Folder",
                cx,
                |this, _, cx| this.add_folder(cx),
            )));

        vec![
            Self::section("Jellyfin servers", servers_body),
            Self::section("Folders", folders_body),
        ]
    }

    fn sign_in_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let form = &self.form;
        let found: Vec<AnyElement> = form
            .found
            .iter()
            .enumerate()
            .map(|(i, server)| {
                let address = server.address.clone();
                self.button(
                    SharedString::from(format!("found-{i}")),
                    format!("{} · {}", server.name, server.address),
                    cx,
                    move |this, _, cx| {
                        let address = address.clone();
                        this.form
                            .url
                            .update(cx, |input, cx| input.set_text(address, cx));
                    },
                )
            })
            .collect();
        let network_note = if form.searching {
            Some("Looking for servers on your network…")
        } else if form.found.is_empty() {
            Some("No servers found on your network. Enter the address instead.")
        } else {
            None
        };
        div()
            .flex()
            .flex_col()
            .gap_4()
            .mt_2()
            .p_5()
            .rounded_lg()
            .bg(rgba(0xffffff0a))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Sign in to a server"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(network_note.map(note))
                    .child(div().flex().flex_wrap().gap_2().children(found)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .max_w(px(440.0))
                    .child(Self::field("Server address", &form.url))
                    .child(Self::field("Username", &form.user))
                    .child(Self::field("Password", &form.password)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(self.button(
                        "sign-in",
                        if form.busy {
                            "Signing in…"
                        } else {
                            "Sign In"
                        },
                        cx,
                        |this, window, cx| this.connect(window, cx),
                    ))
                    .child(
                        self.button("cancel-sign-in", "Cancel", cx, |this, window, cx| {
                            this.close_connect(window, cx)
                        }),
                    )
                    .children(
                        form.error
                            .clone()
                            .map(|e| div().text_sm().text_color(rgb(ERROR)).child(e)),
                    ),
            )
            .into_any_element()
    }

    fn playback_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let skip = self.media_skip_buttons();
        vec![
            Self::section("Sound", self.sound_entry(cx)),
            Self::section(
                "Headphones and media keys",
                self.switch_row(
                    "setting-media-skip",
                    "Next and previous switch episodes",
                    "When off, the skip buttons on headphones and keyboards do nothing in the \
                     player. Play and pause always work.",
                    skip,
                    cx,
                    move |this, _, cx| {
                        this.set_media_skip_buttons(!skip);
                        cx.notify();
                    },
                ),
            ),
        ]
    }
}

/// A muted explanation, for other Settings pages.
pub(super) fn note_text(text: &'static str) -> gpui::Div {
    note(text)
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

impl Shell {
    /// A setting that's on or off: the text on the left, a switch on the
    /// right; clicking anywhere on the row flips it.
    fn switch_row(
        &self,
        id: &'static str,
        title: &'static str,
        description: &'static str,
        on: bool,
        cx: &mut Context<Shell>,
        toggle: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_between()
            .gap_6()
            .px_4()
            .py_3()
            .rounded_lg()
            .bg(rgba(0xffffff0a))
            .hover(|s| s.bg(rgba(0xffffff14)))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_1()
                    .min_w(px(0.0))
                    .child(div().font_weight(FontWeight::MEDIUM).child(title))
                    .child(div().text_sm().text_color(rgb(MUTED)).child(description)),
            )
            .child(self.switch(id, on))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.revealed.borrow_mut().switch_flipped(id);
                toggle(this, window, cx);
            }))
            .into_any_element()
    }

    /// The switch itself. Flipped, its knob glides across as its colour
    /// changes; otherwise it's drawn at rest.
    pub(super) fn switch(&self, id: &'static str, on: bool) -> AnyElement {
        // 0: off, 1: on.
        let track = |p: f32| {
            let mix =
                |from: u32, to: u32| (from as f32 + (to as f32 - from as f32) * p).round() as u32;
            let color = (mix(0xff, ACCENT >> 16) << 24)
                | (mix(0xff, (ACCENT >> 8) & 0xff) << 16)
                | (mix(0xff, ACCENT & 0xff) << 8)
                | mix(0x2e, 0xff);
            div()
                .flex_none()
                .w(px(40.0))
                .h(px(22.0))
                .p(px(2.0))
                .rounded_full()
                .bg(rgba(color))
                .flex()
                .child(
                    div()
                        .size(px(18.0))
                        .ml(px(18.0 * p))
                        .rounded_full()
                        .bg(rgb(0xffffff)),
                )
        };
        let moving = crate::motion::enabled()
            .then(|| self.revealed.borrow().switch_moving(id))
            .flatten();
        let Some(flips) = moving else {
            return track(if on { 1.0 } else { 0.0 }).into_any_element();
        };
        div()
            .flex_none()
            .w(px(40.0))
            .h(px(22.0))
            .child(
                div().size_full().with_animation(
                    SharedString::from(format!("{id}-switch-{flips}")),
                    Animation::new(crate::motion::SWITCH.time())
                        .with_easing(crate::motion::ease_in_out),
                    move |d, t| {
                        let p = if on { t } else { 1.0 - t };
                        d.child(track(p))
                    },
                ),
            )
            .into_any_element()
    }
}
