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

use super::{ACCENT, ERROR, MUTED, Shell, TEXT};
use crate::arr::{AddOptions, Candidate, Kind, Monitor, Setup, Subject};

/// Typing stops for this long before the apps are asked.
const SETTLE: Duration = Duration::from_millis(700);
/// Shortest name worth looking up.
const MIN_QUERY: usize = 3;
/// Results shown per app.
const SHOWN: usize = 8;
const POSTER_W: f32 = 72.0;
const POSTER_H: f32 = 108.0;
const ADDED: u32 = 0x7ad89a;

type AppKey = (i64, Kind);
type ResultKey = (i64, Kind, i64);

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
    lookups: HashMap<AppKey, Lookup>,
    setups: HashMap<AppKey, SetupState>,
    form: Option<AddForm>,
    /// Added from here, with whether a search was started.
    added: HashMap<ResultKey, bool>,
    posters: HashMap<String, Poster>,
}

fn kind_noun(kind: Kind) -> &'static str {
    match kind {
        Kind::Sonarr => "show",
        Kind::Radarr => "movie",
    }
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

/// A row of choices, one picked.
fn chips(
    id: &str,
    title: &str,
    options: Vec<(String, bool)>,
    cx: &mut Context<Shell>,
    pick: impl Fn(&mut Shell, usize) + Clone + 'static,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(title.to_string()),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .children(options.into_iter().enumerate().map(|(i, (label, on))| {
                    let pick = pick.clone();
                    div()
                        .id(SharedString::from(format!("{id}-{i}")))
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
                        .bg(if on { rgb(ACCENT) } else { rgba(0xffffff12) })
                        .when(!on, |d| d.hover(|s| s.bg(rgba(0xffffff26))))
                        .child(label)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            pick(this, i);
                            cx.notify();
                        }))
                }))
                .into_any_element(),
        )
        .into_any_element()
}

