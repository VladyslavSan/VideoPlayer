//! Windows: System Media Transport Controls for the app's window. Bluetooth
//! headphone buttons and keyboard media keys reach the app through these,
//! and the media flyout shows what's playing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow};
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
    SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsButtonPressedEventArgs, SystemMediaTransportControlsDisplayUpdater,
    SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::StorageFile;
use windows::Storage::Streams::RandomAccessStreamReference;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
use windows::core::{HSTRING, factory};

use super::{CommandSender, MediaCommand, NowPlaying};

/// Seconds skipped by the fast-forward and rewind buttons.
const SKIP_SECONDS: f64 = 10.0;

pub struct Controls {
    smtc: SystemMediaTransportControls,
    button_token: i64,
    position_token: i64,
    /// Counts `show` calls, so a thumbnail that finishes loading after the
    /// next video started isn't shown for it.
    shown: Arc<AtomicU64>,
}

fn window_hwnd(window: &Window) -> Result<HWND> {
    let handle = HasWindowHandle::window_handle(window).map_err(|e| anyhow!("{e}"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Ok(HWND(h.hwnd.get() as *mut _)),
        other => Err(anyhow!("unsupported window handle: {other:?}")),
    }
}

fn time_span(seconds: f64) -> TimeSpan {
    TimeSpan {
        Duration: (seconds * 10_000_000.0) as i64,
    }
}

impl Controls {
    pub fn new(window: &Window, sender: CommandSender) -> Result<Self> {
        let interop =
            factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
        let smtc: SystemMediaTransportControls =
            unsafe { interop.GetForWindow(window_hwnd(window)?)? };

        let buttons = sender.clone();
        let button_token = smtc.ButtonPressed(&TypedEventHandler::new(
            move |_,
                  args: windows::core::Ref<
                '_,
                SystemMediaTransportControlsButtonPressedEventArgs,
            >| {
                let command = match args.ok()?.Button()? {
                    SystemMediaTransportControlsButton::Play => MediaCommand::Play,
                    SystemMediaTransportControlsButton::Pause => MediaCommand::Pause,
                    SystemMediaTransportControlsButton::Next => MediaCommand::Next,
                    SystemMediaTransportControlsButton::Previous => MediaCommand::Previous,
                    SystemMediaTransportControlsButton::FastForward => {
                        MediaCommand::SeekBy(SKIP_SECONDS)
                    }
                    SystemMediaTransportControlsButton::Rewind => {
                        MediaCommand::SeekBy(-SKIP_SECONDS)
                    }
                    _ => return Ok(()),
                };
                let _ = buttons.send(command);
                Ok(())
            },
        ))?;
        let position_token = smtc.PlaybackPositionChangeRequested(&TypedEventHandler::new(
            move |_, args: windows::core::Ref<'_, PlaybackPositionChangeRequestedEventArgs>| {
                let position = args.ok()?.RequestedPlaybackPosition()?;
                let _ = sender.send(MediaCommand::SeekTo(position.Duration as f64 / 1e7));
                Ok(())
            },
        ))?;
        smtc.SetIsEnabled(false)?;
        Ok(Self {
            smtc,
            button_token,
            position_token,
            shown: Arc::default(),
        })
    }

    pub fn show(&self, now: NowPlaying) -> Result<()> {
        let generation = self.shown.fetch_add(1, Ordering::SeqCst) + 1;
        let smtc = &self.smtc;
        smtc.SetIsPlayEnabled(true)?;
        smtc.SetIsPauseEnabled(true)?;
        smtc.SetIsFastForwardEnabled(true)?;
        smtc.SetIsRewindEnabled(true)?;
        smtc.SetIsNextEnabled(now.has_next)?;
        smtc.SetIsPreviousEnabled(now.has_previous)?;

        let display = smtc.DisplayUpdater()?;
        display.ClearAll()?;
        display.SetType(MediaPlaybackType::Video)?;
        let video = display.VideoProperties()?;
        video.SetTitle(&HSTRING::from(now.title))?;
        if let Some(subtitle) = now.subtitle {
            video.SetSubtitle(&HSTRING::from(subtitle))?;
        }
        display.Update()?;
        smtc.SetPlaybackStatus(MediaPlaybackStatus::Changing)?;
        smtc.SetIsEnabled(true)?;

        if let Some(thumb) = now.thumb.filter(|p| p.is_file()) {
            // Opening the file is asynchronous; wait for it off the UI thread.
            let path = HSTRING::from(thumb.as_os_str());
            let shown = self.shown.clone();
            std::thread::spawn(move || {
                if let Err(e) = set_thumbnail(&display, &path, || {
                    shown.load(Ordering::SeqCst) == generation
                }) {
                    eprintln!("[media] thumbnail: {e}");
                }
            });
        }
        Ok(())
    }

    pub fn update(&self, playing: bool, position: f64, duration: f64) -> Result<()> {
        self.smtc.SetPlaybackStatus(if playing {
            MediaPlaybackStatus::Playing
        } else {
            MediaPlaybackStatus::Paused
        })?;
        let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
        timeline.SetStartTime(time_span(0.0))?;
        timeline.SetMinSeekTime(time_span(0.0))?;
        timeline.SetEndTime(time_span(duration))?;
        timeline.SetMaxSeekTime(time_span(duration))?;
        timeline.SetPosition(time_span(position.min(duration)))?;
        self.smtc.UpdateTimelineProperties(&timeline)?;
        Ok(())
    }

    pub fn hide(&self) -> Result<()> {
        self.shown.fetch_add(1, Ordering::SeqCst);
        self.smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;
        let display = self.smtc.DisplayUpdater()?;
        display.ClearAll()?;
        display.Update()?;
        self.smtc.SetIsEnabled(false)?;
        Ok(())
    }
}

fn set_thumbnail(
    display: &SystemMediaTransportControlsDisplayUpdater,
    path: &HSTRING,
    current: impl Fn() -> bool,
) -> windows::core::Result<()> {
    let file = StorageFile::GetFileFromPathAsync(path)?.get()?;
    let stream = RandomAccessStreamReference::CreateFromFile(&file)?;
    if current() {
        display.SetThumbnail(&stream)?;
        display.Update()?;
    }
    Ok(())
}

impl Drop for Controls {
    fn drop(&mut self) {
        let _ = self.smtc.RemoveButtonPressed(self.button_token);
        let _ = self
            .smtc
            .RemovePlaybackPositionChangeRequested(self.position_token);
        let _ = self.smtc.SetIsEnabled(false);
    }
}
