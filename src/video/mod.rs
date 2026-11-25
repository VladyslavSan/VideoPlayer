//! Native video layer: mpv drawing below the GPUI window's contents. How it
//! gets there is platform-specific (see `win` and `mac`); the rest is mpv.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

use anyhow::{Result, anyhow};
use gpui::Window;
use libmpv2::{Mpv, MpvInitializer};

#[cfg(target_os = "macos")]
use mac::Host;
#[cfg(windows)]
use win::Host;

/// A hidden mpv that only takes snapshots (the seek bar's previews, the
/// library's thumbnails). On Windows it renders with mpv's GPU renderer
/// into a window that's never shown, so snapshots of Dolby Vision and HDR
/// video get the same colour handling as playback. Without a renderer mpv
/// converts frames in software, which knows nothing of Dolby Vision:
/// profile 5 came out green and purple. The window goes after the player.
pub struct Snapshotter {
    pub mpv: Mpv,
    #[cfg(windows)]
    _window: win::HiddenWindow,
}

impl Snapshotter {
    /// `options` sets the rest; snapshots are taken with
    /// `screenshot-to-file <path> video`.
    pub fn new(options: impl FnOnce(&MpvInitializer) -> libmpv2::Result<()>) -> Result<Self> {
        #[cfg(windows)]
        let window = win::HiddenWindow::new(640, 360)?;
        let mpv = Mpv::with_initializer(|init| {
            #[cfg(windows)]
            {
                // Without a usable GPU, software as before.
                init.set_option("vo", "gpu-next,null")?;
                init.set_option("gpu-api", "d3d11")?;
                init.set_option("wid", window.wid())?;
            }
            #[cfg(not(windows))]
            init.set_option("vo", "null")?;
            init.set_option("audio", "no")?;
            init.set_option("config", "no")?;
            set_lua_option(&init, "load-scripts", "no")?;
            set_lua_option(&init, "ytdl", "no")?;
            init.set_option("input-default-bindings", "no")?;
            options(&init)
        })
        .map_err(|e| anyhow!("starting mpv for snapshots: {e}"))?;
        Ok(Self {
            mpv,
            #[cfg(windows)]
            _window: window,
        })
    }
}

/// The options every player instance gets, whatever draws its video.
fn player_options(init: &MpvInitializer, http_header: Option<&str>) -> libmpv2::Result<()> {
    init.set_option("hwdec", "auto-safe")?;
    init.set_option("keep-open", "yes")?;
    // Arrow-key skips land exactly, not on the next keyframe.
    init.set_option("hr-seek", "yes")?;
    set_lua_option(init, "osc", "no")?;
    init.set_option("osd-level", 0i64)?;
    init.set_option("input-default-bindings", "no")?;
    init.set_option("input-vo-keyboard", "no")?;
    init.set_option("input-cursor", "no")?;
    if let Some(header) = http_header {
        init.set_option("http-header-fields", header)?;
    }
    // A connection that drops for a moment is picked up again by
    // itself, from where it was, before the buffer runs out.
    init.set_option(
        "stream-lavf-o",
        "reconnect=1,reconnect_streamed=1,reconnect_on_network_error=1,reconnect_delay_max=10",
    )?;
    if let Ok(path) = std::env::var("VIDEO_PLAYER_MPV_LOG") {
        init.set_option("log-file", path.as_str())?;
    }
    Ok(())
}

/// Opens `source` at `start` seconds. The start goes in as a property, not
/// as an argument of `loadfile`: that command's arguments differ between
/// mpv versions (0.38 added `index` before the options).
fn load_file(mpv: &Mpv, source: &str, start: f64) -> Result<()> {
    let direct = crate::net::direct_url(source);
    mpv.set_property("start", format!("{start:.3}").as_str())
        .and_then(|()| mpv.command("loadfile", &[&direct, "replace"]))
        .map_err(|e| anyhow!("loading {source}: {e}"))
}

/// Sets an option that only exists in an mpv built with Lua (`osc`,
/// `load-scripts`, `ytdl`). Some libmpv builds have no Lua; the option is
/// then moot, so "option not found" is not an error. Any other error is.
pub fn set_lua_option(init: &MpvInitializer, name: &str, value: &str) -> libmpv2::Result<()> {
    ignore_missing_option(init.set_option(name, value))
}

