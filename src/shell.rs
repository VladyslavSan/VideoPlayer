//! App shell: the library screens, and switching to and from the player.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, Entity, FocusHandle, FontWeight, KeyDownEvent,
    ObjectFit, PathPromptOptions, SharedString, Subscription, Window, div, img, prelude::*, px,
    rgb, rgba,
};

use crate::library::{Db, Item, scan, thumbs};
use crate::player::{self, Player, PlayerEvent};

const BG: u32 = 0x0b0d12;
const TEXT: u32 = 0xf4f5f7;
const MUTED: u32 = 0x8f98a8;
const ACCENT: u32 = 0x4c8dff;
const CARD_WIDTH: f32 = 256.0;
const CARD_HEIGHT: f32 = 144.0;
const CONTINUE_LIMIT: usize = 12;
const PLACEHOLDER_COLORS: [u32; 6] = [0x2b3a55, 0x3d2b55, 0x2b5547, 0x55402b, 0x552b3a, 0x2b4d55];

/// Where the library database and thumbnails live.
#[derive(Clone)]
pub struct Paths {
    pub db: PathBuf,
    pub thumbs: PathBuf,
}

impl Paths {
    /// `%LOCALAPPDATA%\VideoPlayer`, or `VIDEO_PLAYER_DATA_DIR` if set.
    pub fn resolve() -> Result<Self> {
        let base = match std::env::var_os("VIDEO_PLAYER_DATA_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => dirs::data_local_dir()
                .context("no local app data directory")?
                .join("VideoPlayer"),
        };
        Ok(Self {
            db: base.join("library.db"),
            thumbs: base.join("thumbs"),
        })
    }
}

#[derive(Clone, PartialEq)]
enum Screen {
    Home,
    Show(String),
}

/// What a poster card shows.
struct Card {
    id: ElementId,
    thumb: Option<PathBuf>,
    title: String,
    subtitle: Option<String>,
    progress: Option<f32>,
    watched: bool,
}

struct ActivePlayer {
    view: Entity<Player>,
    _subscription: Subscription,
}

pub struct Shell {
    db: Db,
    paths: Paths,
    items: Vec<Item>,
    roots: Vec<PathBuf>,
    screen: Screen,
    player: Option<ActivePlayer>,
    scanning: bool,
    thumbs_running: bool,
    thumbs_left: usize,
    error: Option<SharedString>,
    focus: FocusHandle,
}

