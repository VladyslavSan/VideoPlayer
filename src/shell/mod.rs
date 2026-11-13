//! App shell: the library screens (local folders and Jellyfin), the Jellyfin
//! sign-in screen, and switching to and from the player.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use gpui::{
    Animation, AnimationExt as _, AnyElement, Bounds, ClickEvent, Context, ElementId, Entity,
    FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseDownEvent, ObjectFit, PathPromptOptions,
    Pixels, ScrollHandle, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle,
    Window, canvas, div, img, point, prelude::*, px, rgb, rgba,
};

use crate::introdb;
use crate::jellyfin::{
    self, Client, ImageRef, PlaySession, PlaybackInfo, PlaybackReport, QUALITIES, Quality, Server,
    discovery,
};
use crate::library::{Db, Item, Source, TrackPrefs, scan, thumbs};
use crate::media_session::MediaSession;
use crate::player::{self, Episode, Playback, Player, PlayerEvent, SegmentSource};
use crate::secrets;
use crate::ui::menu;
use crate::ui::text_input::TextInput;

mod arr_settings;
mod browse;
mod details;
mod keynav;
mod live;
mod releases;
mod reports;
mod settings;
mod sonarr;
use browse::{Grid, Sort};
use details::Details;
use settings::SettingsPage;

const BG: u32 = 0x0b0d12;
const TEXT: u32 = 0xf4f5f7;
const MUTED: u32 = 0x8f98a8;
const ACCENT: u32 = 0x4c8dff;
const ERROR: u32 = 0xff7a7a;
const CARD_WIDTH: f32 = 256.0;
const CARD_HEIGHT: f32 = 144.0;
const CONTINUE_LIMIT: usize = 12;
const VOLUME_SETTING: &str = "volume";
const GENERAL_PREFS: &str = "general";
const DISCOVERY_WAIT: Duration = Duration::from_millis(1500);
/// Switching back to the app re-reads Jellyfin libraries older than this.
const REFRESH_ON_FOCUS_AFTER: Duration = Duration::from_secs(60);
const PLACEHOLDER_COLORS: [u32; 6] = [0x2b3a55, 0x3d2b55, 0x2b5547, 0x55402b, 0x552b3a, 0x2b4d55];

/// Where the library database and thumbnails live.
#[derive(Clone)]
pub struct Paths {
    pub db: PathBuf,
    pub thumbs: PathBuf,
    /// Diagnostics of release builds, which have no console.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    pub log: PathBuf,
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
            log: base.join("app.log"),
        })
    }

    /// Seek bar preview pictures of the video playing (temporary).
    pub fn previews(&self) -> PathBuf {
        self.db.with_file_name("previews")
    }

    /// Where the library last fetched from a server is kept.
    pub fn library_cache(&self, server: i64) -> PathBuf {
        self.db.with_file_name(format!("jellyfin-{server}.json"))
    }
}

/// A fresh id for a Jellyfin play session.
fn new_session_id() -> String {
    use std::hash::{BuildHasher, RandomState};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "{:016x}{:016x}",
        RandomState::new().hash_one(nanos),
        RandomState::new().hash_one(std::process::id())
    )
}

/// Where to stream an item from at `quality`.
fn stream_url(client: &Client, item_id: &str, session: &PlaySession, quality: Quality) -> String {
    match quality {
        Quality::Original => client.stream_url(item_id),
        Quality::Bitrate(bitrate) => client.transcode_url(item_id, session, bitrate),
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, PartialEq)]
enum Screen {
    /// Every source together.
    Home,
    /// One source's page: a Jellyfin server, or None for local folders.
    Source(browse::Origin),
    /// A library's grid of movies, or of shows.
    Movies(browse::Shelf),
    Shows(browse::Shelf),
    Show(String),
    /// A movie's details page, by item key.
    Details(String),
    Settings,
}

/// A season folding open or shut, from when; `id` restarts the animation.
struct Fold {
    opening: bool,
    started: Instant,
    id: u64,
}

/// What a poster card shows.
struct Card {
    id: SharedString,
    thumb: Option<PathBuf>,
    title: String,
    subtitle: Option<String>,
    progress: Option<f32>,
    watched: bool,
    /// Ringed: the episode a show page was opened at.
    highlight: bool,
}

/// Cards drawn in a frame: element id and where.
type CardBounds = Rc<RefCell<Vec<(SharedString, Bounds<Pixels>)>>>;

/// What Enter does on a card: the same as clicking it.
type CardAction = Rc<dyn Fn(&mut Shell, &mut Window, &mut Context<Shell>)>;

/// Room kept between the card picked with the keyboard and the page's edge.
const CURSOR_MARGIN: f32 = 24.0;

fn item_card_id(prefix: &str, item: &Item) -> SharedString {
    format!("{prefix}-{}", item.key()).into()
}

fn show_card_id(prefix: &str, show: &str) -> SharedString {
    format!("{prefix}-show-{show}").into()
}

/// How long a show page opened at an episode keeps it in place while the
/// page above it settles.
const FOCUS_SCROLL_SETTLE: Duration = Duration::from_millis(1500);

struct FocusScroll {
    until: Instant,
    /// The offset last set; a different one means the viewer scrolled.
    applied: Option<Pixels>,
}

/// What clicking a card's text (below the picture) does, if it differs
/// from clicking the picture.
type TextClick = Box<dyn Fn(&mut Shell, &mut Window, &mut Context<Shell>)>;

struct ActivePlayer {
    view: Entity<Player>,
    /// None when playing a file passed on the command line.
    source: Option<Source>,
    position: f64,
    duration: f64,
    /// The show being watched, whose track choices are remembered.
    show: Option<String>,
    /// Jellyfin playback: the session reports and transcoding belong to.
    session: Option<PlaySession>,
    /// The item's streams, to rebuild the subtitle list when the quality
    /// changes (a transcoded stream needs more of them as files).
    playback_info: Option<PlaybackInfo>,
    _subscription: Subscription,
}

struct ConnectForm {
    url: Entity<TextInput>,
    user: Entity<TextInput>,
    password: Entity<TextInput>,
    busy: bool,
    searching: bool,
    found: Vec<discovery::Found>,
    error: Option<SharedString>,
}

pub struct Shell {
    db: Db,
    paths: Paths,
    device_id: String,
    local: Vec<Item>,
    remote: Vec<Item>,
    roots: Vec<PathBuf>,
    servers: Vec<Server>,
    screen: Screen,
    /// The library tab to go back to from a show page.
    tab: Screen,
    movie_sort: Sort,
    show_sort: Sort,
    search: Entity<TextInput>,
    /// Details pages' extra information, by item key or `show:<name>`.
    details: HashMap<String, Details>,
    details_loading: HashSet<String>,
    player: Option<ActivePlayer>,
    scanning: bool,
    loading_servers: HashSet<i64>,
    /// Servers to fetch again when their current fetch ends, because
    /// something changed (playback stopped) after it began.
    refetch_servers: HashSet<i64>,
    thumbs_running: bool,
    thumbs_left: usize,
    artwork_left: usize,
    error: Option<SharedString>,
    focus: FocusHandle,
    form: ConnectForm,
    /// Sonarr and Radarr connections, and their Settings forms.
    arr: arr_settings::ArrForms,
    /// What Sonarr says about shows (by name), which are being asked,
    /// and the Download buttons pressed (show, season).
    sonarr: HashMap<String, sonarr::SonarrView>,
    sonarr_loading: HashSet<String>,
    sonarr_requests: HashMap<(String, u32), sonarr::Request>,
    /// Open release lists, by show and season.
    release_panels: HashMap<releases::PanelKey, releases::ReleasePanel>,
    /// The server whose streaming quality menu is open.
    quality_menu: Option<i64>,
    /// The Settings page shown, and whether its sign-in form is open.
    settings_page: SettingsPage,
    adding_server: bool,
    reports: reports::Reports,
    /// The item whose resume point is being asked from the server.
    starting: Option<String>,
    /// When the Jellyfin libraries were last fetched.
    remote_refreshed: Option<Instant>,
    /// Each signed-in server's change notifications, with the token used.
    live: HashMap<i64, (String, crate::jellyfin::live::Watcher)>,
    /// A fetch because of server changes is waiting to run.
    live_refresh_pending: bool,
    /// The scrolling page (everything but the virtualized grids).
    content_scroll: ScrollHandle,
    /// The screen the page was last drawn for; a new one starts at the top.
    scrolled_screen: Option<Screen>,
    /// The seasons open on each show's page (the rest are folded).
    open_seasons: HashMap<String, BTreeSet<u16>>,
    /// Seasons being folded open or shut (show, season), for the animation,
    /// and each open season's height as last drawn.
    folds: HashMap<(String, u16), Fold>,
    season_heights: Rc<RefCell<HashMap<(String, u16), f32>>>,
    fold_count: u64,
    /// Cards across, as last laid out (for a season's height while it folds).
    columns_hint: Cell<usize>,
    /// Counts screen changes; each new page fades in.
    page_generation: usize,
    /// On a show page: the episode it was opened at (by item key), ringed.
    focus_episode: Option<String>,
    /// Where the focused episode's card was last drawn, and whether the
    /// page should still scroll to it.
    focus_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Scrolling to that card, while the page settles.
    focus_scroll: Option<FocusScroll>,
    /// The system's media controls, set up with the first video.
    media: Option<Rc<MediaSession>>,
    /// The card picked with the arrow keys (its element id), if any.
    cursor: Option<SharedString>,
    /// Each screen's cursor when it was left, to put back on return.
    cursors: Vec<(Screen, SharedString)>,
    /// Every card drawn in the last frame, and the screen it was drawn for.
    card_bounds: CardBounds,
    bounds_screen: Option<Screen>,
    /// What Enter does for each card being drawn.
    card_actions: RefCell<HashMap<SharedString, CardAction>>,
    /// Sideways scrolling of the rows on the home page, by row id.
    row_scroll: RefCell<HashMap<SharedString, ScrollHandle>>,
    /// Scrolling of the library grids.
    grid_scroll: UniformListScrollHandle,
    /// Bring the cursor's card into view once it has been drawn.
    reveal_cursor: bool,
}

