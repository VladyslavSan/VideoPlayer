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

/// A window that's never shown, for an mpv that renders only snapshots
/// (the seek bar's previews): with a real renderer they get the same
/// Dolby Vision and HDR handling as the video itself. It lives on its own
/// thread, which handles its messages: mpv's window inside it sends some
/// to its parent and waits for the answer.
pub struct HiddenWindow {
    /// The `HWND`, as a number: it's used only as mpv's `wid`.
    hwnd: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

unsafe extern "system" fn hidden_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{PostQuitMessage, WM_DESTROY};
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

impl HiddenWindow {
    pub fn new(width: i32, height: i32) -> Result<Self> {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, MSG, TranslateMessage, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_POPUP,
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("hidden-window".into())
            .spawn(move || {
                let created = (|| -> Result<HWND> {
                    static REGISTER: Once = Once::new();
                    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
                    REGISTER.call_once(|| unsafe {
                        RegisterClassW(&WNDCLASSW {
                            lpfnWndProc: Some(hidden_proc),
                            hInstance: instance,
                            lpszClassName: w!("VideoPlayerHidden"),
                            ..Default::default()
                        });
                    });
                    Ok(unsafe {
                        CreateWindowExW(
                            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                            w!("VideoPlayerHidden"),
                            None,
                            WS_POPUP,
                            0,
                            0,
                            width,
                            height,
                            None,
                            None,
                            Some(instance),
                            None,
                        )?
                    })
                })();
                let ok = created.is_ok();
                let _ = sender.send(created.map(|hwnd| hwnd.0 as isize));
                if !ok {
                    return;
                }
                let mut msg = MSG::default();
                while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            })?;
        let hwnd = receiver
            .recv()
            .map_err(|_| anyhow!("the hidden window's thread stopped"))??;
        Ok(Self {
            hwnd,
            thread: Some(thread),
        })
    }

    /// For mpv's `wid` option.
    pub fn wid(&self) -> i64 {
        self.hwnd as i64
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
