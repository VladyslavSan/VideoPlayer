//! Windows: mpv renders into a child window of the GPUI window. GPUI's
//! DirectComposition tree is topmost, so the UI is drawn above it without a
//! second top-level window.

use std::sync::Once;

use anyhow::{Result, anyhow};
use gpui::Window;
use libmpv2::MpvInitializer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
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
pub struct Host {
    hwnd: HWND,
    parent: HWND,
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn parent_hwnd(window: &Window) -> Result<HWND> {
    let handle = HasWindowHandle::window_handle(window).map_err(|e| anyhow!("{e}"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Ok(HWND(h.hwnd.get() as *mut _)),
        other => Err(anyhow!("unsupported window handle: {other:?}")),
    }
}

impl Host {
    pub fn new(window: &Window) -> Result<Self> {
        let parent = parent_hwnd(window)?;
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
        Ok(Self { hwnd, parent })
    }

    /// mpv options for drawing into this window.
    pub fn configure(&self, init: &MpvInitializer) -> libmpv2::Result<()> {
        init.set_option("wid", self.hwnd.0 as i64)?;
        init.set_option("vo", "gpu-next")?;
        init.set_option("gpu-api", "d3d11")?;
        init.set_option("target-colorspace-hint", "yes")?;
        // HDR video goes out as HDR, SDR video as SDR. With the display
        // in HDR mode, the default sends everything as PQ, and subtitles
        // blended in PQ lose their antialiasing (a half-covered edge
        // pixel ends up near black), so their edges look jagged.
        init.set_option("target-colorspace-hint-mode", "source")?;
        Ok(())
    }

    /// Fits the video to the parent's client area.
    pub fn sync_size(&self) {
        let mut client = RECT::default();
        unsafe {
            if GetClientRect(self.parent, &mut client).is_ok() {
                let _ = SetWindowPos(
                    self.hwnd,
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
}
