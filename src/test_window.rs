//! Test runs (`VIDEO_PLAYER_TEST_WINDOW=1`): the window draws and takes
//! input messages as usual, but is cloaked (hidden by the compositor, as
//! windows on another virtual desktop are), never activates, and stays out
//! of the taskbar and Alt+Tab, so tests don't get
//! in the way of whoever is using the computer. Test runs also stay out of
//! the system's media controls unless `VIDEO_PLAYER_TEST_MEDIA=1`.

use gpui::Window;

pub fn enabled() -> bool {
    std::env::var_os("VIDEO_PLAYER_TEST_WINDOW").is_some_and(|v| v == "1")
}

/// Where a test asks for screenshots (`VIDEO_PLAYER_TEST_CAPTURE`): it
/// writes a name into `request`, and gets `<name>.ui.png` (the app's own
/// drawing, transparent over the video) and, while a video plays,
/// `<name>.video.png`.
pub fn capture_dir() -> Option<std::path::PathBuf> {
    enabled()
        .then(|| std::env::var_os("VIDEO_PLAYER_TEST_CAPTURE"))
        .flatten()
        .map(std::path::PathBuf::from)
}

/// Whether to register with the system's media controls.
pub fn media_controls() -> bool {
    !enabled() || std::env::var_os("VIDEO_PLAYER_TEST_MEDIA").is_some_and(|v| v == "1")
}

/// The native handle of `window`, for `show`.
pub fn handle(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Shows the window cloaked, without activating it (it was opened hidden).
/// It has to stay on a screen: Windows doesn't ask an off-screen window to
/// repaint, and GPUI only draws when asked. If cloaking fails, it goes
/// off-screen instead, where at least it can't be seen.
#[cfg(windows)]
pub fn show(hwnd: isize) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetClientRect, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
        HWND_BOTTOM, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
        SWP_NOMOVE, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW,
    };

    let hwnd = HWND(hwnd as *mut _);
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let style = (style & !(WS_EX_APPWINDOW.0 as isize))
            | WS_EX_TOOLWINDOW.0 as isize
            | WS_EX_NOACTIVATE.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
        let cloak: i32 = 1;
        let cloaked = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &cloak as *const i32 as *const _,
            std::mem::size_of::<i32>() as u32,
        )
        .is_ok();
        // The usual 1280 x 800 content.
        let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
        let (client_width, client_height) = ((1280.0 * scale) as i32, (800.0 * scale) as i32);
        let (mut outer, mut inner) = (RECT::default(), RECT::default());
        let _ = GetWindowRect(hwnd, &mut outer);
        let _ = GetClientRect(hwnd, &mut inner);
        let width = client_width + (outer.right - outer.left) - inner.right;
        let height = client_height + (outer.bottom - outer.top) - inner.bottom;
        if cloaked {
            let flags = SWP_NOACTIVATE | SWP_NOMOVE;
            let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, width, height, flags);
        } else {
            eprintln!("[test] couldn't cloak the window; moving it off-screen");
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN) - width - 200;
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), x, y, width, height, SWP_NOACTIVATE);
        }
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

#[cfg(not(windows))]
pub fn show(_hwnd: isize) {}
