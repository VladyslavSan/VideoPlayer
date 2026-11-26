//! Adding to Sonarr and Radarr from the search field: a moment after typing
//! stops, each set-up app is asked for shows or movies by that name; the
//! search results end with them. Add… opens the choices (root folder,
//! quality profile, what to monitor, whether to search now), filled in the
//! way the library already files such things.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, ObjectFit, SharedString, div, img, prelude::*, px,
    rgb, rgba,
};

use super::{Pages, fnv, noun, subject};
use crate::arr::connections::{ConnectedApp, Id};
use crate::arr::{AddOptions, Candidate, Kind, Monitor, Setup};
use crate::shell::widgets::{self, chips};
use crate::shell::{ACCENT, ERROR, MUTED, TEXT};

/// Typing stops for this long before the apps are asked.
const SETTLE: Duration = Duration::from_millis(700);
/// Shortest name worth looking up.
const MIN_QUERY: usize = 3;
/// Results shown per app.
const SHOWN: usize = 8;
const POSTER_W: f32 = 72.0;
const POSTER_H: f32 = 108.0;
const ADDED: u32 = 0x7ad89a;

/// A search result: the app that found it, and its TVDB or TMDB id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ResultKey {
    pub app: Id,
    pub key: i64,
}

enum Lookup {
    Searching,
    Ready(Vec<Candidate>),
    Failed(SharedString),
}

enum SetupState {
    Loading,
    Ready(Arc<Setup>),
    Failed(SharedString),
}

/// The choices for the result being added.
struct AddForm {
    key: ResultKey,
    /// None until the app's folders are known.
    options: Option<AddOptions>,
    sending: bool,
    error: Option<SharedString>,
}

enum Poster {
    Loading,
    Ready(PathBuf),
    Failed,
}

#[derive(Default)]
pub(super) struct Finder {
    /// What was last typed (and is, or will be, looked up).
    query: String,
    /// The app it's looked up in (typed on its page); None for all.
    scope: Option<Id>,
    lookups: HashMap<Id, Lookup>,
    setups: HashMap<Id, SetupState>,
    form: Option<AddForm>,
    /// Added from here, with whether a search was started.
    added: HashMap<ResultKey, bool>,
    posters: HashMap<String, Poster>,
    /// Results whose page was opened, kept past the next search.
    opened: HashMap<ResultKey, Candidate>,
}

impl Finder {
    /// Takes what's typed now. False if it's what was typed before;
    /// otherwise the last results and the open form are gone.
    fn retype(&mut self, query: &str, scope: Option<Id>) -> bool {
        if query == self.query && scope == self.scope {
            return false;
        }
        self.query = query.to_string();
        self.scope = scope;
        self.lookups.clear();
        self.form = None;
        true
    }

    /// Whether `query`, typed for `scope`, is still what's being looked up.
    fn is_looking_for(&self, query: &str, scope: Option<Id>) -> bool {
        self.query == query && self.scope == scope
    }

    /// Whether the query is looked up in `app`: in every app, or only in
    /// the one whose page it was typed on.
    fn asks(&self, app: Id) -> bool {
        self.scope.is_none_or(|scope| scope == app)
    }
}

/// Whether a name is long enough to look up.
fn worth_looking_up(query: &str) -> bool {
    query.chars().count() >= MIN_QUERY
}

/// `5.1 TB free`.
fn free(bytes: u64) -> String {
    let tb = bytes as f64 / 1e12;
    if tb >= 1.0 {
        format!("{tb:.1} TB free")
    } else {
        format!("{:.0} GB free", bytes as f64 / 1e9)
    }
}

/// A line of text in a form or under a search.
fn note(text: String, color: u32) -> AnyElement {
    div()
        .text_sm()
        .text_color(rgb(color))
        .child(text)
        .into_any_element()
}

