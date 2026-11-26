//! Settings → Library, under each server: its Sonarr and Radarr, each a
//! line saying how its connection is and a form to set it up or change it.
//! The connections themselves are `arr::connections`; the forms ask it to
//! check, save and remove.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, Focusable, FontWeight, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};

use super::Settings;
use crate::arr::Kind;
use crate::arr::connections::{Connection, Id, Status};
use crate::jellyfin::Server;
use crate::shell::{ERROR, MUTED, widgets};
use crate::ui::text_input::TextInput;

const CONNECTED: u32 = 0x7ad89a;

/// The form of one server's app: its fields, and whether it is open.
pub(super) struct AppForm {
    url: Entity<TextInput>,
    key: Entity<TextInput>,
    /// Open, to set the app up or change it.
    editing: bool,
}

/// What an app's line says about its connection, and in which colour.
#[derive(Debug, PartialEq)]
struct StatusLine {
    text: SharedString,
    color: u32,
}

/// The line for an app saved at `address` (if it is set up): how its check
/// went, else where it is, else what setting it up would give.
fn status_line(kind: Kind, status: &Status, address: Option<&str>) -> StatusLine {
    let (text, color) = match (status, address) {
        (Status::Testing, _) => ("Checking…".into(), MUTED),
        (Status::Connected { version }, address) => (
            format!("Connected to {version} · {}", address.unwrap_or_default()),
            CONNECTED,
        ),
        (Status::Failed { problem }, _) => (problem.to_string(), ERROR),
        (Status::Idle, Some(address)) => (address.to_string(), MUTED),
        (Status::Idle, None) => {
            let purpose = match kind {
                Kind::Sonarr => "request seasons and episodes",
                Kind::Radarr => "request movies",
            };
            (format!("Optional: set it up to {purpose} here"), MUTED)
        }
    };
    StatusLine {
        text: text.into(),
        color,
    }
}

fn element_id(what: &str, id: Id) -> SharedString {
    format!("arr-{what}-{}-{}", id.server, id.kind.name()).into()
}

impl Settings {
    /// Makes a form for each app the connections know of, starting at its
    /// saved (or likely) address, and drops the forms of the others.
    pub(super) fn sync_app_forms(&mut self, cx: &mut Context<Self>) {
        self.app_forms
            .retain(|id, _| self.apps.connection(*id).is_some());
        for id in self.apps.ids() {
            if self.app_forms.contains_key(&id) {
                continue;
            }
            let address = self
                .connections
                .read(cx)
                .address(id, cx)
                .unwrap_or_default();
            let placeholder = format!("http://localhost:{}", id.kind.default_port());
            let url = cx.new(|cx| {
                let mut input = TextInput::new(placeholder, false, cx);
                input.set_text(address, cx);
                input
            });
            let key = cx.new(|cx| TextInput::new("API key", true, cx));
            let form = AppForm {
                url,
                key,
                editing: false,
            };
            self.app_forms.insert(id, form);
        }
    }

    /// "Save and Test": asks for the form's address and key to be checked
    /// and saved.
    pub(super) fn save_app(&mut self, id: Id, cx: &mut Context<Self>) {
        let Some(form) = self.app_forms.get(&id) else {
            return;
        };
        let address = form.url.read(cx).text();
        let typed_key = form.key.read(cx).text();
        self.connections.update(cx, |connections, cx| {
            connections.save(id, &address, &typed_key, cx)
        });
    }

    /// The connection was checked and saved: its form closes, showing the
    /// address as saved.
    pub(super) fn app_saved(&mut self, id: Id, cx: &mut Context<Self>) {
        let saved = self.apps.connection(id).and_then(|c| c.saved.clone());
        let (Some(form), Some(saved)) = (self.app_forms.get_mut(&id), saved) else {
            return;
        };
        form.url
            .update(cx, |input, cx| input.set_text(saved.url, cx));
        form.key.update(cx, |input, cx| input.set_text("", cx));
        form.editing = false;
        cx.notify();
    }