impl Shell {
    pub fn new(
        paths: Paths,
        source: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self> {
        let db = Db::open(&paths.db)?;
        let device_id = db.device_id()?;
        let focus = cx.focus_handle();
        focus.focus(window);
        let form = ConnectForm {
            url: cx.new(|cx| TextInput::new("http://192.168.1.10:8096", false, cx)),
            user: cx.new(|cx| TextInput::new("Username", false, cx)),
            password: cx.new(|cx| TextInput::new("Password", true, cx)),
            busy: false,
            searching: false,
            found: Vec::new(),
            error: None,
        };
        let search = cx.new(|cx| TextInput::new("Search", false, cx));
        // Re-render results as the query changes.
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let mut shell = Self {
            db,
            paths,
            device_id,
            local: Vec::new(),
            remote: Vec::new(),
            roots: Vec::new(),
            servers: Vec::new(),
            screen: Screen::Home,
            tab: Screen::Home,
            movie_sort: Sort::Title,
            show_sort: Sort::RecentlyWatched,
            search,
            details: HashMap::new(),
            details_loading: HashSet::new(),
            quality_menu: None,
            settings_page: SettingsPage::default(),
            adding_server: false,
            reports: reports::Reports::default(),
            starting: None,
            remote_refreshed: None,
            live: HashMap::new(),
            live_refresh_pending: false,
            content_scroll: ScrollHandle::new(),
            scrolled_screen: None,
            open_seasons: HashMap::new(),
            folds: HashMap::new(),
            season_heights: Rc::default(),
            fold_count: 0,
            columns_hint: Cell::new(4),
            page_generation: 0,
            focus_episode: None,
            focus_bounds: Rc::default(),
            focus_scroll: None,
            media: None,
            cursor: None,
            cursors: Vec::new(),
            card_bounds: Rc::default(),
            bounds_screen: None,
            card_actions: RefCell::default(),
            row_scroll: RefCell::default(),
            grid_scroll: UniformListScrollHandle::new(),
            reveal_cursor: false,
            player: None,
            scanning: false,
            loading_servers: HashSet::new(),
            refetch_servers: HashSet::new(),
            thumbs_running: false,
            thumbs_left: 0,
            artwork_left: 0,
            error: None,
            focus,
            form,
            arr: HashMap::new(),
            sonarr: HashMap::new(),
            sonarr_loading: HashSet::new(),
            sonarr_requests: HashMap::new(),
            release_panels: HashMap::new(),
        };
        // Closing the window while a video is open still reports it.
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            this.update(cx, |this, cx| this.close_requested(cx))
                .unwrap_or(true)
        });
        // Coming back to the app picks up changes made elsewhere (watched in
        // another app, marked watched on the web).
        cx.observe_window_activation(window, |this, window, cx| {
            let stale = this
                .remote_refreshed
                .is_none_or(|at| at.elapsed() >= REFRESH_ON_FOCUS_AFTER);
            if window.is_window_active() && this.player.is_none() && stale {
                this.refresh_remote(cx);
            }
        })
        .detach();
        if let Some(dir) = crate::test_window::capture_dir() {
            shell.serve_test_captures(dir, window, cx);
        }
        shell.reload();
        shell.load_cached_remote();
        shell.refresh(cx);
        shell.generate_thumbs(cx);
        if let Some(source) = source {
            let title = Path::new(&source)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| source.clone());
            let playback = Playback {
                title: title.into(),
                subtitle: None,
                episodes: Vec::new(),
                current: None,
                prefs: shell.track_prefs(None),
                quality: None,
                thumb: None,
            };
            shell.start_player(source, None, playback, 0.0, None, window, cx);
        }
        Ok(shell)
    }

    /// Test runs: screenshots on request, taken by the app itself (see
    /// `test_window::capture_dir`).
    fn serve_test_captures(&self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let request = dir.join("request");
                let Ok(name) = fs::read_to_string(&request) else {
                    continue;
                };
                let _ = fs::remove_file(&request);
                let name = name.trim().to_string();
                gpui::frame_capture::capture_next_frame(dir.join(format!("{name}.ui.png")));
                let served = this.update_in(cx, |this, window, cx| {
                    if let Some(active) = &this.player {
                        let video = dir.join(format!("{name}.video.png"));
                        active.view.read(cx).screenshot(&video);
                    }
                    window.refresh();
                    cx.notify();
                });
                if served.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn items(&self) -> impl Iterator<Item = &Item> {
        self.local.iter().chain(self.remote.iter())
    }

    /// Re-reads local items, folders and servers from the database.
    fn reload(&mut self) {
        let loaded =
            (|| Ok::<_, anyhow::Error>((self.db.items()?, self.db.roots()?, self.db.servers()?)))();
        match loaded {
            Ok((items, roots, mut servers)) => {
                self.local = items;
                self.roots = roots;
                if let Err(e) = self.attach_tokens(&mut servers) {
                    self.error = Some(format!("Couldn't read saved sign-ins: {e:#}").into());
                }
                self.servers = servers;
            }
            Err(e) => self.error = Some(format!("Couldn't read the library: {e}").into()),
        }
    }

    /// Fills in each server's token from the credential store. A token that
    /// an older version left in the database is moved there first.
    fn attach_tokens(&self, servers: &mut [Server]) -> Result<()> {
        for server in servers {
            let account = server.token_account(&self.device_id);
            if server.token.is_empty() {
                server.token = secrets::load(&account)?.unwrap_or_default();
            } else {
                secrets::save(&account, &server.token)?;
                self.db.clear_token(server.id)?;
            }
        }
        Ok(())
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.rescan(cx);
        self.refresh_remote(cx);
    }

    // ---- local folders ----

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
                if !errors.is_empty() {
                    this.error = Some(errors.join("; ").into());
                }
                this.reload();
                this.generate_thumbs(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Generates missing local thumbnails one at a time in the background.
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

    // ---- Jellyfin ----

    fn client(&self, server: i64) -> Option<Client> {
        let server = self.servers.iter().find(|s| s.id == server)?;
        Some(Client::for_server(server, &self.device_id))
    }

    /// Fetches every server's library; artwork follows in the background.
    fn refresh_remote(&mut self, cx: &mut Context<Self>) {
        self.remote_refreshed = Some(Instant::now());
        self.sync_live(cx);
        self.sync_arr(cx);
        for server in self.servers.clone() {
            if server.token.is_empty() {
                // Removed from the credential store outside the app.
                self.error = Some(
                    format!(
                        "{}: signed out; sign in again in Settings → Library",
                        server.name
                    )
                    .into(),
                );
                continue;
            }
            if !self.loading_servers.insert(server.id) {
                self.refetch_servers.insert(server.id);
                continue;
            }
            let client = Client::for_server(&server, &self.device_id);
            let thumbs = self.paths.thumbs.clone();
            let cache = self.paths.library_cache(server.id);
            cx.spawn(async move |this, cx| {
                let user_id = server.user_id.clone();
                let server_row = server.id;
                let fetch_client = client.clone();
                let fetched = cx
                    .background_executor()
                    .spawn(async move {
                        let items = fetch_client.library(&user_id)?;
                        if let Err(e) = jellyfin::save_library_cache(&cache, &items) {
                            eprintln!("[jellyfin] saving the library cache: {e:#}");
                        }
                        Ok::<_, anyhow::Error>(
                            items
                                .iter()
                                .map(|b| (b.to_item(server_row, &thumbs), b.card_image()))
                                .collect::<Vec<_>>(),
                        )
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.loading_servers.remove(&server.id);
                    match fetched {
                        Ok(entries) => {
                            this.remote.retain(|i| {
                                !matches!(&i.source, Source::Jellyfin { server: s, .. } if *s == server.id)
                            });
                            let mut wanted = Vec::new();
                            for (item, image) in entries {
                                if item.thumb.is_none()
                                    && let Some(image) = image
                                {
                                    wanted.push((item.key(), image));
                                }
                                this.remote.push(item);
                            }
                            this.fetch_artwork(client, wanted, cx);
                        }
                        Err(e) => {
                            eprintln!("[jellyfin] {}: {e:#}", server.name);
                            let cached = this.remote.iter().any(|i| {
                                matches!(&i.source, Source::Jellyfin { server: s, .. } if *s == server.id)
                            });
                            this.error = Some(
                                if cached {
                                    format!("{} is offline; showing the saved library", server.name)
                                } else {
                                    format!("{}: {e:#}", server.name)
                                }
                                .into(),
                            );
                        }
                    }
                    if this.refetch_servers.remove(&server.id) {
                        this.refresh_remote(cx);
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Shows each server's last fetched library right away; the refresh
    /// that follows replaces it.
    fn load_cached_remote(&mut self) {
        for server in &self.servers {
            let path = self.paths.library_cache(server.id);
            if !path.exists() {
                continue;
            }
            match jellyfin::load_library_cache(&path) {
                Ok(items) => self.remote.extend(
                    items
                        .iter()
                        .map(|b| b.to_item(server.id, &self.paths.thumbs)),
                ),
                Err(e) => eprintln!("[jellyfin] reading {}: {e:#}", path.display()),
            }
        }
    }

    fn fetch_artwork(
        &mut self,
        client: Client,
        wanted: Vec<(String, ImageRef)>,
        cx: &mut Context<Self>,
    ) {
        // Episodes can share artwork (a show backdrop), so download each file once.
        let mut by_file: BTreeMap<PathBuf, (ImageRef, Vec<String>)> = BTreeMap::new();
        for (key, image) in wanted {
            let path = image.cache_path(&self.paths.thumbs);
            by_file
                .entry(path)
                .or_insert_with(|| (image, Vec::new()))
                .1
                .push(key);
        }
        if by_file.is_empty() {
            return;
        }
        self.artwork_left += by_file.len();
        cx.spawn(async move |this, cx| {
            for (path, (image, keys)) in by_file {
                let target = path.clone();
                let download = client.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        fs::create_dir_all(target.parent().unwrap())?;
                        download.download_image(&image, &target)
                    })
                    .await;
                let alive = this.update(cx, |this, cx| {
                    this.artwork_left = this.artwork_left.saturating_sub(1);
                    match result {
                        Ok(()) => {
                            for item in this.remote.iter_mut().filter(|i| keys.contains(&i.key())) {
                                item.thumb = Some(path.clone());
                            }
                        }
                        Err(e) => eprintln!("[jellyfin] artwork {}: {e:#}", path.display()),
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    fn discover(&mut self, cx: &mut Context<Self>) {
        self.form.searching = true;
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { discovery::discover(DISCOVERY_WAIT) })
                .await;
            this.update(cx, |this, cx| {
                this.form.searching = false;
                let url_empty = this.form.url.read(cx).text().is_empty();
                if url_empty && let Some(first) = found.first() {
                    let address = first.address.clone();
                    this.form
                        .url
                        .update(cx, |input, cx| input.set_text(address, cx));
                }
                this.form.found = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.busy {
            return;
        }
        let url = jellyfin::normalize_url(&self.form.url.read(cx).text());
        let user = self.form.user.read(cx).text().trim().to_string();
        let password = self.form.password.read(cx).text();
        if url == "http://" || user.is_empty() {
            self.form.error = Some("Enter the server address and your username.".into());
            cx.notify();
            return;
        }
        self.form.busy = true;
        self.form.error = None;
        let device_id = self.device_id.clone();
        let focus = self.focus.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut client = Client::new(&url, &device_id);
                    let info = client.public_info()?;
                    let auth = client.login(&user, &password)?;
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
                    secrets::save(&server.token_account(&device_id), &server.token)?;
                    Ok(server)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.form.busy = false;
                match result.and_then(|server| this.db.save_server(&server)) {
                    Ok(_) => {
                        this.form
                            .password
                            .update(cx, |input, cx| input.set_text("", cx));
                        // A first library goes straight to Home; another
                        // server shows up in the Settings list.
                        let first = this.servers.is_empty() && this.roots.is_empty();
                        this.reload();
                        this.adding_server = false;
                        if first {
                            this.screen = Screen::Home;
                        }
                        window.focus(&focus);
                        this.refresh_remote(cx);
                    }
                    Err(e) => this.form.error = Some(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn sign_out(&mut self, server_id: i64, cx: &mut Context<Self>) {
        if let Some(client) = self.client(server_id) {
            // Best effort: invalidate the token on the server too.
            cx.background_executor()
                .spawn(async move {
                    let _ = client.logout();
                })
                .detach();
        }
        let server = self.servers.iter().find(|s| s.id == server_id).cloned();
        if let Some(server) = &server {
            self.forget_arr(server);
        }
        let account = server.map(|s| s.token_account(&self.device_id));
        let removed = account
            .map_or(Ok(()), |account| secrets::delete(&account))
            .and_then(|()| self.db.remove_server(server_id));
        if let Err(e) = removed {
            self.error = Some(format!("Couldn't sign out: {e:#}").into());
        }
        let _ = fs::remove_file(self.paths.library_cache(server_id));
        self.remote.retain(
            |i| !matches!(&i.source, Source::Jellyfin { server, .. } if *server == server_id),
        );
        self.reload();
        self.sync_live(cx);
        self.sync_arr(cx);
        cx.notify();
    }

    fn focus_next_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fields = [&self.form.url, &self.form.user, &self.form.password];
        let current = fields
            .iter()
            .position(|f| f.focus_handle(cx).is_focused(window));
        let next = current.map(|i| (i + 1) % fields.len()).unwrap_or(0);
        window.focus(&fields[next].focus_handle(cx));
    }

    // ---- playback ----

    /// Plays an item from where it was left off. For a Jellyfin item, that's
    /// asked from the server first: the library here may be minutes old (it
    /// refreshes in the background) or the item may have been watched
    /// elsewhere since.
    fn play(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items().find(|i| i.key() == key).cloned() else {
            return;
        };
        let Source::Jellyfin { server, id } = &item.source else {
            return self.play_at(item, false, window, cx);
        };
        let (Some(client), Some(user_id)) = (self.client(*server), self.user_id(*server)) else {
            return self.play_at(item, false, window, cx);
        };
        if self.starting.is_some() {
            return;
        }
        self.starting = Some(item.key());
        let id = id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let fresh = cx
                .background_executor()
                .spawn(async move { client.user_data(&user_id, &id) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.starting = None;
                let mut item = item;
                match fresh {
                    Ok(data) => {
                        item.position = jellyfin::ticks_to_seconds(data.playback_position_ticks);
                        item.watched = data.played;
                        if let Some(cached) = this.remote.iter_mut().find(|i| i.key() == item.key())
                        {
                            cached.position = item.position;
                            cached.watched = item.watched;
                        }
                    }
                    Err(e) => eprintln!("[jellyfin] resume point: {e:#}; using the saved one"),
                }
                if this.player.is_none() {
                    this.play_at(item, false, window, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn play_from_start(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let item = self.items().find(|i| i.key() == key).cloned();
        if let Some(item) = item {
            self.play_at(item, true, window, cx)
        }
    }

    /// Plays the next episode right away, from the saved resume point.
    fn play_next_now(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let item = self.items().find(|i| i.key() == key).cloned();
        if let Some(item) = item {
            self.play_at(item, false, window, cx)
        }
    }

    fn user_id(&self, server: i64) -> Option<String> {
        self.servers
            .iter()
            .find(|s| s.id == server)
            .map(|s| s.user_id.clone())
    }

    /// Plays an item from where it was left off, or from the start.
    fn play_at(&mut self, item: Item, restart: bool, window: &mut Window, cx: &mut Context<Self>) {
        let key = item.key();
        // Episodes: the show on top, the episode beside it; movies: title and year.
        let (title, subtitle): (SharedString, Option<SharedString>) = match &item.show {
            Some(show) => (
                show.clone().into(),
                Some(match item.episode_label() {
                    Some(label) => format!("{label} · {}", item.title).into(),
                    None => item.title.clone().into(),
                }),
            ),
            None => (
                item.title.clone().into(),
                item.year.map(|y| y.to_string().into()),
            ),
        };
        let (episodes, current) = self.episodes_around(&item);
        let playback = Playback {
            title,
            subtitle,
            episodes,
            current,
            prefs: self.track_prefs(item.show.as_deref()),
            quality: None,
            thumb: item.thumb.clone(),
        };
        let start = if restart { 0.0 } else { item.resume_position() };
        eprintln!("[play] {} ({key}) from {start:.1}s", item.display_title());
        match &item.source {
            Source::Local { path, .. } => {
                if !path.exists() {
                    self.error = Some(format!("{} is not available", path.display()).into());
                    cx.notify();
                    return;
                }
                let url = path.to_string_lossy().into_owned();
                self.start_player(
                    url,
                    None,
                    playback,
                    start,
                    Some(item.source.clone()),
                    window,
                    cx,
                );
            }
            Source::Jellyfin { server, id } => {
                let Some(client) = self.client(*server) else {
                    self.error = Some("That server is no longer signed in.".into());
                    cx.notify();
                    return;
                };
                let quality = self.quality_for(*server);
                let session = PlaySession {
                    id: new_session_id(),
                    transcoding: quality.transcodes(),
                };
                let url = stream_url(&client, id, &session, quality);
                let header = client.stream_header();
                let playback = Playback {
                    quality: Some(quality),
                    ..playback
                };
                self.start_player(
                    url,
                    header,
                    playback,
                    start,
                    Some(item.source.clone()),
                    window,
                    cx,
                );
                if let Some(active) = &mut self.player {
                    active.session = Some(session.clone());
                    self.report(
                        &item.source,
                        PlaybackReport::Started,
                        start,
                        Some(session),
                        cx,
                    );
                    self.load_server_extras(client, *server, id.clone(), cx);
                }
            }
        }
        if let Some(active) = &mut self.player {
            active.show = item.show.clone();
        }
    }

    /// The show's episodes in watching order (seasons in order, specials
    /// last) and where `item` sits among them. Empty for movies.
    fn episodes_around(&self, item: &Item) -> (Vec<Episode>, Option<usize>) {
        let Some(show) = &item.show else {
            return (Vec::new(), None);
        };
        let mut items: Vec<&Item> = self
            .items()
            .filter(|i| i.show.as_ref() == Some(show))
            .collect();
        items.sort_by_key(|i| i.watch_order());
        let key = item.key();
        let current = items.iter().position(|i| i.key() == key);
        let episodes = items
            .into_iter()
            .map(|i| Episode {
                key: i.key(),
                season: i.season.unwrap_or(0),
                number: i.episode,
                title: i.title.clone(),
                watched: i.watched,
                progress: i.progress(),
                thumb: i.thumb.clone(),
            })
            .collect();
        (episodes, current)
    }

    /// Remembered track choices: the show's own, then the general ones.
    fn track_prefs(&self, show: Option<&str>) -> TrackPrefs {
        let load = |scope: &str| self.db.track_prefs(scope).unwrap_or_default();
        let general = load(GENERAL_PREFS);
        match show {
            Some(show) => load(&format!("show:{show}")).or(general),
            None => general,
        }
    }

    /// Where a track choice made now is remembered: for the current show
    /// (if it's an episode) and as the general preference.
    fn pref_scopes(&self) -> Vec<String> {
        let show = self.player.as_ref().and_then(|p| p.show.clone());
        let mut scopes = vec![GENERAL_PREFS.to_string()];
        scopes.extend(show.map(|s| format!("show:{s}")));
        scopes
    }

    /// The streaming quality chosen for a server (Original by default).
    fn quality_for(&self, server: i64) -> Quality {
        self.db
            .setting(&format!("quality:{server}"))
            .ok()
            .flatten()
            .map(|v| Quality::from_setting(&v))
            .unwrap_or(Quality::Original)
    }

    fn set_quality(&mut self, server: i64, quality: Quality) {
        if let Err(e) = self
            .db
            .set_setting(&format!("quality:{server}"), &quality.to_setting())
        {
            eprintln!("[library] saving quality: {e}");
        }
    }

    /// Switches the playing Jellyfin video to another quality, keeping the
    /// position: a new play session replaces the old one on the server.
    fn change_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        let Some(active) = &self.player else {
            return;
        };
        let Some(Source::Jellyfin { server, id }) = active.source.clone() else {
            return;
        };
        self.set_quality(server, quality);
        let Some(client) = self.client(server) else {
            return;
        };
        let Some(active) = &mut self.player else {
            return;
        };
        let old = active.session.clone();
        let session = PlaySession {
            id: new_session_id(),
            transcoding: quality.transcodes(),
        };
        let url = stream_url(&client, &id, &session, quality);
        let position = active.view.read(cx).position();
        let subtitles = active
            .playback_info
            .as_ref()
            .map(|i| i.subtitle_files(client.base(), &id, session.transcoding))
            .unwrap_or_default();
        active.view.update(cx, |player, cx| {
            player.reload(&url, position, subtitles, quality, cx)
        });
        active.session = Some(session.clone());
        eprintln!("[jellyfin] quality {} ({})", quality.label(), session.id);
        // The old session stops (ending its transcode) and the new one starts.
        let source = Source::Jellyfin { server, id };
        self.queue_report(&source, PlaybackReport::Stopped, position, old, false, cx);
        self.queue_report(
            &source,
            PlaybackReport::Started,
            position,
            Some(session),
            false,
            cx,
        );
    }

    fn saved_volume(&self) -> f64 {
        self.db
            .setting(VOLUME_SETTING)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100.0)
    }

    /// Asks the server for what it knows beyond the file itself: subtitle
    /// files stored beside the video, and intro/credits markers. The player
    /// takes them once the video has loaded.
    fn load_server_extras(
        &mut self,
        client: Client,
        server: i64,
        item_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(user_id) = self
            .servers
            .iter()
            .find(|s| s.id == server)
            .map(|s| s.user_id.clone())
        else {
            return;
        };
        let source = Source::Jellyfin {
            server,
            id: item_id.clone(),
        };
        let base = client.base().to_string();
        let (client_for_lookup, user_id_for_lookup, source_for_lookup) =
            (client.clone(), user_id.clone(), source.clone());
        cx.spawn(async move |this, cx| {
            let fetch_id = item_id.clone();
            let (info, segments) = cx
                .background_executor()
                .spawn(async move {
                    (
                        client.playback_info(&user_id, &fetch_id),
                        client.media_segments(&fetch_id),
                    )
                })
                .await;
            let info = info
                .map_err(|e| eprintln!("[jellyfin] listing subtitles: {e:#}"))
                .ok();
            // Older servers have no segments API; that's fine.
            let segments = segments.unwrap_or_default();
            this.update(cx, |this, cx| {
                // Only for the video they were fetched for.
                if let Some(active) = &mut this.player
                    && active.source.as_ref() == Some(&source)
                {
                    let transcoding = active.session.as_ref().is_some_and(|s| s.transcoding);
                    let subtitles = info
                        .as_ref()
                        .map(|i| i.subtitle_files(&base, &item_id, transcoding))
                        .unwrap_or_default();
                    eprintln!(
                        "[jellyfin] {} subtitle file(s), {} segment(s)",
                        subtitles.len(),
                        segments.len()
                    );
                    active.playback_info = info;
                    active.view.update(cx, |player, _| {
                        player.add_external_subtitles(subtitles);
                        player.add_segments(SegmentSource::Server, segments);
                    });
                }
            })
            .ok();
        })
        .detach();
        self.load_online_segments(client_for_lookup, user_id_for_lookup, source_for_lookup, cx);
    }

    /// Intro, recap and credits times from TheIntroDB and IntroDB, by the
    /// show's (or movie's) ids on the server. They fill in whatever kinds
    /// the server itself has no markers for.
    fn load_online_segments(
        &mut self,
        client: Client,
        user_id: String,
        source: Source,
        cx: &mut Context<Self>,
    ) {
        let Source::Jellyfin { id, .. } = &source else {
            return;
        };
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let segments = cx
                .background_executor()
                .spawn(async move {
                    let lookup = client.intro_lookup(&user_id, &id)?;
                    let segments = introdb::fetch(&lookup);
                    eprintln!("[segments] {} from the intro databases", segments.len());
                    anyhow::Ok(segments)
                })
                .await;
            let segments = match segments {
                Ok(s) => s,
                Err(e) => return eprintln!("[segments] looking up ids: {e:#}"),
            };
            this.update(cx, |this, cx| {
                if let Some(active) = &mut this.player
                    && active.source.as_ref() == Some(&source)
                {
                    active.view.update(cx, |player, _| {
                        player.add_segments(SegmentSource::Online, segments);
                    });
                }
            })
            .ok();
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn start_player(
        &mut self,
        url: String,
        http_header: Option<String>,
        playback: Playback,
        start: f64,
        source: Option<Source>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let video = match player::open_video(window, &url, start, http_header.as_deref()) {
            Ok(video) => video,
            Err(e) => {
                self.error = Some(format!("Couldn't play {}: {e}", playback.title).into());
                cx.notify();
                return;
            }
        };
        self.error = None;
        let volume = self.saved_volume();
        if self.media.is_none() && crate::test_window::media_controls() {
            match MediaSession::new(window) {
                Ok(media) => {
                    media.set_skip_buttons(self.media_skip_buttons());
                    self.media = Some(Rc::new(media));
                }
                Err(e) => eprintln!("[media] system media controls unavailable: {e:#}"),
            }
        }
        let media = self.media.clone();
        // Previews open the video a second time: not for transcoded streams,
        // which are for connections too slow for that.
        let previews = playback
            .quality
            .is_none_or(|q| !q.transcodes())
            .then(|| player::Previews::start(&url, http_header.as_deref(), self.paths.previews()));
        let view = cx.new(|cx| {
            let mut player = Player::new(video, playback, volume, media, window, cx);
            if let Some(previews) = previews {
                player.set_previews(previews);
            }
            player
        });
        let subscription = cx.subscribe_in(&view, window, |this, _, event, window, cx| {
            this.on_player_event(event, window, cx)
        });
        self.player = Some(ActivePlayer {
            view,
            source,
            position: start,
            duration: 0.0,
            show: None,
            session: None,
            playback_info: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn on_player_event(
        &mut self,
        event: &PlayerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PlayerEvent::Progress { position, duration } => {
                let Some(active) = self.player.as_mut() else {
                    return;
                };
                active.position = *position;
                active.duration = *duration;
                match active.source.clone() {
                    Some(Source::Local { id, .. }) => {
                        if let Err(e) = self.db.save_progress(id, *position, *duration) {
                            eprintln!("[library] saving progress: {e}");
                        }
                    }
                    Some(source @ Source::Jellyfin { .. }) => {
                        let session = active.session.clone();
                        self.report(&source, PlaybackReport::Progress, *position, session, cx)
                    }
                    None => {}
                }
            }
            PlayerEvent::Exit => {
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
                self.finish_playback(cx);
                self.focus.focus(window);
                cx.notify();
            }
            PlayerEvent::QualityChosen(quality) => self.change_quality(*quality, cx),
            PlayerEvent::AudioChosen(lang) => {
                for scope in self.pref_scopes() {
                    if let Err(e) = self.db.save_audio_pref(&scope, lang) {
                        eprintln!("[library] saving audio choice: {e}");
                    }
                }
            }
            PlayerEvent::SubtitleChosen(pref) => {
                for scope in self.pref_scopes() {
                    if let Err(e) = self.db.save_subtitle_pref(&scope, pref) {
                        eprintln!("[library] saving subtitle choice: {e}");
                    }
                }
            }
            PlayerEvent::Play(key) => {
                // Straight to the next video; fullscreen stays as it is.
                self.finish_playback(cx);
                self.play_next_now(key, window, cx);
                if self.player.is_none() {
                    if window.is_fullscreen() {
                        window.toggle_fullscreen();
                    }
                    self.focus.focus(window);
                }
                cx.notify();
            }
        }
    }

    /// Ends the current video: saves the volume, records progress, tells the
    /// server, and re-reads the library so watched state shows up.
    fn finish_playback(&mut self, cx: &mut Context<Self>) {
        // Dropping the view shuts mpv down and destroys its window.
        let Some(active) = self.player.take() else {
            return;
        };
        if let Some(media) = &self.media {
            media.hide();
        }
        // Remember the volume for next time, but never start silent.
        let volume = active.view.read(cx).volume();
        if volume >= 1.0
            && let Err(e) = self.db.set_setting(VOLUME_SETTING, &format!("{volume:.0}"))
        {
            eprintln!("[library] saving volume: {e}");
        }
        // The player's own position is the freshest (progress events come
        // every few seconds). A finished video is reported at its full
        // runtime, so the server marks it watched even when it was left
        // during the end credits, before the server's own 90% mark.
        let player = active.view.read(cx);
        let (position, duration, finished) = if player.duration() > 0.0 {
            (player.position(), player.duration(), player.finished())
        } else {
            (active.position, active.duration, false)
        };
        let reported = if finished { duration } else { position };
        match &active.source {
            Some(Source::Local { id, .. }) => {
                if duration > 0.0
                    && let Err(e) = self.db.save_progress(*id, reported, duration)
                {
                    eprintln!("[library] saving progress: {e}");
                }
            }
            Some(source @ Source::Jellyfin { .. }) => {
                // Update the card now; the refresh after "stopped" confirms it.
                if let Some(item) = self.remote.iter_mut().find(|i| &i.source == source) {
                    item.position = if finished { 0.0 } else { position };
                    item.watched |= finished;
                    item.last_played = Some(now());
                }
                eprintln!(
                    "[jellyfin] stopped at {position:.0}/{duration:.0} s{}",
                    if finished { " (watched)" } else { "" }
                );
                self.report(
                    source,
                    PlaybackReport::Stopped,
                    reported,
                    active.session.clone(),
                    cx,
                );
                // The cached library is what the next start shows first.
                if let Source::Jellyfin { server, id } = source {
                    let (cache, id) = (self.paths.library_cache(*server), id.clone());
                    self.queue_task(
                        "updating the library cache",
                        move || jellyfin::cache_play_state(&cache, &id, position, finished, now()),
                        cx,
                    );
                }
            }
            None => {}
        }
        self.reload();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let browsing = !matches!(self.screen, Screen::Settings);
        let modifiers = &event.keystroke.modifiers;
        let plain = !(modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift);
        if browsing && plain {
            let searching = self.search.focus_handle(cx).is_focused(window);
            match keynav::Direction::from_key(key) {
                // From the search field, Down goes into the results.
                Some(keynav::Direction::Down) if searching => {
                    window.focus(&self.focus);
                    self.cursor = None;
                    if self.move_cursor(keynav::Direction::Down, window, cx) {
                        return cx.stop_propagation();
                    }
                }
                Some(direction) if !searching && self.move_cursor(direction, window, cx) => {
                    return cx.stop_propagation();
                }
                _ => {}
            }
            if key == "enter" && !searching {
                if self.activate_cursor(window, cx) {
                    return cx.stop_propagation();
                }
                // A movie's page has no cards: Enter is its Play button.
                if let Screen::Details(item) = self.screen.clone() {
                    self.play(&item, window, cx);
                    return cx.stop_propagation();
                }
            }
        }
        match (&self.screen, key) {
            (Screen::Settings, "escape") if self.quality_menu.is_some() => {
                self.quality_menu = None;
                cx.notify();
            }
            (Screen::Settings, "escape") if self.adding_server => {
                self.adding_server = false;
                self.form.error = None;
                window.focus(&self.focus);
                cx.notify();
            }
            (Screen::Settings, "enter" | "tab") if self.focused_arr(window, cx).is_some() => {
                if let Some(id) = self.focused_arr(window, cx) {
                    if key == "enter" {
                        self.save_arr(id, cx);
                    } else {
                        self.focus_next_arr_field(id, window, cx);
                    }
                }
            }
            (Screen::Settings, "enter") if self.adding_server => self.connect(window, cx),
            (Screen::Settings, "tab") if self.adding_server => self.focus_next_field(window, cx),

            (Screen::Settings, "escape")
            | (Screen::Show(_) | Screen::Details(_), "escape" | "backspace") => {
                self.screen = self.tab.clone();
                window.focus(&self.focus);
                cx.notify();
            }
            (Screen::Home | Screen::Source(_) | Screen::Movies(_) | Screen::Shows(_), "escape") => {
                // Clear the search and leave the field.
                self.search.update(cx, |input, cx| input.set_text("", cx));
                window.focus(&self.focus);
                cx.notify();
            }
            (Screen::Home | Screen::Source(_) | Screen::Movies(_) | Screen::Shows(_), "f")
                if event.keystroke.modifiers.secondary() =>
            {
                window.focus(&self.search.focus_handle(cx));
                cx.notify();
            }
            _ => {}
        }
    }

    // ---- rendering ----

    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id.into())
            .flex_none()
            .px_3()
            .py_1p5()
            .rounded_md()
            .text_sm()
            .bg(rgba(0xffffff14))
            .hover(|s| s.bg(rgba(0xffffff26)))
            .cursor_pointer()
            .child(label.into())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| action(this, window, cx)))
            .into_any_element()
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let heading =
            |text: SharedString| div().text_2xl().font_weight(FontWeight::BOLD).child(text);
        let back = |this: &Self, cx: &mut Context<Self>| {
            this.button("home", "‹ Library", cx, |this, window, cx| {
                this.screen = this.tab.clone();
                window.focus(&this.focus);
                cx.notify();
            })
        };
        let title: AnyElement = match &self.screen {
            Screen::Home | Screen::Source(_) | Screen::Movies(_) | Screen::Shows(_) => {
                self.search_field()
            }
            Screen::Show(_) | Screen::Details(_) => back(self, cx),
            Screen::Settings => div()
                .flex()
                .items_center()
                .gap_3()
                .child(back(self, cx))
                .child(heading("Settings".into()))
                .into_any_element(),
        };
        let status = if self.scanning {
            Some(SharedString::from("Scanning…"))
        } else if !self.loading_servers.is_empty() {
            Some("Loading Jellyfin library…".into())
        } else if self.artwork_left > 0 {
            Some(format!("Downloading artwork ({} left)", self.artwork_left).into())
        } else if self.thumbs_left > 0 {
            Some(format!("Generating thumbnails ({} left)", self.thumbs_left).into())
        } else {
            None
        };
        let can_refresh = !self.roots.is_empty() || !self.servers.is_empty();
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap_4()
            .px_8()
            .py_5()
            .child(div().flex_none().child(title))
            .child(
                // Status and error text give way (truncated) before the
                // tabs or buttons do.
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .justify_end()
                    .items_center()
                    .gap_3()
                    .children(status.map(|s| {
                        div()
                            .min_w(px(0.0))
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(s)
                    }))
                    .children(self.error.clone().map(|e| {
                        div()
                            .min_w(px(0.0))
                            .text_sm()
                            .text_color(rgb(ERROR))
                            .truncate()
                            .child(e)
                    }))
                    .children(can_refresh.then(|| {
                        self.button("refresh", "Refresh", cx, |this, _, cx| this.refresh(cx))
                    }))
                    .children((self.screen != Screen::Settings).then(|| {
                        self.button("settings", "Settings", cx, |this, window, cx| {
                            this.open_settings(this.settings_page, window, cx)
                        })
                    })),
            )
            .into_any_element()
    }

    /// A poster card. Clicking it calls `on_click`; with `on_text_click`,
    /// the title and subtitle below the picture act on their own (like a
    /// link to the show under an episode).
    fn card(
        &self,
        card: Card,
        on_text_click: Option<TextClick>,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let Card {
            id,
            thumb,
            title,
            subtitle,
            progress,
            watched,
            highlight,
        } = card;
        let text_id = ElementId::NamedChild(Box::new(id.clone().into()), "text".into());
        let selected = self.cursor.as_ref() == Some(&id);
        let on_click: CardAction = Rc::new(on_click);
        self.card_actions
            .borrow_mut()
            .insert(id.clone(), on_click.clone());
        let registry = self.card_bounds.clone();
        let key = id.clone();
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
        let art: AnyElement = match thumb {
            Some(path) => img(path)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(placeholder)
                .into_any_element(),
            None => placeholder(),
        };

        div()
            .id(id)
            .relative()
            .flex_none()
            .w(px(CARD_WIDTH))
            .flex()
            .flex_col()
            .gap_2()
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .child(
                // Records where the card is, for the arrow keys.
                canvas(
                    move |b, _, _| registry.borrow_mut().push((key, b)),
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(CARD_HEIGHT))
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(rgb(color))
                    .when(highlight || selected, |d| {
                        d.border_2().border_color(rgb(ACCENT))
                    })
                    .child(art)
                    .children(highlight.then(|| {
                        // Records where the card is, so the page can scroll to it.
                        let bounds = self.focus_bounds.clone();
                        canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                    }))
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
                    .id(text_id)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .h(px(16.0))
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(subtitle.unwrap_or_default()),
                    )
                    .when_some(on_text_click, |d, on_text_click| {
                        d.hover(|s| s.underline()).on_click(cx.listener(
                            move |this, _: &ClickEvent, window, cx| {
                                // Instead of the card's own action.
                                cx.stop_propagation();
                                on_text_click(this, window, cx)
                            },
                        ))
                    }),
            )
            .on_click(
                cx.listener(move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)),
            )
            .into_any_element()
    }

    /// Moves the keyboard cursor; false if there's nothing to move between.
    fn move_cursor(
        &mut self,
        direction: keynav::Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let grid = Grid::of(&self.screen).filter(|_| self.search_query(cx).is_empty());
        if let Some(grid) = grid {
            let ids: Vec<SharedString> = if grid.shows {
                self.shows(&grid)
                    .iter()
                    .map(|s| show_card_id("all", &s.name))
                    .collect()
            } else {
                self.movies(&grid)
                    .iter()
                    .map(|m| item_card_id("movie", m))
                    .collect()
            };
            let columns = browse::columns(window);
            let current = self
                .cursor
                .as_ref()
                .and_then(|c| ids.iter().position(|id| id == c));
            let next = match current {
                // Start at the first card in view.
                None => {
                    let view = self.grid_scroll.0.borrow().base_handle.bounds();
                    self.first_card_in(view)
                        .and_then(|first| ids.iter().position(|id| *id == first))
                        .or((!ids.is_empty()).then_some(0))
                }
                Some(i) => keynav::grid_step(i, ids.len(), columns, direction),
            };
            if let Some(next) = next.filter(|n| *n < ids.len()) {
                self.cursor = Some(ids[next].clone());
                self.grid_scroll
                    .scroll_to_item(next / columns, ScrollStrategy::Top);
            }
            cx.notify();
            return !ids.is_empty();
        }

        let cards = self.card_bounds.borrow().clone();
        if cards.is_empty() {
            return false;
        }
        let rect = |b: &Bounds<Pixels>| keynav::Rect {
            left: f32::from(b.left()),
            top: f32::from(b.top()),
            right: f32::from(b.right()),
            bottom: f32::from(b.bottom()),
        };
        let current = self
            .cursor
            .as_ref()
            .and_then(|c| cards.iter().position(|(id, _)| id == c));
        let next = match current {
            Some(i) => {
                let rects: Vec<keynav::Rect> = cards.iter().map(|(_, b)| rect(b)).collect();
                keynav::nearest(rects[i], &rects, direction)
            }
            None => self
                .first_card_in(self.content_scroll.bounds())
                .and_then(|first| cards.iter().position(|(id, _)| *id == first)),
        };
        if let Some(next) = next {
            let (id, bounds) = cards[next].clone();
            self.cursor = Some(id);
            self.reveal(bounds);
        }
        cx.notify();
        true
    }

    /// The top-left card drawn within `view`, where the cursor starts.
    fn first_card_in(&self, view: Bounds<Pixels>) -> Option<SharedString> {
        self.card_bounds
            .borrow()
            .iter()
            .filter(|(_, b)| {
                b.bottom() > view.top()
                    && b.top() < view.bottom()
                    && b.left() >= view.left() - px(1.0)
            })
            .min_by(|(_, a), (_, b)| {
                (a.top(), a.left())
                    .partial_cmp(&(b.top(), b.left()))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(id, _)| id.clone())
    }

    /// Scrolls the page, and the row the card is in, so `card` is in view.
    fn reveal(&self, card: Bounds<Pixels>) {
        let margin = px(CURSOR_MARGIN);
        let handle = &self.content_scroll;
        let (view, offset, max) = (handle.bounds(), handle.offset(), handle.max_offset());
        let mut y = offset.y;
        if card.top() < view.top() + margin {
            y += view.top() + margin - card.top();
        } else if card.bottom() > view.bottom() - margin {
            y -= card.bottom() - (view.bottom() - margin);
        }
        let y = y.clamp(-max.height, px(0.0));
        if y != offset.y {
            handle.set_offset(point(offset.x, y));
        }
        let center = card.top() + card.size.height / 2.0;
        for row in self.row_scroll.borrow().values() {
            let view = row.bounds();
            if center < view.top() || center > view.bottom() {
                continue;
            }
            let (offset, max) = (row.offset(), row.max_offset());
            let mut x = offset.x;
            if card.left() < view.left() {
                x += view.left() - card.left();
            } else if card.right() > view.right() {
                x -= card.right() - view.right();
            }
            let x = x.clamp(-max.width, px(0.0));
            if x != offset.x {
                row.set_offset(point(x, offset.y));
            }
        }
    }

    /// Enter on the keyboard cursor: what clicking its card does.
    fn activate_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let action = self
            .cursor
            .as_ref()
            .and_then(|c| self.card_actions.borrow().get(c).cloned());
        match action {
            Some(action) => {
                action(self, window, cx);
                true
            }
            None => false,
        }
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
        let key = item.key();
        let card = Card {
            id: item_card_id(prefix, item),
            thumb: item.thumb.clone(),
            title,
            subtitle,
            progress: item.progress(),
            watched: item.watched,
            highlight: !show_context && self.focus_episode.as_ref() == Some(&key),
        };
        // An episode shown with its show's name (Continue Watching, search):
        // the name leads to the show, at this episode.
        let text_click: Option<TextClick> = (show_context && item.is_episode()).then(|| {
            let key = key.clone();
            Box::new(
                move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                    this.open_show_at(&key, cx)
                },
            ) as TextClick
        });
        let open_details = !item.is_episode() && prefix != "continue";
        self.card(card, text_click, cx, move |this, window, cx| {
            if open_details {
                this.open_details(key.clone(), cx)
            } else {
                this.play(&key, window, cx)
            }
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
        let message = if self.scanning || !self.loading_servers.is_empty() {
            "Loading your library…"
        } else if self.roots.is_empty() && self.servers.is_empty() {
            "Connect a Jellyfin server or add a folder with movies or TV shows."
        } else {
            "No videos found yet."
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
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(self.button(
                        "jellyfin-empty",
                        "Connect Jellyfin",
                        cx,
                        |this, window, cx| this.open_connect(window, cx),
                    ))
                    .child(self.button("add-empty", "Add Folder", cx, |this, _, cx| {
                        this.add_folder(cx)
                    })),
            )
            .into_any_element()
    }

    /// The show page of an episode's show, scrolled to that episode, which
    /// is ringed.
    fn open_show_at(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some((show, season)) = self
            .items()
            .find(|i| i.key() == key)
            .and_then(|i| Some((i.show.clone()?, i.season.unwrap_or(0))))
        else {
            return;
        };
        self.open_show(show.clone(), cx);
        // The episode's season is open, so it can be shown.
        self.open_seasons.entry(show).or_default().insert(season);
        self.focus_episode = Some(key.to_string());
        self.focus_bounds.set(None);
        self.focus_scroll = Some(FocusScroll {
            until: Instant::now() + FOCUS_SCROLL_SETTLE,
            applied: None,
        });
    }

    /// Scrolling for the page about to be drawn: a new screen starts at the
    /// top; a show page opened at an episode scrolls to it. The card's
    /// position is only known once it's been drawn, and the page above it
    /// can still grow (the show's details arrive), so the scroll is redone
    /// each frame for a moment, unless the viewer scrolls first.
    fn update_scroll(&mut self, window: &mut Window) {
        if self.scrolled_screen.as_ref() != Some(&self.screen) {
            self.page_generation += 1;
            if let (Some(old), Some(cursor)) = (self.scrolled_screen.take(), self.cursor.take()) {
                self.cursors.retain(|(screen, _)| *screen != old);
                self.cursors.push((old, cursor));
                if self.cursors.len() > 20 {
                    self.cursors.remove(0);
                }
            }
            self.cursor = self
                .cursors
                .iter()
                .find(|(screen, _)| *screen == self.screen)
                .map(|(_, cursor)| cursor.clone());
            self.reveal_cursor = self.cursor.is_some();
            self.scrolled_screen = Some(self.screen.clone());
            self.content_scroll.set_offset(point(px(0.0), px(0.0)));
        }
        if self.reveal_cursor {
            self.reveal_restored_cursor(window);
        }
        let Some(focus) = &mut self.focus_scroll else {
            return;
        };
        let handle = &self.content_scroll;
        let offset = handle.offset();
        let scrolled_away = focus.applied.is_some_and(|y| y != offset.y);
        if Instant::now() >= focus.until || scrolled_away {
            self.focus_scroll = None;
            return;
        }
        window.request_animation_frame();
        let Some(card) = self.focus_bounds.get() else {
            return; // not drawn yet
        };
        let (viewport, max) = (handle.bounds(), handle.max_offset());
        // Room above the card for its season's heading.
        const ABOVE: f32 = 64.0;
        let in_content = card.top() - offset.y;
        let y = (viewport.top() + px(ABOVE) - in_content).clamp(-max.height, px(0.0));
        if y != offset.y {
            handle.set_offset(point(offset.x, y));
        }
        focus.applied = Some(y);
    }

    /// Brings a cursor put back on return into view: in a grid by its
    /// index, on a page once the page has been drawn.
    fn reveal_restored_cursor(&mut self, window: &mut Window) {
        let Some(cursor) = self.cursor.clone() else {
            self.reveal_cursor = false;
            return;
        };
        if let Some(grid) = Grid::of(&self.screen) {
            let columns = browse::columns(window);
            let index = if grid.shows {
                let shows = self.shows(&grid);
                shows
                    .iter()
                    .position(|s| show_card_id("all", &s.name) == cursor)
            } else {
                self.movies(&grid)
                    .iter()
                    .position(|m| item_card_id("movie", m) == cursor)
            };
            if let Some(index) = index {
                self.grid_scroll
                    .scroll_to_item(index / columns, ScrollStrategy::Center);
            }
            self.reveal_cursor = false;
            return;
        }
        if self.bounds_screen.as_ref() != Some(&self.screen) {
            window.request_animation_frame();
            return;
        }
        let card = self
            .card_bounds
            .borrow()
            .iter()
            .find(|(id, _)| *id == cursor)
            .map(|(_, b)| *b);
        if let Some(card) = card {
            self.reveal(card);
        }
        self.reveal_cursor = false;
    }

    fn show_page(&self, show: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut seasons: BTreeMap<u16, Vec<&Item>> = BTreeMap::new();
        for item in self.items().filter(|i| i.show.as_deref() == Some(show)) {
            seasons
                .entry(item.season.unwrap_or(0))
                .or_default()
                .push(item);
        }
        let header = self.show_header(show, cx);
        // Seasons Sonarr could still get join the list in order.
        let sonarr: BTreeMap<u16, &crate::arr::SeasonSummary> = self
            .sonarr_seasons(show)
            .into_iter()
            .filter_map(|s| Some((u16::try_from(s.number).ok()?, s)))
            .collect();
        let mut numbers: Vec<u16> = seasons.keys().chain(sonarr.keys()).copied().collect();
        numbers.sort_unstable();
        numbers.dedup();
        let blocks: Vec<AnyElement> = numbers
            .into_iter()
            .map(|number| {
                let episodes = seasons.remove(&number).unwrap_or_default();
                self.season_block(show, number, episodes, sonarr.get(&number).copied(), cx)
            })
            .collect();
        let list = div()
            .flex()
            .flex_col()
            .gap_2()
            .children(blocks)
            .children(self.sonarr_notes(show));
        vec![header, Self::section("Seasons", list)]
    }

    /// One season on a show's page: a row that opens and closes it (with
    /// what Sonarr says about it), and its episodes when open.
    fn season_block(
        &self,
        show: &str,
        number: u16,
        mut episodes: Vec<&Item>,
        sonarr: Option<&crate::arr::SeasonSummary>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        episodes.sort_by_key(|e| e.episode);
        let have = !episodes.is_empty();
        let open = have
            && self
                .open_seasons
                .get(show)
                .is_some_and(|open| open.contains(&number));
        let title = if number == 0 {
            "Specials".to_string()
        } else {
            format!("Season {number}")
        };
        let unwatched = episodes.iter().filter(|e| !e.watched).count();
        let summary = match (episodes.len(), unwatched) {
            (0, _) => None,
            (1, 0) => Some("1 episode · watched".to_string()),
            (n, 0) => Some(format!("{n} episodes · all watched")),
            (1, _) => Some("1 episode".to_string()),
            (n, u) if u == n => Some(format!("{n} episodes")),
            (n, u) => Some(format!("{n} episodes · {u} unwatched")),
        };
        let (status, buttons) = match sonarr {
            Some(season) => {
                let (text, color, buttons) = self.sonarr_season_controls(show, season, cx);
                (Some((text, color)), buttons)
            }
            None => (None, Vec::new()),
        };
        let toggle_show = show.to_string();
        let label = div()
            .id(SharedString::from(format!("season-{number}")))
            .flex()
            .items_center()
            .gap_3()
            .flex_1()
            .min_w(px(0.0))
            .py_3()
            .when(have, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let open = this.open_seasons.entry(toggle_show.clone()).or_default();
                        let opening = !open.remove(&number);
                        if opening {
                            open.insert(number);
                        }
                        this.fold_count += 1;
                        this.folds.insert(
                            (toggle_show.clone(), number),
                            Fold {
                                opening,
                                started: Instant::now(),
                                id: this.fold_count,
                            },
                        );
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .w(px(14.0))
                    .flex_none()
                    .text_color(rgb(MUTED))
                    .child(if !have {
                        ""
                    } else if open {
                        "▾"
                    } else {
                        "▸"
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .children(summary.map(|s| div().flex_none().text_sm().text_color(rgb(MUTED)).child(s)))
            .children(status.map(|(text, color)| {
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .text_sm()
                    .text_color(rgb(color))
                    .child(if have { format!("· {text}") } else { text })
            }));
        let row = div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .rounded_lg()
            .bg(rgba(0xffffff0a))
            .hover(|s| s.bg(rgba(0xffffff12)))
            .child(label)
            .children(buttons);
        let panel = sonarr.and_then(|s| self.release_panel(show, s.number, cx));
        // Folding: the episodes grow from nothing (or shrink to it) while
        // fading, as one motion over their real height (measured each time
        // they're drawn); a closing season stays drawn until it's folded
        // away. All spacing is inside the folding part, so it ends at zero.
        let key = (show.to_string(), number);
        let fold_time = crate::motion::duration(crate::motion::FOLD);
        let fold = self.folds.get(&key).filter(|_| crate::motion::enabled());
        let closing = fold.is_some_and(|f| !f.opening && f.started.elapsed() < fold_time);
        let height = self
            .season_heights
            .borrow()
            .get(&key)
            .copied()
            .unwrap_or_else(|| {
                // Not drawn yet: a guess, corrected after the first frame.
                let rows = episodes.len().div_ceil(self.columns_hint.get().max(1));
                24.0 + rows as f32 * (CARD_HEIGHT + 56.0)
            });
        let grid = (open || (have && closing)).then(|| {
            let cards = episodes
                .into_iter()
                .map(|e| self.item_card("episode", e, false, cx))
                .collect();
            let heights = self.season_heights.clone();
            let inner = div()
                .relative()
                .pt_2()
                .pb_4()
                .child(Self::grid(cards))
                .child(
                    canvas(
                        move |bounds, _, _| {
                            heights
                                .borrow_mut()
                                .insert(key, f32::from(bounds.size.height));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                );
            let outer = div().overflow_hidden().child(inner);
            match fold {
                Some(fold) => {
                    let opening = fold.opening;
                    outer
                        .with_animation(
                            ("fold", fold.id),
                            Animation::new(fold_time).with_easing(gpui::ease_in_out),
                            move |outer, t| {
                                let shown = if opening { t } else { 1.0 - t };
                                if opening && t >= 1.0 {
                                    outer
                                } else {
                                    outer.max_h(px(height * shown)).opacity(shown)
                                }
                            },
                        )
                        .into_any_element()
                }
                None => outer.into_any_element(),
            }
        });
        div()
            .flex()
            .flex_col()
            .child(row)
            .children(panel.map(|panel| div().pt_2().child(panel)))
            .children(grid)
            .into_any_element()
    }

    fn field(label: &str, input: &Entity<TextInput>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(label.to_string()),
            )
            .child(input.clone())
            .into_any_element()
    }

    /// "Streaming" and a button that opens a menu of quality presets, for
    /// a server in Settings → Library.
    fn quality_picker(&self, server: i64, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let current = self.quality_for(server);
        let open = self.quality_menu == Some(server);
        let mut picker = div().relative().flex().child(
            menu::trigger(
                SharedString::from(format!("quality-{server}")),
                format!("Streaming: {}", current.label()),
                open,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.quality_menu = if open { None } else { Some(server) };
                cx.notify();
            })),
        );
        if open {
            let items = QUALITIES
                .iter()
                .enumerate()
                .map(|(i, &quality)| {
                    menu::item(
                        ("quality-choice", i),
                        quality.menu_label(),
                        quality == current,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_quality(server, quality);
                        this.quality_menu = None;
                        cx.notify();
                    }))
                    .into_any_element()
                })
                .collect();
            picker = picker.child(menu::popup(
                false,
                items,
                window,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.quality_menu = None;
                    cx.notify();
                }),
            ));
        }
        picker.into_any_element()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(player) = &self.player {
            return div()
                .size_full()
                .child(player.view.clone())
                .into_any_element();
        }
        // A library tab that's gone (the server's libraries changed, or the
        // cache from before they were tracked was replaced) falls back to Home.
        let gone = match &self.screen {
            Screen::Movies(_) | Screen::Shows(_) => !self.library_screens().contains(&self.screen),
            Screen::Source(origin) => !self.sources().iter().any(|s| s.origin == *origin),
            _ => false,
        };
        if gone {
            self.screen = Screen::Home;
            self.tab = Screen::Home;
        }
        self.update_scroll(window);
        self.columns_hint.set(browse::columns(window));
        // Refilled as this frame's cards are laid out.
        self.card_bounds.borrow_mut().clear();
        self.card_actions.borrow_mut().clear();
        self.bounds_screen = Some(self.screen.clone());
        let query = self.search_query(cx);
        let on_tab = matches!(
            self.screen,
            Screen::Home | Screen::Source(_) | Screen::Movies(_) | Screen::Shows(_)
        );
        let body: Result<Vec<AnyElement>, AnyElement> = match self.screen.clone() {
            _ if on_tab && !query.is_empty() => Ok(self.search_results(&query, cx)),
            Screen::Home => Ok(self.home(None, cx)),
            Screen::Source(origin) => Ok(self.home(Some(origin), cx)),
            ref screen @ (Screen::Movies(_) | Screen::Shows(_)) => {
                let grid = Grid::of(screen).expect("a library tab");
                Err(self.library_grid(grid, window, cx))
            }
            Screen::Show(name) => Ok(self.show_page(&name, cx)),
            Screen::Details(key) => Ok(self.details_page(&key, cx)),
            Screen::Settings => Err(self.settings_view(window, cx)),
        };
        // Sections scroll as one page; the grids scroll (and virtualize) themselves.
        let content = match body {
            Ok(sections) => {
                let mut page = div()
                    .id("content")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.content_scroll)
                    .px_8()
                    .pb_10()
                    .children(sections);
                // Sideways scrolling (shift+wheel, trackpad) is for the rows.
                page.style().restrict_scroll_to_axis = Some(true);
                page.into_any_element()
            }
            Err(grid) => grid,
        };
        // Each new page fades in, rising a little.
        let content = if crate::motion::enabled() {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .child(content)
                .with_animation(
                    ("page", self.page_generation),
                    Animation::new(crate::motion::duration(crate::motion::PAGE))
                        .with_easing(crate::motion::ease_out),
                    |page, t| {
                        page.opacity(t)
                            .relative()
                            .top(px((1.0 - t) * crate::motion::PAGE_RISE))
                    },
                )
                .into_any_element()
        } else {
            content
        };
        // Settings has its own sidebar; everything else has the library's.
        let main = if self.screen == Screen::Settings {
            content
        } else {
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
            .child(main)
            .into_any_element()
    }
}