/// Why the folder picked in the form: what's in it, and whether it's
/// the one suggested for a result that is (or isn't) `anime`.
fn folder_note(setup: &Setup, root: usize, anime: bool) -> Option<String> {
    let habits = setup.habits.get(root)?;
    let noun = match setup.kind {
        Kind::Sonarr => "shows",
        Kind::Radarr => "movies",
    };
    let suggested = setup.suggest(anime);
    let (a, c) = (habits.anime, habits.count);
    let why = match (root == suggested && setup.roots.len() > 1, anime) {
        (true, true) => {
            format!("Suggested because it looks like anime, like {a} of the {c} {noun} here.")
        }
        (true, false) => {
            format!("Suggested because it doesn't look like anime; {a} of the {c} {noun} here are.")
        }
        (false, _) => format!("{a} of the {c} {noun} here are anime."),
    };
    let labels: Vec<String> = habits
        .tags
        .iter()
        .map(|t| format!("“{}”", setup.tag_label(*t).unwrap_or("?")))
        .collect();
    let tags = match labels.len() {
        0 => String::new(),
        1 => format!(" It's tagged {}, like most of them.", labels[0]),
        _ => format!(" It's tagged {}, like most of them.", labels.join(", ")),
    };
    Some(format!("{why}{tags}"))
}

/// The line of facts under a result's title.
fn candidate_facts(candidate: &Candidate) -> Vec<String> {
    let mut facts: Vec<String> = Vec::new();
    facts.extend(candidate.network.clone());
    if candidate.seasons > 0 {
        facts.push(if candidate.seasons == 1 {
            "1 season".into()
        } else {
            format!("{} seasons", candidate.seasons)
        });
    }
    if let Some(status) = &candidate.status {
        let mut status = status.clone();
        if let Some(first) = status.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        facts.push(status);
    }
    facts.extend(candidate.genres.iter().take(3).cloned());
    facts
}

/// A result's title, year, facts and the start of its overview.
fn candidate_text(candidate: &Candidate) -> gpui::Div {
    let facts = candidate_facts(candidate);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .min_w(px(0.0))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(candidate.title.clone()),
                )
                .children(
                    candidate
                        .year
                        .map(|y| div().text_sm().text_color(rgb(MUTED)).child(y.to_string())),
                )
                .when(candidate.is_anime(), |d| {
                    d.child(
                        div()
                            .px_1p5()
                            .rounded_sm()
                            .text_xs()
                            .bg(rgba(0xffffff1a))
                            .child("Anime"),
                    )
                }),
        )
        .when(!facts.is_empty(), |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(facts.join(" · ")),
            )
        })
        .children(candidate.overview.clone().map(|o| {
            div()
                .text_sm()
                .text_color(rgb(MUTED))
                .line_clamp(2)
                .child(o)
        }))
}

impl Pages {
    /// Looks up what's typed: the search field's `query` in every app,
    /// or, if that's empty on an app's page (`open`), the page's own field.
    pub(in crate::shell) fn search_changed(
        &mut self,
        query: String,
        open: Option<Id>,
        cx: &mut Context<Self>,
    ) {
        if query.is_empty()
            && let Some(app) = open
        {
            let own = self.name_field(app.kind).read(cx).text().trim().to_string();
            return self.find(own, Some(app), cx);
        }
        self.find(query, None, cx);
    }

