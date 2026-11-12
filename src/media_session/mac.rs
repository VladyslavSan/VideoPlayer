//! macOS: the Now Playing info center and remote commands (headphone
//! buttons, media keys, Control Center), through souvlaki.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use anyhow::{Result, anyhow};
use gpui::Window;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use super::{CommandSender, MediaCommand, NowPlaying};

const SKIP_SECONDS: f64 = 10.0;

struct Info {
    title: String,
    subtitle: Option<String>,
    cover_url: Option<String>,
}

pub struct Controls {
    controls: RefCell<MediaControls>,
    sender: CommandSender,
    attached: Cell<bool>,
    info: RefCell<Option<Info>>,
    /// Duration last sent with the metadata; it's only known after loading.
    duration: Cell<f64>,
}

fn command(event: MediaControlEvent) -> Option<MediaCommand> {
    let seek = |direction: SeekDirection, seconds: f64| match direction {
        SeekDirection::Forward => MediaCommand::SeekBy(seconds),
        SeekDirection::Backward => MediaCommand::SeekBy(-seconds),
    };
    Some(match event {
        MediaControlEvent::Play => MediaCommand::Play,
        MediaControlEvent::Pause => MediaCommand::Pause,
        MediaControlEvent::Toggle => MediaCommand::Toggle,
        MediaControlEvent::Next => MediaCommand::Next,
        MediaControlEvent::Previous => MediaCommand::Previous,
        MediaControlEvent::Seek(direction) => seek(direction, SKIP_SECONDS),
        MediaControlEvent::SeekBy(direction, by) => seek(direction, by.as_secs_f64()),
        MediaControlEvent::SetPosition(MediaPosition(at)) => MediaCommand::SeekTo(at.as_secs_f64()),
        _ => return None,
    })
}

impl Controls {
    pub fn new(_window: &Window, sender: CommandSender) -> Result<Self> {
        let controls = MediaControls::new(PlatformConfig {
            display_name: "Video Player",
            dbus_name: "video_player",
            hwnd: None,
        })
        .map_err(|e| anyhow!("{e}"))?;
        Ok(Self {
            controls: RefCell::new(controls),
            sender,
            attached: Cell::new(false),
            info: RefCell::new(None),
            duration: Cell::new(0.0),
        })
    }

    fn send_metadata(&self) -> Result<()> {
        let info = self.info.borrow();
        let Some(info) = info.as_ref() else {
            return Ok(());
        };
        let duration = self.duration.get();
        self.controls
            .borrow_mut()
            .set_metadata(MediaMetadata {
                title: Some(&info.title),
                artist: info.subtitle.as_deref(),
                cover_url: info.cover_url.as_deref(),
                duration: (duration > 0.0).then(|| Duration::from_secs_f64(duration)),
                ..Default::default()
            })
            .map_err(|e| anyhow!("{e}"))
    }

    pub fn show(&self, now: NowPlaying) -> Result<()> {
        if !self.attached.get() {
            let sender = self.sender.clone();
            self.controls
                .borrow_mut()
                .attach(move |event| {
                    if let Some(command) = command(event) {
                        let _ = sender.send(command);
                    }
                })
                .map_err(|e| anyhow!("{e}"))?;
            self.attached.set(true);
        }
        *self.info.borrow_mut() = Some(Info {
            title: now.title.to_string(),
            subtitle: now.subtitle.map(str::to_string),
            cover_url: now
                .thumb
                .filter(|p| p.is_file())
                .map(|p| format!("file://{}", p.display())),
        });
        self.duration.set(0.0);
        self.send_metadata()
    }

    /// souvlaki can't turn single commands off; the session drops them.
    pub fn set_skip(&self, _previous: bool, _next: bool) -> Result<()> {
        Ok(())
    }

    pub fn update(&self, playing: bool, position: f64, duration: f64) -> Result<()> {
        if (duration - self.duration.get()).abs() > 0.5 {
            self.duration.set(duration);
            self.send_metadata()?;
        }
        let progress = Some(MediaPosition(Duration::from_secs_f64(position)));
        self.controls
            .borrow_mut()
            .set_playback(if playing {
                MediaPlayback::Playing { progress }
            } else {
                MediaPlayback::Paused { progress }
            })
            .map_err(|e| anyhow!("{e}"))
    }

    pub fn hide(&self) -> Result<()> {
        *self.info.borrow_mut() = None;
        let mut controls = self.controls.borrow_mut();
        controls
            .set_playback(MediaPlayback::Stopped)
            .map_err(|e| anyhow!("{e}"))?;
        if self.attached.replace(false) {
            controls.detach().map_err(|e| anyhow!("{e}"))?;
        }
        Ok(())
    }
}