impl Shell {
    /// After each change to the search field: looks the name up in the
    /// apps once typing settles.
    pub(super) fn search_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.search_query(cx);
        if query == self.finder.query {
            return;
        }
        self.finder.query = query.clone();
        self.finder.lookups.clear();
        self.finder.form = None;
        if query.chars().count() < MIN_QUERY || self.arr_apps().is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            this.update(cx, |this, cx| {
                if this.finder.query == query {
                    this.look_up(query, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn look_up(&mut self, query: String, cx: &mut Context<Self>) {
        for (server, kind, client) in self.arr_apps() {
            let key = (server, kind);
            self.finder.lookups.insert(key, Lookup::Searching);
            self.load_setup(key, cx);
            let query = query.clone();
            cx.spawn(async move |this, cx| {
                let term = query.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { client.lookup(&term) })
                    .await;
                this.update(cx, |this, cx| {
                    // Typed on since: no longer wanted.
                    if this.finder.query != query {
                        return;
                    }
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
                    this.finder.lookups.insert(key, lookup);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Reads the app's folders and profiles once (again after a failure).
    fn load_setup(&mut self, key: AppKey, cx: &mut Context<Self>) {
        if matches!(
            self.finder.setups.get(&key),
            Some(SetupState::Loading | SetupState::Ready(_))
        ) {
            return;
        }
        let Some(client) = self.arr_client(key.0, key.1) else {
            return;
        };
        self.finder.setups.insert(key, SetupState::Loading);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { client.setup() })
                .await;
            this.update(cx, |this, cx| {
                let state = match result {
                    Ok(setup) => SetupState::Ready(Arc::new(setup)),
                    Err(e) => {
                        eprintln!("[{}] folders: {e:#}", key.1.name());
                        SetupState::Failed(
                            format!("Couldn't read {}'s folders: {e:#}", key.1.name()).into(),
                        )
                    }
                };
                this.finder.setups.insert(key, state);
                this.fill_form();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_poster(&mut self, url: String, cx: &mut Context<Self>) {
        if self.finder.posters.contains_key(&url) {
            return;
        }
        // Named by the address, so a changed poster is fetched anew.
        let name = format!("{:016x}.jpg", fnv(&url));
        let path = self.paths.thumbs.join("arr").join(name);
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
        match self.finder.lookups.get(&(key.0, key.1))? {
            Lookup::Ready(found) => found.iter().find(|c| c.key == key.2),
            _ => None,
        }
    }

    fn setup(&self, key: AppKey) -> Option<Arc<Setup>> {
        match self.finder.setups.get(&key)? {
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
        let (Some(setup), Some(candidate)) = (self.setup((key.0, key.1)), self.candidate(key))
        else {
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
            self.load_setup((key.0, key.1), cx);
            self.fill_form();
        }
        cx.notify();
    }

    fn change_options(&mut self, change: impl FnOnce(&mut AddOptions, &Setup)) {
        let Some(form) = &mut self.finder.form else {
            return;
        };
        let Some(setup) = (match self.finder.setups.get(&(form.key.0, form.key.1)) {
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
            self.setup((key.0, key.1)),
            self.candidate(key).cloned(),
            self.arr_client(key.0, key.1),
        ) else {
            return;
        };
        if let Some(form) = &mut self.finder.form {
            form.sending = true;
            form.error = None;
        }
        cx.notify();
        let search = options.search && !(key.1 == Kind::Sonarr && options.monitor == Monitor::None);
        cx.spawn(async move |this, cx| {
            let title = candidate.title.clone();
            let result = cx
                .background_executor()
                .spawn(async move { client.add(&setup, &candidate, &options) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(id) => {
                        eprintln!("[{}] added {title} (search: {search})", key.1.name());
                        this.finder.form = None;
                        this.finder.added.insert(key, search);
                        if let Some(Lookup::Ready(found)) =
                            this.finder.lookups.get_mut(&(key.0, key.1))
                            && let Some(c) = found.iter_mut().find(|c| c.key == key.2)
                        {
                            c.existing = Some(id);
                        }
                        // Its download shows up there soon.
                        this.refresh_downloads(cx);
                    }
                    Err(e) => {
                        eprintln!("[{}] adding {title}: {e:#}", key.1.name());
                        if let Some(form) = &mut this.finder.form {
                            form.sending = false;
                            form.error = Some(format!("{} said no: {e:#}", key.1.name()).into());
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
    pub(super) fn finder_sections(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let query = &self.finder.query;
        if query.chars().count() < MIN_QUERY {
            return Vec::new();
        }
        let apps = self.arr_apps();
        let mut servers: Vec<i64> = apps.iter().map(|(s, ..)| *s).collect();
        servers.dedup();
        let note = |text: String, color: u32| {
            div()
                .text_sm()
                .text_color(rgb(color))
                .child(text)
                .into_any_element()
        };
        let mut sections = Vec::new();
        for (server, kind, _) in apps {
            let key = (server, kind);
            let body: Vec<AnyElement> = match self.finder.lookups.get(&key) {
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
                    .map(|c| self.candidate_row(server, kind, c, cx))
                    .collect(),
            };
            let title = if servers.len() > 1 {
                format!(
                    "Add a {} to {} ({})",
                    kind_noun(kind),
                    self.source_name(Some(server)),
                    kind.name()
                )
            } else {
                format!("Add a {} ({})", kind_noun(kind), kind.name())
            };
            sections.push(Self::section(
                &title,
                div().flex().flex_col().gap_2().children(body),
            ));
        }
        sections
    }

    fn candidate_row(
        &self,
        server: i64,
        kind: Kind,
        candidate: &Candidate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key: ResultKey = (server, kind, candidate.key);
        let subject = match kind {
            Kind::Sonarr => Subject::Episodes {
                show: candidate.title.clone(),
                year: candidate.year,
                episodes: Vec::new(),
            },
            Kind::Radarr => Subject::Movie {
                title: candidate.title.clone(),
                year: candidate.year,
            },
        };
        let in_library = candidate
            .existing
            .and_then(|_| self.library_target(server, &subject));
        let poster = candidate
            .poster
            .as_ref()
            .and_then(|url| match self.finder.posters.get(url) {
                Some(Poster::Ready(path)) => Some(path.clone()),
                _ => None,
            });
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
        let action: AnyElement = if let Some(target) = in_library {
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
            self.button(
                SharedString::from(format!("add-{}-{}", kind.name(), candidate.key)),
                if open { "Cancel" } else { "Add…" },
                cx,
                move |this, _, cx| this.open_form(key, cx),
            )
        };
        let summary = div()
            .flex()
            .gap_4()
            .child(
                div()
                    .flex_none()
                    .w(px(POSTER_W))
                    .h(px(POSTER_H))
                    .rounded_md()
                    .overflow_hidden()
                    .bg(rgba(0xffffff0d))
                    .children(poster.map(|p| img(p).size_full().object_fit(ObjectFit::Cover))),
            )
            .child(
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
                            .children(candidate.year.map(|y| {
                                div().text_sm().text_color(rgb(MUTED)).child(y.to_string())
                            }))
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
                    })),
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
            .children(
                self.finder
                    .form
                    .as_ref()
                    .filter(|f| f.key == key)
                    .map(|form| self.add_form(form, candidate, cx)),
            )
            .into_any_element()
    }

    fn add_form(
        &self,
        form: &AddForm,
        candidate: &Candidate,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server, kind, _) = form.key;
        let note = |text: String, color: u32| {
            div()
                .text_sm()
                .text_color(rgb(color))
                .child(text)
                .into_any_element()
        };
        let frame = |children: Vec<AnyElement>| {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .pl(px(POSTER_W + 16.0))
                .children(children)
                .into_any_element()
        };
        let setup = match self.finder.setups.get(&(server, kind)) {
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
        let suggested = setup.suggest(candidate.is_anime());
        let roots = setup
            .roots
            .iter()
            .map(|r| {
                let mut label = r.path.clone();
                if let Some(space) = r.free_space {
                    label = format!("{label} · {}", free(space));
                }
                (label, false)
            })
            .enumerate()
            .map(|(i, (label, _))| (label, i == options.root))
            .collect();
        let mut children = vec![chips("add-root", "Folder", roots, cx, |this, i| {
            this.change_options(|o, setup| {
                o.root = i;
                if let Some(profile) = setup.profile_for(i) {
                    o.profile = profile;
                }
            })
        })];
        // Why this folder: what's in it.
        if let Some(habits) = setup.habits.get(options.root) {
            let noun = match kind {
                Kind::Sonarr => "shows",
                Kind::Radarr => "movies",
            };
            let (a, c) = (habits.anime, habits.count);
            let why = match (
                options.root == suggested && setup.roots.len() > 1,
                candidate.is_anime(),
            ) {
                (true, true) => {
                    format!(
                        "Suggested because it looks like anime, like {a} of the {c} {noun} here."
                    )
                }
                (true, false) => format!(
                    "Suggested because it doesn't look like anime; {a} of the {c} {noun} here are."
                ),
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
            children.push(note(format!("{why}{tags}"), MUTED));
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
            move |this, i| {
                let id = ids[i];
                this.change_options(|o, _| o.profile = id)
            },
        ));
        if kind == Kind::Sonarr {
            let monitors = Monitor::ALL
                .iter()
                .map(|m| (m.label().to_string(), *m == options.monitor))
                .collect();
            children.push(chips("add-monitor", "Monitor", monitors, cx, |this, i| {
                this.change_options(|o, _| o.monitor = Monitor::ALL[i])
            }));
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
                |this, i| this.change_options(|o, _| o.search = i == 0),
            ));
        }
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
}

/// FNV-1a: a stable name for a cached file.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
