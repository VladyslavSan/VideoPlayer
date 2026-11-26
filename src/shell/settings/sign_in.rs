//! Settings → Library → Add Server: the Jellyfin sign-in form. It looks for
//! servers on the network when it opens, signs in off the UI thread, and
//! hands the new server to the library.

use std::time::Duration;

use anyhow::{Result, bail};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, Focusable, FontWeight, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};

use super::{Event, Page, Settings, note};
use crate::jellyfin::{self, Client, Server, discovery};
use crate::secrets;
use crate::shell::{ERROR, widgets};
use crate::ui::text_input::TextInput;

/// How long servers on the network get to answer.
const DISCOVERY_WAIT: Duration = Duration::from_millis(1500);

/// The sign-in form: its fields, and how looking and signing in are going.
pub(super) struct SignIn {
    url: Entity<TextInput>,
    user: Entity<TextInput>,
    password: Entity<TextInput>,
    /// The form is showing.
    open: bool,
    /// Signing in.
    busy: bool,
    /// Looking for servers on the network.
    searching: bool,
    found: Vec<discovery::Found>,
    error: Option<SharedString>,
}

impl SignIn {
    pub(super) fn new(cx: &mut Context<Settings>) -> Self {
        Self {
            url: cx.new(|cx| TextInput::new("http://192.168.1.10:8096", false, cx)),
            user: cx.new(|cx| TextInput::new("Username", false, cx)),
            password: cx.new(|cx| TextInput::new("Password", true, cx)),
            open: false,
            busy: false,
            searching: false,
            found: Vec::new(),
            error: None,
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }
}

/// What to say about the search for servers, if anything.
fn network_note(searching: bool, found: &[discovery::Found]) -> Option<&'static str> {
    if searching {
        Some("Looking for servers on your network…")
    } else if found.is_empty() {
        Some("No servers found on your network. Enter the address instead.")
    } else {
        None
    }
}

/// Signs in to the server at `url` and keeps the token in the credential store.
fn sign_in(url: &str, user: &str, password: &str, device_id: &str) -> Result<Server> {
    let mut client = Client::new(url, device_id);
    let info = client.public_info()?;
    let auth = client.login(user, password)?;
    if auth.server_id != info.id {
        bail!("the server answered with an unexpected id");
    }
    let server = Server {
        id: 0,
        url: client.base().to_string(),
        name: info.server_name,
        server_id: auth.server_id,
        user_id: auth.user.id,
        user_name: auth.user.name,
        token: auth.access_token,
    };
    secrets::save(&server.token_account(device_id), &server.token)?;
    Ok(server)
}

