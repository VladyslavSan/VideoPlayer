//! Sonarr and Radarr, set up per Jellyfin server: they fill that server's
//! libraries, so a server's shows and movies can be requested through
//! them. Addresses live in the library settings, API keys in the
//! credential store, and both go when the server is signed out of.

use std::collections::HashMap;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, Focusable, FontWeight, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};

use super::{ERROR, MUTED, Shell};
use crate::arr::{self, Client, Kind};
use crate::jellyfin::Server;
use crate::secrets;
use crate::ui::text_input::TextInput;

const CONNECTED: u32 = 0x7ad89a;

/// One app for one server (by server row id).
pub(super) type ArrKey = (i64, Kind);
pub(super) type ArrForms = HashMap<ArrKey, ArrForm>;

pub(super) struct ArrForm {
    url: Entity<TextInput>,
    key: Entity<TextInput>,
    /// The saved address and API key.
    saved: Option<(String, String)>,
    state: State,
    /// The form is open (setting up or editing).
    editing: bool,
}

enum State {
    Idle,
    Testing,
    /// "Sonarr 4.0.15".
    Connected(String),
    Failed(SharedString),
}

/// Ties a setting or key to the server and user, as Jellyfin tokens are.
fn scope(server: &Server) -> String {
    format!("{}:{}", server.server_id, server.user_id)
}

fn url_setting(kind: Kind, server: &Server) -> String {
    format!("{}:{}", kind.url_setting(), scope(server))
}

/// Includes the library's device id, so a test library never reads or
/// replaces the real one's key.
fn key_account(kind: Kind, device_id: &str, server: &Server) -> String {
    format!("{}:{device_id}:{}", kind.key_account(), scope(server))
}

/// Error text as shown on its own: starting with a capital.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

impl Shell {
    fn server_row(&self, id: i64) -> Option<&Server> {
        self.servers.iter().find(|s| s.id == id)
    }

    /// Makes a form for each app of each signed-in server, loading what
    /// was saved (and checking it in the background); drops the forms of
    /// servers signed out of.
    pub(super) fn sync_arr(&mut self, cx: &mut Context<Self>) {
        self.arr
            .retain(|(server, _), _| self.servers.iter().any(|s| s.id == *server));
        self.migrate_arr();
        for server in self.servers.clone() {
            for kind in Kind::ALL {
                if self.arr.contains_key(&(server.id, kind)) {
                    continue;
                }
                let url = self
                    .db
                    .setting(&url_setting(kind, &server))
                    .ok()
                    .flatten()
                    .filter(|u| !u.is_empty());
                let key = match secrets::load(&key_account(kind, &self.device_id, &server)) {
                    Ok(key) => key,
                    Err(e) => {
                        eprintln!("[{}] reading the API key: {e:#}", kind.name());
                        None
                    }
                };
                let text = url
                    .clone()
                    .or_else(|| arr::guess_url(kind, &server.url))
                    .unwrap_or_default();
                let form = ArrForm {
                    url: cx.new(|cx| {
                        let mut input = TextInput::new(
                            format!("http://localhost:{}", kind.default_port()),
                            false,
                            cx,
                        );
                        input.set_text(text, cx);
                        input
                    }),
                    key: cx.new(|cx| TextInput::new("API key", true, cx)),
                    saved: url.zip(key),
                    state: State::Idle,
                    editing: false,
                };
                let saved = form.saved.is_some();
                self.arr.insert((server.id, kind), form);
                if saved {
                    self.test_arr((server.id, kind), None, cx);
                }
            }
        }
    }

    /// A build from before this was per server kept one Sonarr and one
    /// Radarr for the whole app; with a single server, they're its.
    fn migrate_arr(&mut self) {
        let [server] = self.servers.as_slice() else {
            return;
        };
        for kind in Kind::ALL {
            let Ok(Some(url)) = self.db.setting(kind.url_setting()) else {
                continue;
            };
            let old_account = format!("{}:{}", kind.key_account(), self.device_id);
            let moved = (|| {
                if let Some(key) = secrets::load(&old_account)?
                    && !url.is_empty()
                {
                    secrets::save(&key_account(kind, &self.device_id, server), &key)?;
                    self.db.set_setting(&url_setting(kind, server), &url)?;
                }
                secrets::delete(&old_account)?;
                self.db.delete_setting(kind.url_setting())
            })();
            if let Err(e) = moved {
                eprintln!("[{}] moving the old connection: {e:#}", kind.name());
            }
        }
    }

