//! Pictures for the seek bar's hover preview. A second, hidden mpv opens the
//! same video, jumps to the hovered time and saves a small snapshot. Each
//! snapshot covers a `STEP`-second slot and is kept for the rest of the
//! video, so hovering over the same place again is instant.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use libmpv2::Mpv;
use libmpv2::events::Event;

/// Seconds of video each snapshot stands for.
const STEP: f64 = 10.0;
/// Width of the snapshots, in pixels.
const WIDTH: u32 = 384;
/// How long to wait for the hidden player to open the video or to seek.
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const SEEK_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Default)]
struct State {
    /// Slots with a snapshot on disk.
    ready: HashSet<u32>,
    /// The slot last asked for, so it's only asked once.
    requested: Option<u32>,
    /// The slot last shown, kept up while the next one loads.
    shown: Option<u32>,
}

pub struct Previews {
    dir: PathBuf,
    requests: Sender<u32>,
    state: Arc<Mutex<State>>,
}

fn slot(time: f64) -> u32 {
    (time.max(0.0) / STEP).round() as u32
}

fn slot_path(dir: &Path, slot: u32) -> PathBuf {
    dir.join(format!("{slot}.jpg"))
}

impl Previews {
    /// Starts the hidden player for `source` (with `http_header` on its
    /// requests). Snapshots go into `dir`, which is emptied first and
    /// removed when this is dropped.
    pub fn start(source: &str, http_header: Option<&str>, dir: PathBuf) -> Self {
        let _ = fs::remove_dir_all(&dir);
        let (requests, receiver) = channel();
        let state = Arc::new(Mutex::new(State::default()));
        let worker = Worker {
            source: source.to_string(),
            http_header: http_header.map(str::to_string),
            dir: dir.clone(),
            state: state.clone(),
        };
        std::thread::Builder::new()
            .name("previews".into())
            .spawn(move || {
                let dir = worker.dir.clone();
                if let Err(e) = worker.run(receiver) {
                    eprintln!("[previews] {e:#}");
                }
                let _ = fs::remove_dir_all(dir);
            })
            .ok();
        Self {
            dir,
            requests,
            state,
        }
    }

    /// The picture to show for `time`: its own snapshot once it's ready,
    /// meanwhile the last one shown (or none). Asks for it if needed.
    pub fn frame_for(&self, time: f64) -> Option<PathBuf> {
        let wanted = slot(time);
        let mut state = self.state.lock().ok()?;
        if state.ready.contains(&wanted) {
            state.shown = Some(wanted);
        } else if state.requested != Some(wanted) {
            state.requested = Some(wanted);
            let _ = self.requests.send(wanted);
        }
        state.shown.map(|s| slot_path(&self.dir, s))
    }
}

struct Worker {
    source: String,
    http_header: Option<String>,
    dir: PathBuf,
    state: Arc<Mutex<State>>,
}

impl Worker {
    fn run(self, requests: Receiver<u32>) -> Result<()> {
        // Nothing happens until the first hover.
        let Ok(mut wanted) = requests.recv() else {
            return Ok(());
        };
        fs::create_dir_all(&self.dir)?;
        let mpv = self.open()?;
        loop {
            // Only the latest request matters.
            while let Ok(newer) = requests.try_recv() {
                wanted = newer;
            }
            let path = slot_path(&self.dir, wanted);
            if !path.exists() {
                match self.snapshot(&mpv, wanted, &path) {
                    Ok(()) => {
                        if let Ok(mut state) = self.state.lock() {
                            state.ready.insert(wanted);
                        }
                    }
                    Err(e) => eprintln!("[previews] at {}s: {e:#}", f64::from(wanted) * STEP),
                }
            }
            match requests.recv() {
                Ok(next) => wanted = next,
                Err(_) => return Ok(()), // the player closed
            }
        }
    }

    fn open(&self) -> Result<Mpv> {
        let mpv = Mpv::with_initializer(|init| {
            init.set_option("vo", "null")?;
            init.set_option("audio", "no")?;
            init.set_option("sid", "no")?;
            init.set_option("sub-auto", "no")?;
            init.set_option("load-scripts", "no")?;
            init.set_option("ytdl", "no")?;
            init.set_option("input-default-bindings", "no")?;
            init.set_option("pause", "yes")?;
            init.set_option("keep-open", "always")?;
            init.set_option("hwdec", "no")?;
            // Keyframes are enough for a preview; decode them cheaply.
            init.set_option("hr-seek", "no")?;
            init.set_option("vd-lavc-skiploopfilter", "all")?;
            init.set_option("vf", format!("scale={WIDTH}:-2").as_str())?;
            // Don't stream ahead while it waits for the next hover.
            init.set_option("cache", "no")?;
            init.set_option("demuxer-readahead-secs", "0")?;
            init.set_option("demuxer-max-bytes", "4MiB")?;
            init.set_option("screenshot-format", "jpg")?;
            init.set_option("screenshot-jpeg-quality", 80i64)?;
            if let Some(header) = &self.http_header {
                init.set_option("http-header-fields", header.as_str())?;
            }
            Ok(())
        })
        .map_err(|e| anyhow!("starting the preview player: {e}"))?;
        mpv.command("loadfile", &[&self.source])
            .map_err(|e| anyhow!("opening the video for previews: {e}"))?;
        wait_for(&mpv, OPEN_TIMEOUT, |e| matches!(e, Event::PlaybackRestart))?;
        Ok(mpv)
    }

    fn snapshot(&self, mpv: &Mpv, slot: u32, path: &Path) -> Result<()> {
        // Events left over from before.
        while mpv.wait_event(0.0).is_some() {}
        let time = format!("{:.3}", f64::from(slot) * STEP);
        mpv.command("seek", &[&time, "absolute+keyframes"])
            .map_err(|e| anyhow!("seeking: {e}"))?;
        wait_for(mpv, SEEK_TIMEOUT, |e| matches!(e, Event::PlaybackRestart))?;
        // Written under another name, then renamed: a half-written file is
        // never shown.
        let partial = path.with_extension("part.jpg");
        let partial_str = partial.to_string_lossy();
        mpv.command("screenshot-to-file", &[&partial_str, "video"])
            .map_err(|e| anyhow!("taking the snapshot: {e}"))?;
        fs::rename(&partial, path)?;
        Ok(())
    }
}

fn wait_for(mpv: &Mpv, timeout: Duration, done: impl Fn(&Event) -> bool) -> Result<()> {
    let until = Instant::now() + timeout;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(anyhow!("timed out"));
        }
        match mpv.wait_event(left.as_secs_f64()) {
            Some(Ok(event)) if done(&event) => return Ok(()),
            Some(Ok(Event::EndFile(_))) => return Err(anyhow!("the video couldn't be opened")),
            Some(Ok(Event::Shutdown)) => return Err(anyhow!("the preview player stopped")),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::slot;

    #[test]
    fn times_round_to_the_nearest_slot() {
        assert_eq!(slot(0.0), 0);
        assert_eq!(slot(4.9), 0);
        assert_eq!(slot(5.0), 1);
        assert_eq!(slot(-3.0), 0);
        assert_eq!(slot(3600.0), 360);
    }
}