impl Settings {
    /// Settings → Library with the sign-in form open.
    pub(in crate::shell) fn open_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page = Page::Library;
        self.opened(window, cx);
        self.sign_in.open = true;
        self.sign_in.error = None;
        window.focus(&self.sign_in.url.focus_handle(cx));
        if self.sign_in.found.is_empty() && !self.sign_in.searching {
            self.discover(cx);
        }
    }

    pub(super) fn close_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sign_in.open = false;
        self.sign_in.error = None;
        window.focus(&self.page_focus);
        cx.notify();
    }

    /// Looks for servers on the network; the first found fills an empty
    /// address field.
    fn discover(&mut self, cx: &mut Context<Self>) {
        self.sign_in.searching = true;
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { discovery::discover(DISCOVERY_WAIT) })
                .await;
            this.update(cx, |this, cx| this.discovered(found, cx)).ok();
        })
        .detach();
    }

    fn discovered(&mut self, found: Vec<discovery::Found>, cx: &mut Context<Self>) {
        self.sign_in.searching = false;
        let url_empty = self.sign_in.url.read(cx).text().is_empty();
        if url_empty && let Some(first) = found.first() {
            self.use_address(first.address.clone(), cx);
        }
        self.sign_in.found = found;
        cx.notify();
    }

    fn use_address(&mut self, address: String, cx: &mut Context<Self>) {
        self.sign_in
            .url
            .update(cx, |input, cx| input.set_text(address, cx));
    }

    /// "Sign In": signs in with what the form holds.
    pub(super) fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sign_in.busy {
            return;
        }
        let url = jellyfin::normalize_url(&self.sign_in.url.read(cx).text());
        let user = self.sign_in.user.read(cx).text().trim().to_string();
        let password = self.sign_in.password.read(cx).text();
        if url == "http://" || user.is_empty() {
            self.sign_in.error = Some("Enter the server address and your username.".into());
            cx.notify();
            return;
        }
        self.sign_in.busy = true;
        self.sign_in.error = None;
        let device_id = self.profile.device_id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let signed_in = cx
                .background_executor()
                .spawn(async move { sign_in(&url, &user, &password, &device_id) })
                .await;
            this.update_in(cx, |this, window, cx| this.signed_in(signed_in, window, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Signing in ended: a server that answered joins the library and the
    /// form closes; otherwise the form says what went wrong.
    fn signed_in(
        &mut self,
        signed_in: Result<Server>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sign_in.busy = false;
        // A first library goes straight to Home; another server shows up
        // in the list of servers.
        let first = !self.catalog.has_sources();
        let added = signed_in.and_then(|server| {
            self.update_library(cx, |library, cx| library.add_server(&server, cx))
        });
        match added {
            Ok(()) => {
                self.sign_in
                    .password
                    .update(cx, |input, cx| input.set_text("", cx));
                self.sign_in.open = false;
                if first {
                    cx.emit(Event::FirstLibraryAdded);
                }
                window.focus(&self.page_focus);
            }
            Err(e) => self.sign_in.error = Some(format!("{e:#}").into()),
        }
        cx.notify();
    }

    /// Tab moves between the form's fields, round and round.
    pub(super) fn focus_next_sign_in_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = &self.sign_in;
        let fields = [&form.url, &form.user, &form.password];
        let focused = fields
            .iter()
            .position(|field| field.focus_handle(cx).is_focused(window));
        let next = focused.map_or(0, |index| (index + 1) % fields.len());
        window.focus(&fields[next].focus_handle(cx));
    }

    /// The form, under the list of servers.
    pub(super) fn sign_in_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let form = &self.sign_in;
        let fields = div()
            .flex()
            .flex_col()
            .gap_4()
            .max_w(px(440.0))
            .child(widgets::field("Server address", &form.url))
            .child(widgets::field("Username", &form.user))
            .child(widgets::field("Password", &form.password));
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
            .child(self.found_servers(cx))
            .child(fields)
            .child(self.sign_in_buttons(cx))
            .into_any_element()
    }

    /// The servers found on the network, each a button that fills in its address.
    fn found_servers(&self, cx: &mut Context<Self>) -> gpui::Div {
        let form = &self.sign_in;
        let found = form.found.iter().enumerate().map(|(index, server)| {
            let address = server.address.clone();
            widgets::button(
                SharedString::from(format!("found-{index}")),
                format!("{} · {}", server.name, server.address),
                cx,
                move |this: &mut Self, _, cx| this.use_address(address.clone(), cx),
            )
        });
        let found: Vec<AnyElement> = found.collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(network_note(form.searching, &form.found).map(note))
            .child(div().flex().flex_wrap().gap_2().children(found))
    }

    fn sign_in_buttons(&self, cx: &mut Context<Self>) -> gpui::Div {
        let form = &self.sign_in;
        let sign_in_label = if form.busy {
            "Signing in…"
        } else {
            "Sign In"
        };
        let error = form
            .error
            .clone()
            .map(|error| div().text_sm().text_color(rgb(ERROR)).child(error));
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(widgets::button(
                "sign-in",
                sign_in_label,
                cx,
                |this: &mut Self, window, cx| this.connect(window, cx),
            ))
            .child(widgets::button(
                "cancel-sign-in",
                "Cancel",
                cx,
                |this: &mut Self, window, cx| this.close_sign_in(window, cx),
            ))
            .children(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str) -> discovery::Found {
        discovery::Found {
            address: format!("http://{name}:8096"),
            id: name.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn the_form_says_how_the_search_for_servers_is_going() {
        assert_eq!(
            network_note(true, &[]),
            Some("Looking for servers on your network…")
        );
        assert_eq!(
            network_note(false, &[]),
            Some("No servers found on your network. Enter the address instead.")
        );
        assert_eq!(network_note(false, &[found("moto")]), None);
    }
}