fn ignore_missing_option(result: libmpv2::Result<()>) -> libmpv2::Result<()> {
    match result {
        Err(libmpv2::Error::Raw(code)) if code == libmpv2::mpv_error::OptionNotFound => Ok(()),
        other => other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Subtitle,
}

/// An audio or subtitle track, as mpv reports it in `track-list`.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub kind: TrackKind,
    /// mpv's id, used to select the track (`aid` / `sid`).
    pub id: i64,
    pub title: Option<String>,
    pub lang: Option<String>,
    pub codec: Option<String>,
    pub channels: Option<i64>,
    pub selected: bool,
    pub external: bool,
    pub forced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Intro,
    Recap,
    Preview,
    Credits,
}

/// A marked part of a video (from the server, or from chapter names).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start: f64,
    pub end: f64,
}

/// A subtitle file to load next to the video (Jellyfin's external subtitles).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSubtitle {
    pub url: String,
    pub title: Option<String>,
    pub lang: Option<String>,
}

pub struct VideoSurface {
    // Field order matters. On macOS the host owns mpv's render context,
    // which must be freed before mpv; on Windows mpv must shut down before
    // its window is destroyed.
    #[cfg(target_os = "macos")]
    host: Host,
    mpv: Mpv,
    #[cfg(windows)]
    host: Host,
    last_start: std::cell::Cell<f64>,
    source: std::cell::RefCell<String>,
}

impl VideoSurface {
    /// Starts playing `source` at `start` seconds, below `window`'s
    /// contents. `http_header` (for example a server token) is sent with
    /// every HTTP request mpv makes.
    pub fn new(
        window: &Window,
        source: &str,
        start: f64,
        http_header: Option<&str>,
    ) -> Result<Self> {
        #[cfg(windows)]
        let host = Host::new(window)?;
        let mpv = Mpv::with_initializer(|init| {
            #[cfg(windows)]
            host.configure(&init)?;
            #[cfg(target_os = "macos")]
            Host::configure(&init)?;
            player_options(&init, http_header)?;
            // Developer override, e.g. `ao=null` to keep test runs silent.
            if let Ok(options) = std::env::var("VIDEO_PLAYER_MPV_OPTIONS") {
                for (key, value) in options.split_whitespace().filter_map(|o| o.split_once('=')) {
                    init.set_option(key, value)?;
                }
            }
            Ok(())
        })
        .map_err(|e| anyhow!("initialising mpv: {e}"))?;
        #[cfg(target_os = "macos")]
        let host = Host::new(window, &mpv)?;
        let surface = Self {
            host,
            mpv,
            last_start: std::cell::Cell::new(start),
            source: Default::default(),
        };
        surface.load(source, start)?;
        Ok(surface)
    }

    /// Replaces what's playing with `source`, starting at `start` seconds.
    pub fn load(&self, source: &str, start: f64) -> Result<()> {
        self.last_start.set(start);
        *self.source.borrow_mut() = source.to_string();
        load_file(&self.mpv, source, start)
    }

    /// The source being played, as passed to `load`.
    pub fn path(&self) -> Option<String> {
        self.mpv.get_property("path").ok()
    }

    /// Fits the video to the window, after it was resized.
    pub fn sync_size(&self) {
        self.host.sync_size();
    }

    pub fn position(&self) -> f64 {
        self.mpv.get_property("time-pos").unwrap_or(0.0)
    }

    pub fn duration(&self) -> f64 {
        self.mpv.get_property("duration").unwrap_or(0.0)
    }

    /// True once playback reaches the end (mpv holds the last frame).
    pub fn ended(&self) -> bool {
        self.mpv.get_property("eof-reached").unwrap_or(false)
    }

    /// Nothing open: the last `load` failed (the server couldn't be
    /// reached, say).
    pub fn idle(&self) -> bool {
        self.mpv.get_property("idle-active").unwrap_or(false)
    }

    /// What the last `load` opened (mpv's own `path` is gone when that
    /// failed).
    pub fn source(&self) -> String {
        self.source.borrow().clone()
    }

    /// Where the last `load` started.
    pub fn last_start(&self) -> f64 {
        self.last_start.get()
    }

    pub fn set_pause(&self, paused: bool) {
        let _ = self.mpv.set_property("pause", paused);
    }

    pub fn paused(&self) -> bool {
        self.mpv.get_property("pause").unwrap_or(false)
    }

    pub fn toggle_pause(&self) {
        let _ = self.mpv.set_property("pause", !self.paused());
    }

    pub fn seek_to(&self, seconds: f64) {
        let _ = self
            .mpv
            .command("seek", &[&format!("{seconds:.3}"), "absolute"]);
    }

    /// Fast, approximate seek (nearest keyframe), for live scrubbing.
    pub fn seek_to_keyframe(&self, seconds: f64) {
        let _ = self
            .mpv
            .command("seek", &[&format!("{seconds:.3}"), "absolute+keyframes"]);
    }

