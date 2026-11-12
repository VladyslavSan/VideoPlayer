//! Saving a window's next rendered frame to a PNG file, for tests that run
//! the app in a window nobody sees (the screen can't be captured then).
//! Patch for the video player; see PATCHES.md. Windows only for now: other
//! platforms ignore the request.

use std::path::PathBuf;
use std::sync::Mutex;

static REQUEST: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Saves the next frame any window draws to `path` (RGBA PNG; transparent
/// where the window draws nothing). The window must redraw for it to happen.
pub fn capture_next_frame(path: PathBuf) {
    if let Ok(mut request) = REQUEST.lock() {
        *request = Some(path);
    }
}

/// The pending request, taken by the renderer that fulfils it.
#[allow(dead_code)]
pub(crate) fn take_request() -> Option<PathBuf> {
    REQUEST.lock().ok()?.take()
}
