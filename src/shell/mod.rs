//! App shell: the library screens (local folders and Jellyfin), the Jellyfin
//! sign-in screen, and switching to and from the player.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, Entity, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, ObjectFit, PathPromptOptions, SharedString, Subscription, Window, div, img,
    prelude::*, px, rgb, rgba,
};

use crate::jellyfin::{
    self, Client, ImageRef, PlaySession, PlaybackInfo, PlaybackReport, QUALITIES, Quality, Server,
    discovery,
};
use crate::library::{Db, Item, Source, TrackPrefs, WATCHED_FRACTION, scan, thumbs};
use crate::player::{self, Episode, Playback, Player, PlayerEvent};
use crate::secrets;
use crate::ui::text_input::TextInput;

mod browse;
mod details;
use browse::{Grid, Sort};
use details::Details;

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
    Home,
    Movies,
    Shows,
    Show(String),
    /// A movie's details page, by item key.
    Details(String),
    Connect,
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
    thumbs_running: bool,
    thumbs_left: usize,
    artwork_left: usize,
    error: Option<SharedString>,
    focus: FocusHandle,
    form: ConnectForm,
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
            player: None,
            scanning: false,
            loading_servers: HashSet::new(),
            thumbs_running: false,
            thumbs_left: 0,
            artwork_left: 0,
            error: None,
            focus,
            form,
        };
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
            };
            shell.start_player(source, None, playback, 0.0, None, window, cx);
        }
        Ok(shell)
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
        for server in self.servers.clone() {
            if server.token.is_empty() {
                // Removed from the credential store outside the app.
                self.error = Some(
                    format!(
                        "{}: signed out; sign in again from the Jellyfin page",
                        server.name
                    )
                    .into(),
                );
                continue;
            }
            if !self.loading_servers.insert(server.id) {
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

    /// Tells the server about playback so its resume/watched state follows.
    fn report(
        &mut self,
        source: &Source,
        kind: PlaybackReport,
        position: f64,
        session: Option<PlaySession>,
        cx: &mut Context<Self>,
    ) {
        let (Source::Jellyfin { server, id }, Some(session)) = (source, session) else {
            return;
        };
        let Some(client) = self.client(*server) else {
            return;
        };
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let reported = client.report(kind, &id, position, false, &session);
                    if kind == PlaybackReport::Stopped && session.transcoding {
                        // Don't leave the server transcoding for nobody.
                        if let Err(e) = client.stop_transcoding(&session) {
                            eprintln!("[jellyfin] stopping transcoding: {e:#}");
                        }
                    }
                    reported
                })
                .await;
            if let Err(e) = result {
                eprintln!("[jellyfin] report {kind:?}: {e:#}");
            }
            if kind == PlaybackReport::Stopped {
                this.update(cx, |this, cx| this.refresh_remote(cx)).ok();
            }
        })
        .detach();
    }

    fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Connect;
        self.form.error = None;
        window.focus(&self.form.url.focus_handle(cx));
        if self.form.found.is_empty() && !self.form.searching {
            self.discover(cx);
        }
        cx.notify();
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
                        this.reload();
                        this.screen = Screen::Home;
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
        let account = self
            .servers
            .iter()
            .find(|s| s.id == server_id)
            .map(|s| s.token_account(&self.device_id));
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

    fn play(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.play_at(key, false, window, cx)
    }

    fn play_from_start(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.play_at(key, true, window, cx)
    }

    /// Plays an item from where it was left off, or from the start.
    fn play_at(&mut self, key: &str, restart: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items().find(|i| i.key() == key).cloned() else {
            return;
        };
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
        cx.background_executor()
            .spawn(async move {
                if let Some(old) = old {
                    let _ = client.report(PlaybackReport::Stopped, &id, position, false, &old);
                    if old.transcoding {
                        let _ = client.stop_transcoding(&old);
                    }
                }
                if let Err(e) =
                    client.report(PlaybackReport::Started, &id, position, false, &session)
                {
                    eprintln!("[jellyfin] report Started: {e:#}");
                }
            })
            .detach();
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
                        player.add_segments(segments);
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
        let view = cx.new(|cx| Player::new(video, playback, volume, window, cx));
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
                self.play(key, window, cx);
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
        // Remember the volume for next time, but never start silent.
        let volume = active.view.read(cx).volume();
        if volume >= 1.0
            && let Err(e) = self.db.set_setting(VOLUME_SETTING, &format!("{volume:.0}"))
        {
            eprintln!("[library] saving volume: {e}");
        }
        if let Some(source @ Source::Jellyfin { .. }) = &active.source {
            // Update the card now; the refresh after "stopped" confirms it.
            let finished =
                active.duration > 0.0 && active.position >= active.duration * WATCHED_FRACTION;
            if let Some(item) = self.remote.iter_mut().find(|i| &i.source == source) {
                item.position = if finished { 0.0 } else { active.position };
                item.watched |= finished;
                item.last_played = Some(now());
            }
            self.report(
                source,
                PlaybackReport::Stopped,
                active.position,
                active.session.clone(),
                cx,
            );
        }
        self.reload();
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        match (&self.screen, key) {
            (Screen::Connect, "enter") => self.connect(window, cx),
            (Screen::Connect, "tab") => self.focus_next_field(window, cx),
            (Screen::Connect, "escape")
            | (Screen::Show(_) | Screen::Details(_), "escape" | "backspace") => {
                self.screen = self.tab.clone();
                window.focus(&self.focus);
                cx.notify();
            }
            (Screen::Home | Screen::Movies | Screen::Shows, "escape") => {
                // Clear the search and leave the field.
                self.search.update(cx, |input, cx| input.set_text("", cx));
                window.focus(&self.focus);
                cx.notify();
            }
            (Screen::Home | Screen::Movies | Screen::Shows, "f")
                if event.keystroke.modifiers.control =>
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
            Screen::Home | Screen::Movies | Screen::Shows => self.tabs(cx),
            Screen::Show(_) | Screen::Details(_) => back(self, cx),
            Screen::Connect => div()
                .flex()
                .items_center()
                .gap_3()
                .child(back(self, cx))
                .child(heading("Jellyfin".into()))
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
                            .map(|e| div().text_sm().text_color(rgb(ERROR)).truncate().child(e)),
                    )
                    .children(can_refresh.then(|| {
                        self.button("refresh", "Refresh", cx, |this, _, cx| this.refresh(cx))
                    }))
                    .child(self.button("jellyfin", "Jellyfin", cx, |this, window, cx| {
                        this.open_connect(window, cx)
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
            thumb,
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
            .child(
                div()
                    .h(px(16.0))
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .truncate()
                    .child(subtitle.unwrap_or_default()),
            )
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
        let key = item.key();
        let card = Card {
            id: SharedString::from(format!("{prefix}-{key}")).into(),
            thumb: item.thumb.clone(),
            title,
            subtitle,
            progress: item.progress(),
            watched: item.watched,
        };
        let open_details = !item.is_episode() && prefix != "continue";
        self.card(card, cx, move |this, window, cx| {
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

    fn show_page(&self, show: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut seasons: BTreeMap<u16, Vec<&Item>> = BTreeMap::new();
        for item in self.items().filter(|i| i.show.as_deref() == Some(show)) {
            seasons
                .entry(item.season.unwrap_or(0))
                .or_default()
                .push(item);
        }
        let header = self.show_header(show, cx);
        std::iter::once(header)
            .chain(seasons.into_iter().map(|(season, mut episodes)| {
                episodes.sort_by_key(|e| e.episode);
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
            }))
            .collect()
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

    fn connect_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
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

        let sign_in = div()
            .flex()
            .flex_col()
            .gap_4()
            .w(px(440.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(network_note.map(|n| div().text_sm().text_color(rgb(MUTED)).child(n)))
                    .child(div().flex().flex_wrap().gap_2().children(found)),
            )
            .child(Self::field("Server address", &form.url))
            .child(Self::field("Username", &form.user))
            .child(Self::field("Password", &form.password))
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
                    .children(
                        form.error
                            .clone()
                            .map(|e| div().text_sm().text_color(rgb(ERROR)).child(e)),
                    ),
            );

        let mut sections = vec![Self::section("Sign in to a server", sign_in)];
        if !self.servers.is_empty() {
            let rows: Vec<AnyElement> = self
                .servers
                .iter()
                .map(|server| {
                    let id = server.id;
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(server.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(MUTED))
                                        .child(format!("{} · {}", server.user_name, server.url)),
                                ),
                        )
                        .child({
                            // Click to step through the qualities.
                            let quality = self.quality_for(id);
                            let next = QUALITIES[(QUALITIES
                                .iter()
                                .position(|q| *q == quality)
                                .unwrap_or(0)
                                + 1)
                                % QUALITIES.len()];
                            self.button(
                                SharedString::from(format!("quality-{id}")),
                                format!("Streaming: {}", quality.label()),
                                cx,
                                move |this, _, cx| {
                                    this.set_quality(id, next);
                                    cx.notify();
                                },
                            )
                        })
                        .child(self.button(
                            SharedString::from(format!("sign-out-{id}")),
                            "Sign Out",
                            cx,
                            move |this, _, cx| this.sign_out(id, cx),
                        ))
                        .into_any_element()
                })
                .collect();
            sections.push(Self::section(
                "Signed in",
                div().flex().flex_col().gap_3().children(rows),
            ));
        }
        sections
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
        let query = self.search_query(cx);
        let on_tab = matches!(self.screen, Screen::Home | Screen::Movies | Screen::Shows);
        let body: Result<Vec<AnyElement>, AnyElement> = match self.screen.clone() {
            _ if on_tab && !query.is_empty() => Ok(self.search_results(&query, cx)),
            Screen::Home => Ok(self.home(cx)),
            Screen::Movies => Err(self.library_grid(Grid::Movies, window, cx)),
            Screen::Shows => Err(self.library_grid(Grid::Shows, window, cx)),
            Screen::Show(name) => Ok(self.show_page(&name, cx)),
            Screen::Details(key) => Ok(self.details_page(&key, cx)),
            Screen::Connect => Ok(self.connect_page(cx)),
        };
        // Sections scroll as one page; the grids scroll (and virtualize) themselves.
        let content = match body {
            Ok(sections) => div()
                .id("content")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .px_8()
                .pb_10()
                .children(sections)
                .into_any_element(),
            Err(grid) => grid,
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
            .child(content)
            .into_any_element()
    }
}