    /// Timestamp up to which the stream is buffered, if known.
    pub fn buffered_until(&self) -> Option<f64> {
        self.mpv.get_property("demuxer-cache-time").ok()
    }

    pub fn seek_by(&self, seconds: f64) {
        let _ = self
            .mpv
            .command("seek", &[&format!("{seconds:.3}"), "relative"]);
    }

    /// True once the file is open and its tracks are known.
    pub fn loaded(&self) -> bool {
        self.mpv
            .get_property::<i64>("track-list/count")
            .unwrap_or(0)
            > 0
    }

    /// True once mpv has set up its video output for the current file, so
    /// its window shows frames rather than nothing.
    pub fn video_configured(&self) -> bool {
        self.mpv.get_property("vo-configured").unwrap_or(false)
    }

    /// Stalled waiting for the network.
    pub fn buffering(&self) -> bool {
        self.mpv.get_property("paused-for-cache").unwrap_or(false)
    }

    pub fn tracks(&self) -> Vec<Track> {
        let count: i64 = self.mpv.get_property("track-list/count").unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let text = |field: &str| {
                    self.mpv
                        .get_property::<String>(&format!("track-list/{i}/{field}"))
                        .ok()
                        .filter(|s| !s.is_empty())
                };
                let flag = |field: &str| {
                    self.mpv
                        .get_property::<bool>(&format!("track-list/{i}/{field}"))
                        .unwrap_or(false)
                };
                let kind = match text("type")?.as_str() {
                    "audio" => TrackKind::Audio,
                    "sub" => TrackKind::Subtitle,
                    _ => return None,
                };
                Some(Track {
                    kind,
                    id: self.mpv.get_property(&format!("track-list/{i}/id")).ok()?,
                    title: text("title"),
                    lang: text("lang"),
                    codec: text("codec"),
                    channels: self
                        .mpv
                        .get_property(&format!("track-list/{i}/demux-channel-count"))
                        .ok(),
                    selected: flag("selected"),
                    external: flag("external"),
                    forced: flag("forced"),
                })
            })
            .collect()
    }

    /// Chapter titles and start times, in order.
    pub fn chapters(&self) -> Vec<(String, f64)> {
        let count: i64 = self.mpv.get_property("chapter-list/count").unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let time: f64 = self
                    .mpv
                    .get_property(&format!("chapter-list/{i}/time"))
                    .ok()?;
                let title = self
                    .mpv
                    .get_property::<String>(&format!("chapter-list/{i}/title"))
                    .unwrap_or_default();
                Some((title, time))
            })
            .collect()
    }

    pub fn select_audio(&self, id: i64) {
        let _ = self.mpv.set_property("aid", id);
    }

    /// `None` turns subtitles off.
    pub fn select_subtitle(&self, id: Option<i64>) {
        let _ = match id {
            Some(id) => self.mpv.set_property("sid", id),
            None => self.mpv.set_property("sid", "no"),
        };
    }

    /// Adds a subtitle file without selecting it. Must be called once the
    /// file has loaded; HTTP requests carry the same header as the video.
    pub fn add_subtitle(&self, subtitle: &ExternalSubtitle) -> Result<()> {
        let title = subtitle.title.as_deref().unwrap_or("");
        let lang = subtitle.lang.as_deref().unwrap_or("");
        let url = crate::net::direct_url(&subtitle.url);
        self.mpv
            .command("sub-add", &[&url, "auto", title, lang])
            .map_err(|e| anyhow!("adding subtitle {}: {e}", subtitle.url))
    }

    pub fn volume(&self) -> f64 {
        self.mpv.get_property("volume").unwrap_or(100.0)
    }

    pub fn set_volume(&self, volume: f64) {
        let _ = self.mpv.set_property("volume", volume.clamp(0.0, 100.0));
    }

    pub fn muted(&self) -> bool {
        self.mpv.get_property("mute").unwrap_or(false)
    }

    pub fn set_muted(&self, muted: bool) {
        let _ = self.mpv.set_property("mute", muted);
    }

    pub fn speed(&self) -> f64 {
        self.mpv.get_property("speed").unwrap_or(1.0)
    }

    pub fn set_speed(&self, speed: f64) {
        let _ = self.mpv.set_property("speed", speed);
    }

    /// Subtitle size as a multiplier (1.0 = default).
    pub fn subtitle_scale(&self) -> f64 {
        self.mpv.get_property("sub-scale").unwrap_or(1.0)
    }

    pub fn set_subtitle_scale(&self, scale: f64) {
        let _ = self.mpv.set_property("sub-scale", scale);
    }

    /// Seconds; positive shows subtitles later.
    pub fn subtitle_delay(&self) -> f64 {
        self.mpv.get_property("sub-delay").unwrap_or(0.0)
    }

    pub fn set_subtitle_delay(&self, seconds: f64) {
        let _ = self.mpv.set_property("sub-delay", seconds);
    }

    /// Vertical subtitle position in percent of the window (100 = bottom).
    /// Where subtitles sit, in percent of the height (100 = the bottom).
    /// Fractions move them smoothly where mpv takes them (0.37 and later);
    /// older versions take whole percents.
    pub fn set_subtitle_position(&self, percent: f64) {
        let percent = percent.clamp(0.0, 100.0);
        if self.mpv.set_property("sub-pos", percent).is_err() {
            let _ = self.mpv.set_property("sub-pos", percent.round() as i64);
        }
    }

    /// Saves what's on screen (the video as scaled to the window, with
    /// subtitles) to `path`. For tests, where the screen can't be captured.
    pub fn screenshot(&self, path: &std::path::Path) -> Result<()> {
        self.mpv
            .command("screenshot-to-file", &[&path.to_string_lossy(), "window"])
            .map_err(|e| anyhow!("video screenshot: {e}"))
    }

    /// One-line summary of the active video pipeline, for diagnostics.
    pub fn pipeline(&self) -> String {
        let prop = |name: &str| {
            self.mpv
                .get_property::<String>(name)
                .unwrap_or_else(|_| "?".into())
        };
        format!(
            "vo={} codec={} hwdec={} size={}x{} source={}/{} output={}/{}",
            prop("current-vo"),
            prop("video-codec"),
            prop("hwdec-current"),
            prop("width"),
            prop("height"),
            prop("video-params/primaries"),
            prop("video-params/gamma"),
            prop("video-target-params/primaries"),
            prop("video-target-params/gamma"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libmpv2::{Error, mpv_error};

    /// A player without a window or sound, with the app's own options.
    fn headless(http_header: Option<&str>) -> Mpv {
        Mpv::with_initializer(|init| {
            init.set_option("vo", "null")?;
            init.set_option("ao", "null")?;
            player_options(&init, http_header)
        })
        .expect("mpv starts")
    }

    /// Waits until `source` plays past `start`, and returns the position.
    fn play_from(mpv: &Mpv, source: &str, start: f64, timeout_secs: u64) -> f64 {
        load_file(mpv, source, start).expect("loadfile is accepted");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        while std::time::Instant::now() < until {
            let position: f64 = mpv.get_property("time-pos").unwrap_or(0.0);
            let tracks: i64 = mpv.get_property("track-list/count").unwrap_or(0);
            if tracks > 0 && position >= start {
                return position;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("{source} did not reach {start}s within {timeout_secs}s");
    }

    #[test]
    fn local_clip_plays_from_its_start() {
        let Some((_dir, clip)) = crate::test_clip::make() else {
            return;
        };
        let mpv = headless(None);
        let position = play_from(&mpv, &clip.to_string_lossy(), 2.0, 10);
        assert!((2.0..4.0).contains(&position), "position {position}");
        let duration: f64 = mpv.get_property("duration").unwrap();
        assert!((4.9..5.1).contains(&duration), "duration {duration}");
        // One video track and one audio track.
        assert_eq!(mpv.get_property::<i64>("track-list/count").unwrap(), 2);
    }

    /// Streams from Jellyfin's public demo server, so the HTTP and
    /// `Authorization` header path is covered: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn demo_server_stream_plays_from_its_start() -> Result<()> {
        let mut client =
            crate::jellyfin::Client::new("https://demo.jellyfin.org/stable", "videoplayer-test");
        let auth = client.login("demo", "")?;
        let items = client.library(&auth.user.id)?;
        let movie = items
            .iter()
            .find(|i| i.kind == "Movie")
            .expect("a movie in the demo library");
        let mpv = headless(client.stream_header().as_deref());
        let position = play_from(&mpv, &client.stream_url(&movie.id), 5.0, 90);
        client.logout()?;
        assert!(position >= 5.0, "position {position}");
        Ok(())
    }

    #[test]
    fn only_a_missing_option_is_ignored() {
        assert!(ignore_missing_option(Ok(())).is_ok());
        assert!(ignore_missing_option(Err(Error::Raw(mpv_error::OptionNotFound))).is_ok());
        for code in [mpv_error::OptionError, mpv_error::OptionFormat] {
            assert!(matches!(
                ignore_missing_option(Err(Error::Raw(code))),
                Err(Error::Raw(c)) if c == code
            ));
        }
        assert!(matches!(
            ignore_missing_option(Err(Error::Null)),
            Err(Error::Null)
        ));
    }
}
