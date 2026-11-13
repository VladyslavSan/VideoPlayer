//! Sonarr and Radarr connections: their addresses (library settings), API
//! keys (credential store) and the Settings page that edits them.

use std::collections::HashMap;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, Focusable, FontWeight, SharedString, Window, div,
    prelude::*, px, rgb,
};

use super::{ERROR, MUTED, Shell};
use crate::arr::{self, Client, Kind};
use crate::secrets;
use crate::ui::text_input::TextInput;

const CONNECTED: u32 = 0x7ad89a;

pub(super) struct ArrForm {
    url: Entity<TextInput>,
    key: Entity<TextInput>,
    /// The saved address and API key.
    saved: Option<(String, String)>,
    state: State,
}

enum State {
    Idle,
    Testing,
    /// "Sonarr 4.0.15".
    Connected(String),
    Failed(SharedString),
}

pub(super) fn new_forms(cx: &mut Context<Shell>) -> HashMap<Kind, ArrForm> {
    Kind::ALL
        .into_iter()
        .map(|kind| {
            let form = ArrForm {
                url: cx.new(|cx| {
                    TextInput::new(
                        format!("http://localhost:{}", kind.default_port()),
                        false,
                        cx,
                    )
                }),
                key: cx.new(|cx| TextInput::new("API key", true, cx)),
                saved: None,
                state: State::Idle,
            };
            (kind, form)
        })
        .collect()
}

/// Error text as shown on its own: starting with a capital.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Per library (like Jellyfin tokens), so a test library never reads or
/// replaces the real one's key.
fn key_account(kind: Kind, device_id: &str) -> String {
    format!("{}:{device_id}", kind.key_account())
}

impl Shell {
    /// Reads the saved connections and checks them in the background.
    pub(super) fn load_arr(&mut self, cx: &mut Context<Self>) {
        for kind in Kind::ALL {
            let url = self
                .db
                .setting(kind.url_setting())
                .ok()
                .flatten()
                .filter(|u| !u.is_empty());
            let key = match secrets::load(&key_account(kind, &self.device_id)) {
                Ok(key) => key,
                Err(e) => {
                    eprintln!("[{}] reading the API key: {e:#}", kind.name());
                    None
                }
            };
            let Some(form) = self.arr.get_mut(&kind) else {
                continue;
            };
            if let Some(url) = &url {
                form.url
                    .update(cx, |input, cx| input.set_text(url.clone(), cx));
            }
            form.saved = url.zip(key);
            if form.saved.is_some() {
                self.test_arr(kind, None, cx);
            }
        }
    }

    /// A client for a connected app, for the features that use it.
    #[allow(dead_code)]
    pub(super) fn arr_client(&self, kind: Kind) -> Option<Client> {
        let (url, key) = self.arr.get(&kind)?.saved.as_ref()?;
        Some(Client::new(kind, url, key))
    }

    /// Fills in an empty address with the Jellyfin server's host on the
    /// app's usual port.
    pub(super) fn suggest_arr_urls(&mut self, cx: &mut Context<Self>) {
        let Some(jellyfin) = self.servers.first().map(|s| s.url.clone()) else {
            return;
        };
        for kind in Kind::ALL {
            let Some(form) = self.arr.get(&kind) else {
                continue;
            };
            if form.saved.is_none()
                && form.url.read(cx).text().trim().is_empty()
                && let Some(guess) = arr::guess_url(kind, &jellyfin)
            {
                form.url.update(cx, |input, cx| input.set_text(guess, cx));
            }
        }
    }

