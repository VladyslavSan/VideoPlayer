//! Native video layer: a child window owned by the GPUI window, with mpv
//! rendering into it. GPUI's DirectComposition tree is topmost, so the UI is
//! drawn above this window without a second top-level window.

use std::sync::Once;

use anyhow::{Context as _, Result, anyhow};
use libmpv2::Mpv;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, RegisterClassW, SWP_NOACTIVATE,
    SWP_NOZORDER, SetWindowPos, WINDOW_EX_STYLE, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_DISABLED,
    WS_VISIBLE,
};
use windows::core::w;

/// Child window that hosts mpv. It stays disabled so that mouse input falls
/// through to the GPUI window instead of being swallowed by the video.
struct Host(HWND);

impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn create_host(parent: HWND) -> Result<Host> {
    static REGISTER: Once = Once::new();
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    REGISTER.call_once(|| unsafe {
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(host_proc),
            hInstance: instance,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: w!("VideoPlayerHost"),
            ..Default::default()
        });
    });

    let mut client = RECT::default();
    unsafe { GetClientRect(parent, &mut client)? };
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("VideoPlayerHost"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_DISABLED | WS_CLIPCHILDREN,
            0,
            0,
            client.right,
            client.bottom,
            Some(parent),
            None,
            Some(instance),
            None,
        )?
    };
    Ok(Host(hwnd))
}

pub struct VideoSurface {
    // Field order matters: mpv must shut down before its window is destroyed.
    mpv: Mpv,
    host: Host,
    parent: HWND,
}

impl VideoSurface {
    /// Starts playing `source` at `start` seconds.
    pub fn new(parent: HWND, source: &str, start: f64) -> Result<Self> {
        let host = create_host(parent).context("creating the video host window")?;
        let wid = host.0.0 as i64;
        let mpv = Mpv::with_initializer(|init| {
            init.set_option("wid", wid)?;
            init.set_option("vo", "gpu-next")?;
            init.set_option("gpu-api", "d3d11")?;
            init.set_option("hwdec", "auto-safe")?;
            init.set_option("target-colorspace-hint", "yes")?;
            init.set_option("keep-open", "yes")?;
            // Arrow-key skips land exactly, not on the next keyframe.
            init.set_option("hr-seek", "yes")?;
            init.set_option("osc", "no")?;
            init.set_option("osd-level", 0i64)?;
            init.set_option("input-default-bindings", "no")?;
            init.set_option("input-vo-keyboard", "no")?;
            init.set_option("input-cursor", "no")?;
            if let Ok(path) = std::env::var("VIDEO_PLAYER_MPV_LOG") {
                init.set_option("log-file", path.as_str())?;
            }
            Ok(())
        })
        .map_err(|e| anyhow!("initialising mpv: {e}"))?;
        let start = format!("start={start:.3}");
        mpv.command("loadfile", &[source, "replace", "-1", &start])
            .map_err(|e| anyhow!("loading {source}: {e}"))?;
        Ok(Self { mpv, host, parent })
    }

    /// Fits the video to the parent's client area.
    pub fn sync_size(&self) {
        let mut client = RECT::default();
        unsafe {
            if GetClientRect(self.parent, &mut client).is_ok() {
                let _ = SetWindowPos(
                    self.host.0,
                    None,
                    0,
                    0,
                    client.right,
                    client.bottom,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
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

    pub fn seek_by(&self, seconds: f64) {
        let _ = self
            .mpv
            .command("seek", &[&format!("{seconds:.3}"), "relative"]);
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