impl Shell {
    pub fn new(
        paths: Paths,
        source: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        let db = Db::open(&paths.db)?;
        let focus = cx.focus_handle();
        focus.focus(window);
        let mut shell = Self {
            db,
            paths,
            items: Vec::new(),
            roots: Vec::new(),
            screen: Screen::Home,
            player: None,
            scanning: false,
            thumbs_running: false,
            thumbs_left: 0,
            error: None,
            focus,
        };
        shell.reload();
        shell.rescan(cx);
        shell.generate_thumbs(cx);
        if let Some(source) = source {
            let title = Path::new(&source)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| source.clone());
            shell.start_player(source, title.into(), 0.0, None, window, cx);
        }
        Ok(shell)
    }

    fn reload(&mut self) {
        match self
            .db
            .items()
            .and_then(|items| Ok((items, self.db.roots()?)))
        {
            Ok((items, roots)) => {
                self.items = items;
                self.roots = roots;
            }
            Err(e) => self.error = Some(format!("Couldn't read the library: {e}").into()),
        }
    }

    fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.scanning || self.roots.is_empty() {
            return;
        }
        self.scanning = true;
        let roots = self.roots.clone();
        let db_path = self.paths.db.clone();
        cx.spawn(async move |this, cx| {
            let errors: Vec<String> = cx
                .background_executor()
                .spawn(async move {
                    // SQLite connections aren't shared across threads; open one here.
                    let db = match Db::open(&db_path) {
                        Ok(db) => db,
                        Err(e) => return vec![e.to_string()],
                    };
                    roots
                        .iter()
                        .filter_map(|root| scan::scan_root(&db, root).err())
                        .map(|e| e.to_string())
                        .collect()
                })
                .await;
            this.update(cx, |this, cx| {
                this.scanning = false;
                this.error = (!errors.is_empty()).then(|| errors.join("; ").into());
                this.reload();
                this.generate_thumbs(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Generates missing thumbnails one at a time in the background.
    fn generate_thumbs(&mut self, cx: &mut Context<Self>) {
        if self.thumbs_running {
            return;
        }
        let pending = self.db.pending_thumbs().unwrap_or_default();
        if pending.is_empty() {
            return;
        }
        self.thumbs_running = true;
        self.thumbs_left = pending.len();
        let dir = self.paths.thumbs.clone();
        cx.spawn(async move |this, cx| {
            for (id, video) in pending {
                let out = dir.join(format!("{id}.jpg"));
                let target = out.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        fs::create_dir_all(target.parent().unwrap())?;
                        thumbs::generate(&video, &target)
                    })
                    .await;
                let alive = this.update(cx, |this, cx| {
                    if let Err(e) = &result {
                        eprintln!("[thumbs] item {id}: {e}");
                    }
                    let thumb = result.is_ok().then_some(out.as_path());
                    if let Err(e) = this.db.set_thumb(id, thumb) {
                        eprintln!("[thumbs] saving item {id}: {e}");
                    }
                    this.thumbs_left = this.thumbs_left.saturating_sub(1);
                    this.reload();
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.thumbs_running = false;
                // Pick up anything a scan added while this batch ran.
                this.generate_thumbs(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add to Library".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            this.update(cx, |this, cx| {
                for path in paths {
                    if let Err(e) = this.db.add_root(&path) {
                        this.error = Some(format!("Couldn't add {}: {e}", path.display()).into());
                    }
                }
                this.reload();
                this.rescan(cx);
            })
            .ok();
        })
        .detach();
    }

    fn play_item(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.iter().find(|i| i.id == id).cloned() else {
            return;
        };
        if !item.path.exists() {
            self.error = Some(format!("{} is not available", item.path.display()).into());
            cx.notify();
            return;
        }
        let source = item.path.to_string_lossy().into_owned();
        let title = item.display_title().into();
        self.start_player(source, title, item.resume_position(), Some(id), window, cx);
    }

    fn start_player(
        &mut self,
        source: String,
        title: SharedString,
        start: f64,
        item: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let video = match player::open_video(window, &source, start) {
            Ok(video) => video,
            Err(e) => {
                self.error = Some(format!("Couldn't play {title}: {e}").into());
                cx.notify();
                return;
            }
        };
        self.error = None;
        let view = cx.new(|cx| Player::new(video, title, window, cx));
        let subscription = cx.subscribe_in(&view, window, move |this, _, event, window, cx| {
            this.on_player_event(item, event, window, cx)
        });
        self.player = Some(ActivePlayer {
            view,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn on_player_event(
        &mut self,
        item: Option<i64>,
        event: &PlayerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PlayerEvent::Progress { position, duration } => {
                if let Some(id) = item
                    && let Err(e) = self.db.save_progress(id, *position, *duration)
                {
                    eprintln!("[library] saving progress: {e}");
                }
            }
            PlayerEvent::Exit => {
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                // Dropping the view shuts mpv down and destroys its window.
                self.player = None;
                self.reload();
                self.focus.focus(window);
                cx.notify();
            }
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(event.keystroke.key.as_str(), "escape" | "backspace")
            && self.screen != Screen::Home
        {
            self.screen = Screen::Home;
            cx.notify();
        }
    }

    // ---- rendering ----

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px_3()
            .py_1p5()
            .rounded_md()
            .text_sm()
            .bg(rgba(0xffffff14))
            .hover(|s| s.bg(rgba(0xffffff26)))
            .cursor_pointer()
            .child(label)
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| action(this, window, cx)))
            .into_any_element()
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let title: AnyElement = match &self.screen {
            Screen::Home => div()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .child("Library")
                .into_any_element(),
            Screen::Show(name) => div()
                .flex()
                .items_center()
                .gap_3()
                .child(self.button("home", "‹ Library", cx, |this, _, cx| {
                    this.screen = Screen::Home;
                    cx.notify();
                }))
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::BOLD)
                        .child(name.clone()),
                )
                .into_any_element(),
        };
        let status = if self.scanning {
            Some(SharedString::from("Scanning…"))
        } else if self.thumbs_left > 0 {
            Some(format!("Generating thumbnails ({} left)", self.thumbs_left).into())
        } else {
            None
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .px_8()
            .py_5()
            .child(title)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .children(status.map(|s| div().text_sm().text_color(rgb(MUTED)).child(s)))
                    .children(
                        self.error
                            .clone()
                            .map(|e| div().text_sm().text_color(rgb(0xff7a7a)).child(e)),
                    )
                    .children((!self.roots.is_empty()).then(|| {
                        self.button("rescan", "Rescan", cx, |this, _, cx| this.rescan(cx))
                    }))
                    .child(self.button("add", "Add Folder", cx, |this, _, cx| this.add_folder(cx))),
            )
            .into_any_element()
    }

    fn card(
        &self,
        card: Card,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let Card {
            id,
            thumb: item_thumb,
            title,
            subtitle,
            progress,
            watched,
        } = card;
        let color = PLACEHOLDER_COLORS
            [title.bytes().map(usize::from).sum::<usize>() % PLACEHOLDER_COLORS.len()];
        let initial: String = title.chars().next().map(String::from).unwrap_or_default();
        let placeholder = move || {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(color))
                .text_3xl()
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(0xffffffb0))
                .child(initial.clone())
                .into_any_element()
        };
        let art: AnyElement = match item_thumb {
            Some(path) => img(path)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(placeholder)
                .into_any_element(),
            None => placeholder(),
        };

        div()
            .id(id)
            .flex_none()
            .w(px(CARD_WIDTH))
            .flex()
            .flex_col()
            .gap_2()
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(CARD_HEIGHT))
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(rgb(color))
                    .child(art)
                    .children(progress.map(|p| {
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h_1()
                            .bg(rgba(0x00000099))
                            .child(div().h_full().w(gpui::relative(p)).bg(rgb(ACCENT)))
                    }))
                    .children(watched.then(|| {
                        div()
                            .absolute()
                            .top_2()
                            .right_2()
                            .px_2()
                            .rounded_md()
                            .text_xs()
                            .bg(rgba(0x000000a0))
                            .child("Watched")
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .truncate()
                    .child(title),
            )
            .children(subtitle.map(|s| div().text_xs().text_color(rgb(MUTED)).truncate().child(s)))
            .on_click(
                cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)),
            )
            .into_any_element()
    }

    fn item_card(
        &self,
        prefix: &str,
        item: &Item,
        show_context: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, subtitle) = match (show_context, &item.show, item.episode_label()) {
            (true, Some(show), Some(label)) => {
                (show.clone(), Some(format!("{label} · {}", item.title)))
            }
            (false, Some(_), _) => (
                format!("{}. {}", item.episode.unwrap_or(0), item.title),
                None,
            ),
            _ => (item.title.clone(), item.year.map(|y| y.to_string())),
        };
        let id = item.id;
        let card = Card {
            id: SharedString::from(format!("{prefix}-{id}")).into(),
            thumb: item.thumb.clone(),
            title,
            subtitle,
            progress: item.progress(),
            watched: item.watched,
        };
        self.card(card, cx, move |this, window, cx| {
            this.play_item(id, window, cx)
        })
    }

    fn section(title: &str, body: impl IntoElement) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .pt_6()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title.to_string()),
            )
            .child(body)
            .into_any_element()
    }

    fn grid(cards: Vec<AnyElement>) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap_5()
            .children(cards)
            .into_any_element()
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let message = if self.roots.is_empty() {
            "Add a folder with movies or TV shows to get started."
        } else if self.scanning {
            "Looking for videos…"
        } else {
            "No videos found in your folders yet."
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .pt(px(160.0))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Your library is empty"),
            )
            .child(div().text_color(rgb(MUTED)).child(message))
            .child(self.button("add-empty", "Add Folder", cx, |this, _, cx| {
                this.add_folder(cx)
            }))
            .into_any_element()
    }

    fn home(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.items.is_empty() {
            return vec![self.empty_state(cx)];
        }
        let mut sections = Vec::new();

        let mut in_progress: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.progress().is_some())
            .collect();
        in_progress.sort_by_key(|i| std::cmp::Reverse(i.last_played));
        if !in_progress.is_empty() {
            let cards = in_progress
                .into_iter()
                .take(CONTINUE_LIMIT)
                .map(|i| self.item_card("continue", i, true, cx))
                .collect::<Vec<_>>();
            sections.push(Self::section(
                "Continue Watching",
                div()
                    .id("continue-row")
                    .flex()
                    .gap_5()
                    .pb_2()
                    .overflow_x_scroll()
                    .children(cards),
            ));
        }

        let mut movies: Vec<&Item> = self.items.iter().filter(|i| !i.is_episode()).collect();
        movies.sort_by_key(|i| i.title.to_lowercase());
        if !movies.is_empty() {
            let cards = movies
                .into_iter()
                .map(|i| self.item_card("movie", i, true, cx))
                .collect();
            sections.push(Self::section("Movies", Self::grid(cards)));
        }

        let mut shows: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
        for item in &self.items {
            if let Some(show) = &item.show {
                shows.entry(show.clone()).or_default().push(item);
            }
        }
        if !shows.is_empty() {
            let cards = shows
                .into_iter()
                .map(|(show, episodes)| {
                    let thumb = episodes.iter().find_map(|e| e.thumb.clone());
                    let unwatched = episodes.iter().filter(|e| !e.watched).count();
                    let subtitle = match unwatched {
                        0 => format!("{} episodes · all watched", episodes.len()),
                        n if n == episodes.len() => format!("{n} episodes"),
                        n => format!("{} episodes · {n} unwatched", episodes.len()),
                    };
                    let name = show.clone();
                    let card = Card {
                        id: SharedString::from(format!("show-{show}")).into(),
                        thumb,
                        title: show,
                        subtitle: Some(subtitle),
                        progress: None,
                        watched: unwatched == 0,
                    };
                    self.card(card, cx, move |this, _, cx| {
                        this.screen = Screen::Show(name.clone());
                        cx.notify();
                    })
                })
                .collect();
            sections.push(Self::section("TV Shows", Self::grid(cards)));
        }
        sections
    }

    fn show_page(&self, show: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut seasons: BTreeMap<u16, Vec<&Item>> = BTreeMap::new();
        for item in self
            .items
            .iter()
            .filter(|i| i.show.as_deref() == Some(show))
        {
            seasons
                .entry(item.season.unwrap_or(0))
                .or_default()
                .push(item);
        }
        seasons
            .into_iter()
            .map(|(season, episodes)| {
                let title = if season == 0 {
                    "Specials".to_string()
                } else {
                    format!("Season {season}")
                };
                let cards = episodes
                    .into_iter()
                    .map(|e| self.item_card("episode", e, false, cx))
                    .collect();
                Self::section(&title, Self::grid(cards))
            })
            .collect()
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(player) = &self.player {
            return div()
                .size_full()
                .child(player.view.clone())
                .into_any_element();
        }
        let body = match self.screen.clone() {
            Screen::Home => self.home(cx),
            Screen::Show(name) => self.show_page(&name, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .child(self.header(cx))
            .child(
                div()
                    .id("content")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px_8()
                    .pb_10()
                    .children(body),
            )
            .into_any_element()
    }
}
