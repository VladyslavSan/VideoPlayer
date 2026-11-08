// Release builds are GUI apps: no console window opens next to the player.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod jellyfin;
mod library;
mod player;
mod secrets;
mod shell;
mod ui;
mod video;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use shell::{Paths, Shell};

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

/// Release builds have no console, so stderr (diagnostics and panic messages)
/// goes to a log file instead. The previous run's log is kept beside it.
#[cfg(not(debug_assertions))]
fn log_to_file(path: &std::path::Path) {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::rename(path, path.with_extension("previous.log"));
    if let Ok(file) = std::fs::File::create(path) {
        // Rust looks the handle up on every write, so this redirects eprintln!.
        let handle = HANDLE(file.into_raw_handle());
        unsafe {
            let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
        }
    }
}