    /// Checks the address and key in the form (or the saved ones), and
    /// saves them once they work. `save`: the address and key to save.
    fn test_arr(&mut self, kind: Kind, save: Option<(String, String)>, cx: &mut Context<Self>) {
        let Some(form) = self.arr.get_mut(&kind) else {
            return;
        };
        let Some((url, key)) = save.clone().or_else(|| form.saved.clone()) else {
            return;
        };
        form.state = State::Testing;
        cx.notify();
        let client = Client::new(kind, &url, &key);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.status() })
                .await;
            this.update(cx, |this, cx| {
                let account = key_account(kind, &this.device_id);
                let saving = match (&result, save) {
                    (Ok(_), Some((url, key))) => {
                        let stored = secrets::save(&account, &key)
                            .and_then(|()| this.db.set_setting(kind.url_setting(), &url));
                        match stored {
                            Ok(()) => Some((url, key)),
                            Err(e) => {
                                if let Some(form) = this.arr.get_mut(&kind) {
                                    form.state = State::Failed(
                                        format!("Couldn't save the connection: {e:#}").into(),
                                    );
                                }
                                cx.notify();
                                return;
                            }
                        }
                    }
                    _ => None,
                };
                let Some(form) = this.arr.get_mut(&kind) else {
                    return;
                };
                form.state = match result {
                    Ok(status) => {
                        State::Connected(format!("{} {}", status.app_name, status.version))
                    }
                    Err(e) => State::Failed(sentence(&format!("{e:#}")).into()),
                };
                if let Some(saved) = saving {
                    form.url
                        .update(cx, |input, cx| input.set_text(saved.0.clone(), cx));
                    form.key.update(cx, |input, cx| input.set_text("", cx));
                    form.saved = Some(saved);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "Save and Test": the form's address and key; an empty key field
    /// keeps the saved key.
    pub(super) fn save_arr(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let Some(form) = self.arr.get_mut(&kind) else {
            return;
        };
        let url = form.url.read(cx).text();
        let typed_key = form.key.read(cx).text().trim().to_string();
        let key = if typed_key.is_empty() {
            form.saved.as_ref().map(|(_, key)| key.clone())
        } else {
            Some(typed_key)
        };
        let problem = if url.trim().is_empty() {
            Some(format!("Enter {}'s address.", kind.name()))
        } else if key.is_none() {
            Some(format!(
                "Enter the API key from {} → Settings → General.",
                kind.name()
            ))
        } else {
            None
        };
        if let Some(problem) = problem {
            form.state = State::Failed(problem.into());
            cx.notify();
            return;
        }
        let url = arr::normalize_url(&url);
        self.test_arr(kind, Some((url, key.unwrap_or_default())), cx);
    }

    fn remove_arr(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let removed = secrets::delete(&key_account(kind, &self.device_id))
            .and_then(|()| self.db.set_setting(kind.url_setting(), ""));
        if let Some(form) = self.arr.get_mut(&kind) {
            form.saved = None;
            form.key.update(cx, |input, cx| input.set_text("", cx));
            form.state = match removed {
                Ok(()) => State::Idle,
                Err(e) => State::Failed(format!("Couldn't remove it: {e:#}").into()),
            };
        }
        cx.notify();
    }

    /// The app whose form has the keyboard focus.
    pub(super) fn focused_arr(&self, window: &Window, cx: &Context<Self>) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| {
            self.arr.get(kind).is_some_and(|form| {
                form.url.focus_handle(cx).is_focused(window)
                    || form.key.focus_handle(cx).is_focused(window)
            })
        })
    }

    /// Tab: address → key → the next app's address.
    pub(super) fn focus_next_arr_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        let fields: Vec<_> = Kind::ALL
            .into_iter()
            .filter_map(|kind| self.arr.get(&kind))
            .flat_map(|form| [form.url.clone(), form.key.clone()])
            .collect();
        let current = fields
            .iter()
            .position(|f| f.focus_handle(cx).is_focused(window));
        let next = current.map_or(0, |i| (i + 1) % fields.len());
        window.focus(&fields[next].focus_handle(cx));
    }

    pub(super) fn arr_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        Kind::ALL
            .into_iter()
            .filter_map(|kind| {
                Some(Self::section(
                    kind.name(),
                    self.arr_form(kind, self.arr.get(&kind)?, cx),
                ))
            })
            .collect()
    }

    fn arr_form(&self, kind: Kind, form: &ArrForm, cx: &mut Context<Self>) -> AnyElement {
        let purpose = match kind {
            Kind::Sonarr => "Finds and downloads TV shows.",
            Kind::Radarr => "Finds and downloads movies.",
        };
        let status = match &form.state {
            State::Idle if form.saved.is_some() => None,
            State::Idle => Some((SharedString::from("Not connected"), MUTED)),
            State::Testing => Some(("Checking…".into(), MUTED)),
            State::Connected(version) => {
                Some((format!("Connected to {version}").into(), CONNECTED))
            }
            State::Failed(e) => Some((e.clone(), ERROR)),
        };
        let testing = matches!(form.state, State::Testing);
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_sm().text_color(rgb(MUTED)).child(format!(
                "{purpose} The API key is in {} under Settings → General. The address \
                         can be a full URL, such as one behind a reverse proxy.",
                kind.name()
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .max_w(px(440.0))
                    .child(Self::field("Address", &form.url))
                    .child(Self::field(
                        if form.saved.is_some() {
                            "API key (saved; type a new one to replace it)"
                        } else {
                            "API key"
                        },
                        &form.key,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(self.button(
                        SharedString::from(format!("arr-save-{}", kind.name())),
                        if testing {
                            "Checking…"
                        } else {
                            "Save and Test"
                        },
                        cx,
                        move |this, _, cx| this.save_arr(kind, cx),
                    ))
                    .children(form.saved.is_some().then(|| {
                        self.button(
                            SharedString::from(format!("arr-remove-{}", kind.name())),
                            "Remove",
                            cx,
                            move |this, _, cx| this.remove_arr(kind, cx),
                        )
                    }))
                    .children(status.map(|(text, color)| {
                        div()
                            .min_w(px(0.0))
                            .text_sm()
                            .text_color(rgb(color))
                            .when(color == CONNECTED, |d| d.font_weight(FontWeight::MEDIUM))
                            .child(text)
                    })),
            )
            .into_any_element()
    }
}
