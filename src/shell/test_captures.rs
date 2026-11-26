//! Test runs: the app takes its own screenshots on request, so a hidden
//! window can be checked (see `test_window::capture_dir`).

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{Context, Window};

use super::Shell;

impl Shell {
    /// Test runs: screenshots on request, taken by the app itself (see
    /// `test_window::capture_dir`).
    pub(super) fn serve_test_captures(
        &self,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let request = dir.join("request");
                let Ok(name) = fs::read_to_string(&request) else {
                    continue;
                };
                let _ = fs::remove_file(&request);
                let name = name.trim().to_string();
                gpui::frame_capture::capture_next_frame(dir.join(format!("{name}.ui.png")));
                let served = this.update_in(cx, |this, window, cx| {
                    let video = dir.join(format!("{name}.video.png"));
                    this.playback.read(cx).screenshot(&video, cx);
                    window.refresh();
                    cx.notify();
                });
                if served.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}