    /// Forgets a server's apps (when signing out of it).
    pub(super) fn forget_arr(&mut self, server: &Server) {
        for kind in Kind::ALL {
            let removed = secrets::delete(&key_account(kind, &self.device_id, server))
                .and_then(|()| self.db.delete_setting(&url_setting(kind, server)));
            if let Err(e) = removed {
                eprintln!("[{}] removing: {e:#}", kind.name());
            }
            self.arr.remove(&(server.id, kind));
        }
    }

    /// A client for a server's connected app, for the features that use it.
    pub(super) fn arr_client(&self, server: i64, kind: Kind) -> Option<Client> {
        let (url, key) = self.arr.get(&(server, kind))?.saved.as_ref()?;
        Some(Client::new(kind, url, key))
    }

    /// Checks an address and key (`save`, or the saved ones) and saves
    /// `save` once it works.
    fn test_arr(&mut self, id: ArrKey, save: Option<(String, String)>, cx: &mut Context<Self>) {
        let (server_id, kind) = id;
        let Some(form) = self.arr.get_mut(&id) else {
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
                let Some(server) = this.server_row(server_id).cloned() else {
                    return;
                };
                let mut state = match &result {
                    Ok(status) => {
                        State::Connected(format!("{} {}", status.app_name, status.version))
                    }
                    Err(e) => State::Failed(sentence(&format!("{e:#}")).into()),
                };
                let mut saved = None;
                if let (Ok(_), Some((url, key))) = (&result, save) {
                    let stored = secrets::save(&key_account(kind, &this.device_id, &server), &key)
                        .and_then(|()| this.db.set_setting(&url_setting(kind, &server), &url));
                    match stored {
                        Ok(()) => saved = Some((url, key)),
                        Err(e) => {
                            state =
                                State::Failed(format!("Couldn't save the connection: {e:#}").into())
                        }
                    }
                }
                let Some(form) = this.arr.get_mut(&id) else {
                    return;
                };
                form.state = state;
                if let Some(saved) = saved {
                    form.url
                        .update(cx, |input, cx| input.set_text(saved.0.clone(), cx));
                    form.key.update(cx, |input, cx| input.set_text("", cx));
                    form.saved = Some(saved);
                    form.editing = false;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "Save and Test": the form's address and key; an empty key field
    /// keeps the saved key.
    pub(super) fn save_arr(&mut self, id: ArrKey, cx: &mut Context<Self>) {
        let kind = id.1;
        let Some(form) = self.arr.get_mut(&id) else {
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
        self.test_arr(id, Some((url, key.unwrap_or_default())), cx);
    }

    fn remove_arr(&mut self, id: ArrKey, cx: &mut Context<Self>) {
        let (server_id, kind) = id;
        let Some(server) = self.server_row(server_id).cloned() else {
            return;
        };
        let removed = secrets::delete(&key_account(kind, &self.device_id, &server))
            .and_then(|()| self.db.delete_setting(&url_setting(kind, &server)));
        if let Some(form) = self.arr.get_mut(&id) {
            form.saved = None;
            form.editing = false;
            form.key.update(cx, |input, cx| input.set_text("", cx));
            form.state = match removed {
                Ok(()) => State::Idle,
                Err(e) => State::Failed(format!("Couldn't remove it: {e:#}").into()),
            };
        }
        cx.notify();
    }

    fn edit_arr(&mut self, id: ArrKey, editing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.arr.get_mut(&id) else {
            return;
        };
        form.editing = editing;
        if editing {
            // A fresh form starts at the key: the address is usually right.
            let field = if form.saved.is_some() {
                &form.url
            } else {
                &form.key
            };
            window.focus(&field.focus_handle(cx));
        } else {
            form.key.update(cx, |input, cx| input.set_text("", cx));
            if let Some((url, _)) = form.saved.clone() {
                form.url.update(cx, |input, cx| input.set_text(url, cx));
            }
            if matches!(form.state, State::Failed(_)) {
                form.state = State::Idle;
            }
            window.focus(&self.focus);
        }
        cx.notify();
    }

    /// The open form that has the keyboard focus.
    pub(super) fn focused_arr(&self, window: &Window, cx: &Context<Self>) -> Option<ArrKey> {
        self.arr.iter().find_map(|(id, form)| {
            (form.editing
                && (form.url.focus_handle(cx).is_focused(window)
                    || form.key.focus_handle(cx).is_focused(window)))
            .then_some(*id)
        })
    }

    /// Tab between the address and key of the focused form.
    pub(super) fn focus_next_arr_field(&self, id: ArrKey, window: &mut Window, cx: &Context<Self>) {
        let Some(form) = self.arr.get(&id) else {
            return;
        };
        let next = if form.url.focus_handle(cx).is_focused(window) {
            &form.key
        } else {
            &form.url
        };
        window.focus(&next.focus_handle(cx));
    }

    /// Under a server in Settings → Library: its Sonarr and Radarr.
    pub(super) fn server_apps(&self, server: &Server, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgba(0xffffff14))
            .children(Kind::ALL.into_iter().filter_map(|kind| {
                let form = self.arr.get(&(server.id, kind))?;
                Some(self.app_line((server.id, kind), form, cx))
            }))
            .into_any_element()
    }

    fn app_line(&self, id: ArrKey, form: &ArrForm, cx: &mut Context<Self>) -> AnyElement {
        let kind = id.1;
        let element_id =
            |what: &str| SharedString::from(format!("arr-{what}-{}-{}", id.0, kind.name()));
        let purpose = match kind {
            Kind::Sonarr => "request seasons and episodes",
            Kind::Radarr => "request movies",
        };
        let status = match &form.state {
            State::Testing => ("Checking…".into(), MUTED),
            State::Connected(version) => (
                SharedString::from(format!(
                    "Connected to {version} · {}",
                    form.saved.as_ref().map_or("", |(url, _)| url.as_str())
                )),
                CONNECTED,
            ),
            State::Failed(e) => (e.clone(), ERROR),
            State::Idle if form.saved.is_some() => (
                form.saved
                    .as_ref()
                    .map(|(url, _)| url.clone())
                    .unwrap_or_default()
                    .into(),
                MUTED,
            ),
            State::Idle => (
                format!("Optional: set it up to {purpose} here").into(),
                MUTED,
            ),
        };
        let summary = div()
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .w(px(64.0))
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(kind.name()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_sm()
                    .text_color(rgb(status.1))
                    .child(status.0),
            )
            .children((!form.editing).then(|| {
                self.button(
                    element_id("open"),
                    if form.saved.is_some() {
                        "Edit"
                    } else {
                        "Set Up"
                    },
                    cx,
                    move |this, window, cx| this.edit_arr(id, true, window, cx),
                )
            }));
        let testing = matches!(form.state, State::Testing);
        let editor = form.editing.then(|| {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .pl(px(80.0))
                .pt_3()
                .child(div().text_sm().text_color(rgb(MUTED)).child(format!(
                    "The API key is in {} under Settings → General. The address can be any \
                     full URL, such as one behind a reverse proxy.",
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
                            element_id("save"),
                            if testing {
                                "Checking…"
                            } else {
                                "Save and Test"
                            },
                            cx,
                            move |this, _, cx| this.save_arr(id, cx),
                        ))
                        .child(self.button(
                            element_id("cancel"),
                            "Cancel",
                            cx,
                            move |this, window, cx| this.edit_arr(id, false, window, cx),
                        ))
                        .children(form.saved.is_some().then(|| {
                            self.button(element_id("remove"), "Remove", cx, move |this, _, cx| {
                                this.remove_arr(id, cx)
                            })
                        })),
                )
        });
        div()
            .flex()
            .flex_col()
            .px_4()
            .py_3()
            .child(summary)
            .children(editor)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(sentence("that's Sonarr"), "That's Sonarr");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn keys_belong_to_a_server_and_user() {
        let server = Server {
            id: 1,
            url: "http://moto-moto-srv.local:8096".into(),
            name: "moto".into(),
            server_id: "srv".into(),
            user_id: "usr".into(),
            user_name: "me".into(),
            token: String::new(),
        };
        assert_eq!(url_setting(Kind::Sonarr, &server), "sonarr_url:srv:usr");
        assert_eq!(
            key_account(Kind::Radarr, "dev", &server),
            "radarr:dev:srv:usr"
        );
    }
}