    fn remove_app(&mut self, id: Id, cx: &mut Context<Self>) {
        if self.catalog.server(id.server).is_none() {
            return;
        }
        self.connections
            .update(cx, |connections, cx| connections.remove(id, cx));
        if let Some(form) = self.app_forms.get_mut(&id) {
            form.editing = false;
            form.key.update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }

    fn open_app_form(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let is_set_up = self.is_set_up(id);
        let Some(form) = self.app_forms.get_mut(&id) else {
            return;
        };
        form.editing = true;
        // A fresh form starts at the key: the address is usually right.
        let field = if is_set_up { &form.url } else { &form.key };
        window.focus(&field.focus_handle(cx));
        cx.notify();
    }

    /// Closes the form, putting back the saved address.
    fn close_app_form(&mut self, id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.apps.connection(id).and_then(|c| c.saved.clone());
        let Some(form) = self.app_forms.get_mut(&id) else {
            return;
        };
        form.editing = false;
        form.key.update(cx, |input, cx| input.set_text("", cx));
        if let Some(saved) = saved {
            form.url
                .update(cx, |input, cx| input.set_text(saved.url, cx));
        }
        self.connections
            .update(cx, |connections, cx| connections.clear_problem(id, cx));
        window.focus(&self.page_focus);
        cx.notify();
    }

    fn is_set_up(&self, id: Id) -> bool {
        self.apps
            .connection(id)
            .is_some_and(|connection| connection.saved.is_some())
    }

    /// The open form that has the keyboard focus.
    pub(super) fn focused_app_form(&self, window: &Window, cx: &Context<Self>) -> Option<Id> {
        self.app_forms.iter().find_map(|(id, form)| {
            let focused = form.url.focus_handle(cx).is_focused(window)
                || form.key.focus_handle(cx).is_focused(window);
            (form.editing && focused).then_some(*id)
        })
    }

    /// Tab between the address and key of the focused form.
    pub(super) fn focus_next_app_field(&self, id: Id, window: &mut Window, cx: &Context<Self>) {
        let Some(form) = self.app_forms.get(&id) else {
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
        let lines = Kind::ALL.into_iter().filter_map(|kind| {
            let id = Id {
                server: server.id,
                kind,
            };
            let form = self.app_forms.get(&id)?;
            let connection = self.apps.connection(id)?;
            Some(self.app_line(id, form, connection, cx))
        });
        div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgba(0xffffff14))
            .children(lines)
            .into_any_element()
    }

    fn app_line(
        &self,
        id: Id,
        form: &AppForm,
        connection: &Connection,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = form
            .editing
            .then(|| self.app_editor(id, form, connection, cx));
        div()
            .flex()
            .flex_col()
            .px_4()
            .py_3()
            .child(self.app_summary(id, form, connection, cx))
            .children(editor)
            .into_any_element()
    }

    /// The app's name, how its connection is, and the button opening its form.
    fn app_summary(
        &self,
        id: Id,
        form: &AppForm,
        connection: &Connection,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let address = connection.saved.as_ref().map(|saved| saved.url.as_str());
        let status = status_line(id.kind, &connection.status, address);
        let open_label = if connection.saved.is_some() {
            "Edit"
        } else {
            "Set Up"
        };
        let open = (!form.editing).then(|| {
            widgets::button(
                element_id("open", id),
                open_label,
                cx,
                move |this: &mut Self, window, cx| this.open_app_form(id, window, cx),
            )
        });
        div()
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .w(px(64.0))
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(id.kind.name()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_sm()
                    .text_color(rgb(status.color))
                    .child(status.text),
            )
            .children(open)
    }

    /// The open form: what to enter, the two fields, and its buttons.
    fn app_editor(
        &self,
        id: Id,
        form: &AppForm,
        connection: &Connection,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let is_set_up = connection.saved.is_some();
        let key_label = if is_set_up {
            "API key (saved; type a new one to replace it)"
        } else {
            "API key"
        };
        let explanation = format!(
            "The API key is in {} under Settings → General. The address can be any \
             full URL, such as one behind a reverse proxy.",
            id.kind.name()
        );
        div()
            .flex()
            .flex_col()
            .gap_4()
            .pl(px(80.0))
            .pt_3()
            .child(div().text_sm().text_color(rgb(MUTED)).child(explanation))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .max_w(px(440.0))
                    .child(widgets::field("Address", &form.url))
                    .child(widgets::field(key_label, &form.key)),
            )
            .child(self.app_editor_buttons(id, connection, cx))
    }

    fn app_editor_buttons(
        &self,
        id: Id,
        connection: &Connection,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let save_label = if connection.status == Status::Testing {
            "Checking…"
        } else {
            "Save and Test"
        };
        let remove = connection.saved.is_some().then(|| {
            widgets::button(
                element_id("remove", id),
                "Remove",
                cx,
                move |this: &mut Self, _, cx| this.remove_app(id, cx),
            )
        });
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(widgets::button(
                element_id("save", id),
                save_label,
                cx,
                move |this: &mut Self, _, cx| this.save_app(id, cx),
            ))
            .child(widgets::button(
                element_id("cancel", id),
                "Cancel",
                cx,
                move |this: &mut Self, window, cx| this.close_app_form(id, window, cx),
            ))
            .children(remove)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: Kind, status: Status, address: Option<&str>) -> StatusLine {
        status_line(kind, &status, address)
    }

    #[test]
    fn an_app_not_set_up_says_what_it_is_for() {
        assert_eq!(
            line(Kind::Radarr, Status::Idle, None),
            StatusLine {
                text: "Optional: set it up to request movies here".into(),
                color: MUTED
            }
        );
        assert_eq!(
            line(Kind::Sonarr, Status::Idle, None).text,
            "Optional: set it up to request seasons and episodes here"
        );
    }

    #[test]
    fn a_set_up_app_shows_its_address_until_it_answers() {
        let address = Some("http://one:8989");
        assert_eq!(
            line(Kind::Sonarr, Status::Idle, address).text,
            "http://one:8989"
        );
        assert_eq!(
            line(Kind::Sonarr, Status::Testing, address).text,
            "Checking…"
        );
        let version = "Sonarr 4.0.15".to_string();
        assert_eq!(
            line(Kind::Sonarr, Status::Connected { version }, address),
            StatusLine {
                text: "Connected to Sonarr 4.0.15 · http://one:8989".into(),
                color: CONNECTED
            }
        );
    }

    #[test]
    fn a_failed_check_shows_what_went_wrong() {
        let problem = "Couldn't reach http://one:8989".into();
        assert_eq!(
            line(Kind::Sonarr, Status::Failed { problem }, None),
            StatusLine {
                text: "Couldn't reach http://one:8989".into(),
                color: ERROR
            }
        );
    }
}
