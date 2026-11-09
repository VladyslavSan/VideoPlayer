//! Native video layer: mpv drawing below the GPUI window's contents. How it
//! gets there is platform-specific (see `win` and `mac`); the rest is mpv.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod win;

use anyhow::{Result, anyhow};
use gpui::Window;
use libmpv2::Mpv;

#[cfg(target_os = "macos")]
use mac::Host;
#[cfg(windows)]
use win::Host;

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
            init.set_option("hwdec", "auto-safe")?;
            init.set_option("keep-open", "yes")?;
            // Arrow-key skips land exactly, not on the next keyframe.
            init.set_option("hr-seek", "yes")?;
            init.set_option("osc", "no")?;
            init.set_option("osd-level", 0i64)?;
            init.set_option("input-default-bindings", "no")?;
            init.set_option("input-vo-keyboard", "no")?;
            init.set_option("input-cursor", "no")?;
            if let Some(header) = http_header {
                init.set_option("http-header-fields", header)?;
            }
            if let Ok(path) = std::env::var("VIDEO_PLAYER_MPV_LOG") {
                init.set_option("log-file", path.as_str())?;
            }
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
        let surface = Self { host, mpv };
        surface.load(source, start)?;
        Ok(surface)
    }

    /// Replaces what's playing with `source`, starting at `start` seconds.
    pub fn load(&self, source: &str, start: f64) -> Result<()> {
        let start = format!("start={start:.3}");
        self.mpv
            .command("loadfile", &[source, "replace", "-1", &start])
            .map_err(|e| anyhow!("loading {source}: {e}"))
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
        self.mpv
            .command("sub-add", &[&subtitle.url, "auto", title, lang])
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
    pub fn set_subtitle_position(&self, percent: f64) {
        let _ = self
            .mpv
            .set_property("sub-pos", percent.round().clamp(0.0, 100.0) as i64);
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
