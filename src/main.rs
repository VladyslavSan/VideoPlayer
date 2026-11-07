mod jellyfin;
mod library;
mod player;
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

    Application::new().run(move |cx: &mut App| {
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
