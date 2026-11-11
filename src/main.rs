// Release builds are GUI apps: no console window opens next to the player.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod introdb;
mod jellyfin;
mod library;
mod media_session;
mod player;
mod secrets;
mod shell;
mod ui;
mod video;

use gpui::{
    App, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, prelude::*, px, size,
};
use shell::{Paths, Shell};

actions!(video_player, [Quit]);

fn main() {
    // Optional path or URL to play immediately, bypassing the library.
    let source = std::env::args().nth(1);
    let paths = Paths::resolve().expect("couldn't locate the app data directory");
    #[cfg(not(debug_assertions))]
    log_to_file(&paths.log);

    Application::new()
        .with_assets(ui::icons::Assets)
        .run(move |cx: &mut App| {
            ui::text_input::bind_keys(cx);
            // macOS apps need their own menu, Cmd+Q, and quitting when the
            // window closes; Windows does the equivalent by itself.
            if cfg!(target_os = "macos") {
                cx.on_action(|_: &Quit, cx| cx.quit());
                cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
                cx.set_menus(vec![Menu {
                    name: "Video Player".into(),
                    items: vec![MenuItem::action("Quit Video Player", Quit)],
                }]);
                cx.on_window_closed(|cx| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
                .detach();
            }
            let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Video Player".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    Shell::new(paths.clone(), source.clone(), window, cx)
                        .expect("failed to open the library")
                })
            })
            .expect("failed to open the window");
            cx.activate(true);
        });
}

/// Release builds have no console (and a macOS app's output goes nowhere),
/// so stderr (diagnostics and panic messages) goes to a log file instead.
/// The previous run's log is kept beside it.
#[cfg(not(debug_assertions))]
fn log_to_file(path: &std::path::Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::rename(path, path.with_extension("previous.log"));
    if let Ok(file) = std::fs::File::create(path) {
        redirect_stderr(file);
    }
}

#[cfg(all(not(debug_assertions), windows))]
fn redirect_stderr(file: std::fs::File) {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};

    // Rust looks the handle up on every write, so this redirects eprintln!.
    let handle = HANDLE(file.into_raw_handle());
    unsafe {
        let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
    }
}

#[cfg(all(not(debug_assertions), unix))]
fn redirect_stderr(file: std::fs::File) {
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn dup2(from: i32, to: i32) -> i32;
    }
    // Descriptor 2 now refers to the file; it stays open after `file` closes.
    unsafe {
        dup2(file.as_raw_fd(), 2);
    }
}