    /// Looks `query` up in the apps (`scope`: in just that one) once typing
    /// settles.
    fn find(&mut self, query: String, scope: Option<Id>, cx: &mut Context<Self>) {
        if !self.finder.retype(&query, scope) {
            return;
        }
        if !worth_looking_up(&query) || self.apps.connected_ids().is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            this.update(cx, |this, cx| {
                if this.finder.is_looking_for(&query, scope) {
                    this.look_up(query, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn look_up(&mut self, query: String, cx: &mut Context<Self>) {
        let scope = self.finder.scope;
        for ConnectedApp { id: app, client } in self.apps.connected() {
            if !self.finder.asks(app) {
                continue;
            }
            self.finder.lookups.insert(app, Lookup::Searching);
            self.load_setup(app, cx);
            let query = query.clone();
            cx.spawn(async move |this, cx| {
                let term = query.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { client.lookup(&term) })
                    .await;
                this.update(cx, |this, cx| {
                    // Typed on since: no longer wanted.
                    if !this.finder.is_looking_for(&query, scope) {
                        return;
                    }
                    let kind = app.kind;
                    let lookup = match result {
                        Ok(found) => {
                            for poster in found.iter().take(SHOWN).filter_map(|c| c.poster.clone())
                            {
                                this.load_poster(poster, cx);
                            }
                            Lookup::Ready(found)
                        }
                        Err(e) => {
                            eprintln!("[{}] lookup: {e:#}", kind.name());
                            Lookup::Failed(format!("Couldn't search {}: {e:#}", kind.name()).into())
                        }
                    };
                    this.finder.lookups.insert(app, lookup);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Reads the app's folders and profiles once (again after a failure).
    fn load_setup(&mut self, app: Id, cx: &mut Context<Self>) {
        if matches!(
            self.finder.setups.get(&app),
            Some(SetupState::Loading | SetupState::Ready(_))
        ) {
            return;
        }
        let Some(client) = self.client(app) else {
            return;
        };
        self.finder.setups.insert(app, SetupState::Loading);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.setup() })
                .await;
            this.update(cx, |this, cx| {
                let name = app.kind.name();
                let state = match result {
                    Ok(setup) => SetupState::Ready(Arc::new(setup)),
                    Err(e) => {
                        eprintln!("[{name}] folders: {e:#}");
                        SetupState::Failed(format!("Couldn't read {name}'s folders: {e:#}").into())
                    }
                };
                this.finder.setups.insert(app, state);
                this.fill_form();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The picture at `url`, if it's here (`load_poster` fetches it).
    pub(super) fn poster_file(&self, url: &str) -> Option<PathBuf> {
        match self.finder.posters.get(url) {
            Some(Poster::Ready(path)) => Some(path.clone()),
            _ => None,
        }
    }

    /// Keeps a result whose page is opened.
    pub(super) fn keep_candidate(&mut self, key: ResultKey, candidate: Candidate) {
        self.finder.opened.insert(key, candidate);
    }

    pub(super) fn kept_candidate(&self, key: ResultKey) -> Option<&Candidate> {
        self.finder.opened.get(&key)
    }

    pub(super) fn load_poster(&mut self, url: String, cx: &mut Context<Self>) {
        if self.finder.posters.contains_key(&url) {
            return;
        }
        // Named by the address, so a changed poster is fetched anew.
        let name = format!("{:016x}.jpg", fnv(&url));
        let path = self.profile.paths.arr_pictures().join(name);
        if path.exists() {
            self.finder.posters.insert(url, Poster::Ready(path));
            return;
        }
        self.finder.posters.insert(url.clone(), Poster::Loading);
        cx.spawn(async move |this, cx| {
            let out = path.clone();
            let fetch = url.clone();
            let result = cx
                .background_executor()
                .spawn(async move { crate::arr::fetch_poster(&fetch, &out) })
                .await;
            this.update(cx, |this, cx| {
                let poster = match result {
                    Ok(()) => Poster::Ready(path),
                    Err(e) => {
                        eprintln!("[arr] poster: {e:#}");
                        Poster::Failed
                    }
                };
                this.finder.posters.insert(url, poster);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn candidate(&self, key: ResultKey) -> Option<&Candidate> {
        let found = match self.finder.lookups.get(&key.app) {
            Some(Lookup::Ready(found)) => found.iter().find(|c| c.key == key.key),
            _ => None,
        };
        found.or_else(|| self.finder.opened.get(&key))
    }

    fn setup(&self, app: Id) -> Option<Arc<Setup>> {
        match self.finder.setups.get(&app)? {
            SetupState::Ready(setup) => Some(setup.clone()),
            _ => None,
        }
    }

    /// Fills the open form's choices once the folders are known.
    fn fill_form(&mut self) {
        let Some(form) = &self.finder.form else {
            return;
        };
        if form.options.is_some() {
            return;
        }
        let key = form.key;
        let (Some(setup), Some(candidate)) = (self.setup(key.app), self.candidate(key)) else {
            return;
        };
        let root = setup.suggest(candidate.is_anime());
        let Some(profile) = setup.profile_for(root) else {
            return;
        };
        let options = AddOptions {
            root,
            profile,
            monitor: Monitor::All,
            search: true,
        };
        if let Some(form) = &mut self.finder.form {
            form.options = Some(options);
        }
    }

    fn open_form(&mut self, key: ResultKey, cx: &mut Context<Self>) {
        if self.finder.form.as_ref().is_some_and(|f| f.key == key) {
            self.finder.form = None;
        } else {
            self.finder.form = Some(AddForm {
                key,
                options: None,
                sending: false,
                error: None,
            });
            self.load_setup(key.app, cx);
            self.fill_form();
        }
        cx.notify();
    }

    fn change_options(&mut self, change: impl FnOnce(&mut AddOptions, &Setup)) {
        let Some(form) = &mut self.finder.form else {
            return;
        };
        let Some(setup) = (match self.finder.setups.get(&form.key.app) {
            Some(SetupState::Ready(setup)) => Some(setup.clone()),
            _ => None,
        }) else {
            return;
        };
        if let Some(options) = &mut form.options {
            change(options, &setup);
        }
    }

    fn send_add(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.finder.form else {
            return;
        };
        let key = form.key;
        let (Some(options), Some(setup), Some(candidate), Some(client)) = (
            form.options.clone(),
            self.setup(key.app),
            self.candidate(key).cloned(),
            self.client(key.app),
        ) else {
            return;
        };
        if let Some(form) = &mut self.finder.form {
            form.sending = true;
            form.error = None;
        }
        cx.notify();
        let kind = key.app.kind;
        let search = options.search && !(kind == Kind::Sonarr && options.monitor == Monitor::None);
        cx.spawn(async move |this, cx| {
            let title = candidate.title.clone();
            let result = cx
                .background_executor()
                .spawn(async move { client.add(&setup, &candidate, &options) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(id) => {
                        eprintln!("[{}] added {title} (search: {search})", kind.name());
                        this.finder.form = None;
                        this.finder.added.insert(key, search);
                        if let Some(Lookup::Ready(found)) = this.finder.lookups.get_mut(&key.app)
                            && let Some(c) = found.iter_mut().find(|c| c.key == key.key)
                        {
                            c.existing = Some(id);
                        }
                        // Its download shows up there soon, and it's on
                        // the app's page now.
                        this.refresh_downloads(cx);
                        this.load_list(key.app, true, cx);
                    }
                    Err(e) => {
                        eprintln!("[{}] adding {title}: {e:#}", kind.name());
                        if let Some(form) = &mut this.finder.form {
                            form.sending = false;
                            form.error = Some(format!("{} said no: {e:#}", kind.name()).into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The end of the search results: what the apps found.
    pub(in crate::shell) fn search_sections(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.finder.scope.is_some() || !worth_looking_up(&self.finder.query) {
            return Vec::new();
        }
        let apps = self.apps.connected_ids();
        let mut servers: Vec<i64> = apps.iter().map(|app| app.server).collect();
        servers.dedup();
        let mut sections = Vec::new();
        for app in apps {
            let Id { server, kind } = app;
            let body = self.finder_rows(app, cx);
            let title = if servers.len() > 1 {
                format!(
                    "Add a {} to {} ({})",
                    noun(kind),
                    self.server_name(server),
                    kind.name()
                )
            } else {
                format!("Add a {} ({})", noun(kind), kind.name())
            };
            sections.push(widgets::section(
                &title,
                div().flex().flex_col().gap_2().children(body),
            ));
        }
        sections
    }

    /// What one app found for the query: its results, or why there are none.
    pub(super) fn finder_rows(&self, app: Id, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let kind = app.kind;
        let query = &self.finder.query;
        match self.finder.lookups.get(&app) {
            None | Some(Lookup::Searching) => {
                vec![note(format!("Searching {}…", kind.name()), MUTED)]
            }
            Some(Lookup::Failed(e)) => vec![note(e.to_string(), ERROR)],
            Some(Lookup::Ready(found)) if found.is_empty() => vec![note(
                format!("{} found nothing called “{query}”.", kind.name()),
                MUTED,
            )],
            Some(Lookup::Ready(found)) => found
                .iter()
                .take(SHOWN)
                .map(|c| self.candidate_row(app, c, cx))
                .collect(),
        }
    }

    /// Whether the results are for this app's own field holding `query`.
    pub(super) fn finder_shows(&self, app: Id, query: &str) -> bool {
        self.finder.is_looking_for(query, Some(app)) && worth_looking_up(query)
    }

    fn candidate_row(&self, app: Id, candidate: &Candidate, cx: &mut Context<Self>) -> AnyElement {
        let kind = app.kind;
        let key = ResultKey {
            app,
            key: candidate.key,
        };
        let poster = candidate
            .poster
            .as_ref()
            .and_then(|url| self.poster_file(url));
        let action = self.candidate_action(app, candidate, cx);
        let open = candidate.clone();
        let summary = div()
            .flex()
            .gap_4()
            .child(
                // The poster and text open its page.
                div()
                    .id(SharedString::from(format!(
                        "result-{}-{}",
                        kind.name(),
                        candidate.key
                    )))
                    .flex()
                    .gap_4()
                    .flex_1()
                    .min_w(px(0.0))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.open_candidate(app, open.clone(), cx)
                    }))
                    .child(
                        div()
                            .flex_none()
                            .w(px(POSTER_W))
                            .h(px(POSTER_H))
                            .rounded_md()
                            .overflow_hidden()
                            .bg(rgba(0xffffff0d))
                            .children(
                                poster.map(|p| img(p).size_full().object_fit(ObjectFit::Cover)),
                            ),
                    )
                    .child(candidate_text(candidate)),
            )
            .child(div().flex_none().child(action));
        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_3()
            .rounded_lg()
            .bg(rgba(0xffffff08))
            .child(summary)
            .children(self.add_form_for(key, candidate, cx))
            .into_any_element()
    }

    /// The form for adding the result, if it's open.
    pub(super) fn add_form_for(
        &self,
        key: ResultKey,
        candidate: &Candidate,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.finder
            .form
            .as_ref()
            .filter(|f| f.key == key)
            .map(|form| self.add_form(form, candidate, cx))
    }

    /// What can be done with a result: Add…, or where it already is.
    pub(super) fn candidate_action(
        &self,
        app: Id,
        candidate: &Candidate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Id { server, kind } = app;
        let key = ResultKey {
            app,
            key: candidate.key,
        };
        let subject = subject(kind, &candidate.title, candidate.year);
        let in_library = candidate
            .existing
            .and_then(|_| self.library_target(server, &subject, cx));
        if let Some(target) = in_library {
            div()
                .id(SharedString::from(format!(
                    "open-{}-{}",
                    kind.name(),
                    candidate.key
                )))
                .flex_none()
                .text_sm()
                .text_color(rgb(MUTED))
                .cursor_pointer()
                .hover(|s| s.text_color(rgb(TEXT)))
                .child("In your library ›")
                .on_click(
                    cx.listener(move |this, _: &ClickEvent, _, cx| this.open_target(&target, cx)),
                )
                .into_any_element()
        } else if let Some(&searching) = self.finder.added.get(&key) {
            div()
                .flex_none()
                .text_sm()
                .text_color(rgb(ADDED))
                .child(if searching {
                    format!("Added ✓ · {} is searching", kind.name())
                } else {
                    "Added ✓".to_string()
                })
                .into_any_element()
        } else if candidate.existing.is_some() {
            div()
                .flex_none()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(format!("In {}", kind.name()))
                .into_any_element()
        } else {
            let open = self.finder.form.as_ref().is_some_and(|f| f.key == key);
            widgets::button(
                SharedString::from(format!("add-{}-{}", kind.name(), candidate.key)),
                if open { "Cancel" } else { "Add…" },
                cx,
                move |this, _, cx| this.open_form(key, cx),
            )
        }
    }

    fn add_form(
        &self,
        form: &AddForm,
        candidate: &Candidate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = form.key.app.kind;
        let frame = |children: Vec<AnyElement>| {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .pl(px(POSTER_W + 16.0))
                .children(children)
                .into_any_element()
        };
        let setup = match self.finder.setups.get(&form.key.app) {
            Some(SetupState::Ready(setup)) => setup.clone(),
            Some(SetupState::Failed(e)) => return frame(vec![note(e.to_string(), ERROR)]),
            _ => {
                return frame(vec![note(
                    format!("Reading {}'s folders…", kind.name()),
                    MUTED,
                )]);
            }
        };
        let Some(options) = &form.options else {
            return frame(vec![note(
                format!(
                    "{} has no root folder or quality profile set up.",
                    kind.name()
                ),
                ERROR,
            )]);
        };
        let mut children = self.add_choices(&setup, options, candidate.is_anime(), cx);
        if let Some(error) = &form.error {
            children.push(note(error.to_string(), ERROR));
        }
        let add: AnyElement = if form.sending {
            note("Adding…".into(), MUTED)
        } else {
            div()
                .id("add-confirm")
                .flex_none()
                .px_4()
                .py_1p5()
                .rounded_md()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .bg(rgb(ACCENT))
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .child(format!("Add to {}", kind.name()))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.send_add(cx)))
                .into_any_element()
        };
        children.push(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(add)
                .into_any_element(),
        );
        frame(children)
    }

    /// The form's choices: the folder (and why that one), the quality
    /// profile, what to monitor, and whether to search now.
    fn add_choices(
        &self,
        setup: &Setup,
        options: &AddOptions,
        anime: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let kind = setup.kind;
        let roots = setup
            .roots
            .iter()
            .map(|r| match r.free_space {
                Some(space) => format!("{} · {}", r.path, free(space)),
                None => r.path.clone(),
            })
            .enumerate()
            .map(|(i, label)| (label, i == options.root))
            .collect();
        let mut children = vec![chips(
            "add-root",
            "Folder",
            roots,
            cx,
            |this: &mut Self, i| {
                this.change_options(|o, setup| {
                    o.root = i;
                    if let Some(profile) = setup.profile_for(i) {
                        o.profile = profile;
                    }
                })
            },
        )];
        // Why this folder: what's in it.
        if let Some(why) = folder_note(setup, options.root, anime) {
            children.push(note(why, MUTED));
        }
        let profiles = setup
            .profiles
            .iter()
            .map(|p| (p.name.clone(), p.id == options.profile))
            .collect();
        let ids: Vec<i64> = setup.profiles.iter().map(|p| p.id).collect();
        children.push(chips(
            "add-profile",
            "Quality profile",
            profiles,
            cx,
            move |this: &mut Self, i| {
                let id = ids[i];
                this.change_options(|o, _| o.profile = id)
            },
        ));
        if kind == Kind::Sonarr {
            let monitors = Monitor::ALL
                .iter()
                .map(|m| (m.label().to_string(), *m == options.monitor))
                .collect();
            children.push(chips(
                "add-monitor",
                "Monitor",
                monitors,
                cx,
                |this: &mut Self, i| this.change_options(|o, _| o.monitor = Monitor::ALL[i]),
            ));
        }
        let can_search = !(kind == Kind::Sonarr && options.monitor == Monitor::None);
        if can_search {
            children.push(chips(
                "add-search",
                "Then",
                vec![
                    ("Start searching now".into(), options.search),
                    ("Just add it".into(), !options.search),
                ],
                cx,
                |this: &mut Self, i| this.change_options(|o, _| o.search = i == 0),
            ));
        }
        children
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::{Habits, RootFolder, Tag};

    const SONARR: Id = Id {
        server: 1,
        kind: Kind::Sonarr,
    };
    const RADARR: Id = Id {
        server: 1,
        kind: Kind::Radarr,
    };

    #[test]
    fn a_name_is_looked_up_from_three_letters() {
        assert!(!worth_looking_up(""));
        assert!(!worth_looking_up("da"));
        assert!(worth_looking_up("dar"));
        // Letters, not bytes.
        assert!(!worth_looking_up("éé"));
        assert!(worth_looking_up("ééé"));
    }

    #[test]
    fn typing_on_drops_the_last_results_and_the_open_form() {
        let mut finder = Finder::default();
        assert!(finder.retype("dark", None));
        finder.lookups.insert(SONARR, Lookup::Searching);
        finder.form = Some(AddForm {
            key: ResultKey {
                app: SONARR,
                key: 7,
            },
            options: None,
            sending: false,
            error: None,
        });
        // The same again (the field was only redrawn): nothing changes.
        assert!(!finder.retype("dark", None));
        assert_eq!(finder.lookups.len(), 1);
        assert!(finder.retype("dark ", None));
        assert!(finder.lookups.is_empty());
        assert!(finder.form.is_none());
    }

    #[test]
    fn the_same_name_typed_on_an_app_s_page_is_another_search() {
        let mut finder = Finder::default();
        assert!(finder.retype("dark", None));
        assert!(finder.retype("dark", Some(SONARR)));
        assert!(finder.is_looking_for("dark", Some(SONARR)));
        assert!(!finder.is_looking_for("dark", None));
        assert!(!finder.is_looking_for("darker", Some(SONARR)));
    }

    #[test]
    fn a_name_from_the_search_field_is_asked_of_every_app() {
        let mut finder = Finder::default();
        finder.retype("dark", None);
        assert!(finder.asks(SONARR) && finder.asks(RADARR));
        finder.retype("dark", Some(RADARR));
        assert!(!finder.asks(SONARR));
        assert!(finder.asks(RADARR));
    }

    #[test]
    fn free_space_reads_in_terabytes_or_gigabytes() {
        assert_eq!(free(5_100_000_000_000), "5.1 TB free");
        assert_eq!(free(250_000_000_000), "250 GB free");
    }

    fn setup(habits: Vec<Habits>) -> Setup {
        let roots = habits.iter().enumerate().map(|(i, _)| RootFolder {
            path: format!("/tv{i}"),
            free_space: None,
        });
        Setup {
            kind: Kind::Sonarr,
            roots: roots.collect(),
            profiles: Vec::new(),
            tags: vec![Tag {
                id: 4,
                label: "anime".into(),
            }],
            habits,
        }
    }

    #[test]
    fn the_folder_note_says_what_is_in_the_folder() {
        let habits = Habits {
            count: 40,
            anime: 2,
            ..Habits::default()
        };
        let setup = setup(vec![habits]);
        // The only folder is never "suggested".
        assert_eq!(
            folder_note(&setup, 0, false).as_deref(),
            Some("2 of the 40 shows here are anime.")
        );
        assert_eq!(folder_note(&setup, 3, false), None);
    }

    #[test]
    fn the_folder_note_names_the_tags_most_of_the_folder_has() {
        let habits = Habits {
            count: 12,
            anime: 12,
            tags: vec![4, 9],
            ..Habits::default()
        };
        let note = folder_note(&setup(vec![habits]), 0, true);
        assert_eq!(
            note.as_deref(),
            Some("12 of the 12 shows here are anime. It's tagged “anime”, “?”, like most of them.")
        );
    }
}
