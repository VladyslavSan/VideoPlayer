//! Playback screen: the mpv video layer plus GPUI controls drawn above it.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use gpui::{
    AnyElement, Context, EventEmitter, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, SharedString, Subscription, Task, Window, div, prelude::*, rgb, rgba,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;

use crate::video::VideoSurface;

const CONTROLS_TIMEOUT: Duration = Duration::from_millis(2500);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
/// Horizontal distance from the window edge to the seek bar.
const SEEK_INSET: f32 = 40.0;

pub enum PlayerEvent {
    /// Periodic and final position report, for resume.
    Progress { position: f64, duration: f64 },
    /// The user left the player, or the video ended.
    Exit,
}

pub struct Player {
    video: VideoSurface,
    focus: FocusHandle,
    title: SharedString,
    position: f64,
    duration: f64,
    paused: bool,
    exited: bool,
    last_activity: Instant,
    last_report: Instant,
    pipeline: String,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PlayerEvent> for Player {}

fn native_hwnd(window: &Window) -> Result<HWND> {
    let handle = HasWindowHandle::window_handle(window).map_err(|e| anyhow!("{e}"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Ok(HWND(h.hwnd.get() as *mut _)),
        other => Err(anyhow!("unsupported window handle: {other:?}")),
    }
}

/// Creates the native video layer for `window`. Kept separate from
/// `Player::new` so failures surface before the view exists.
pub fn open_video(window: &Window, source: &str, start: f64) -> Result<VideoSurface> {
    VideoSurface::new(native_hwnd(window)?, source, start)
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

impl Player {
    pub fn new(
        video: VideoSurface,
        title: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        video.sync_size();

        let bounds = cx.observe_window_bounds(window, |this, _, _| this.video.sync_size());
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });

        Self {
            video,
            focus,
            title,
            position: 0.0,
            duration: 0.0,
            paused: false,
            exited: false,
            last_activity: Instant::now(),
            last_report: Instant::now(),
            pipeline: String::new(),
            _poll: poll,
            _subscriptions: vec![bounds],
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.position = self.video.position();
        self.duration = self.video.duration();
        self.paused = self.video.paused();
        let pipeline = self.video.pipeline();
        if pipeline != self.pipeline {
            eprintln!("[video] {pipeline}");
            self.pipeline = pipeline;
        }
        if self.video.ended() {
            // Report the full runtime so the item is marked watched.
            self.position = self.duration;
            self.exit(cx);
            return;
        }
        if self.last_report.elapsed() >= PROGRESS_INTERVAL {
            self.report(cx);
        }
        cx.notify();
    }

    fn report(&mut self, cx: &mut Context<Self>) {
        self.last_report = Instant::now();
        if self.duration > 0.0 {
            cx.emit(PlayerEvent::Progress {
                position: self.position,
                duration: self.duration,
            });
        }
    }

    fn exit(&mut self, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.exited = true;
        self.report(cx);
        cx.emit(PlayerEvent::Exit);
    }

    fn poke(&mut self, cx: &mut Context<Self>) {
        self.last_activity = Instant::now();
        cx.notify();
    }

    fn controls_visible(&self) -> bool {
        self.paused || self.last_activity.elapsed() < CONTROLS_TIMEOUT
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "space" => self.video.toggle_pause(),
            "left" => self.video.seek_by(-10.0),
            "right" => self.video.seek_by(10.0),
            "f" => window.toggle_fullscreen(),
            "escape" if window.is_fullscreen() => window.toggle_fullscreen(),
            "escape" | "backspace" => return self.exit(cx),
            _ => return,
        }
        self.poke(cx);
    }

    fn on_seek(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let width = f32::from(window.viewport_size().width) - SEEK_INSET * 2.0;
        let x = f32::from(event.position.x) - SEEK_INSET;
        if width > 0.0 && self.duration > 0.0 {
            let fraction = (x / width).clamp(0.0, 1.0) as f64;
            self.video.seek_to(fraction * self.duration);
        }
        self.poke(cx);
    }

    fn button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgba(0xffffff22))
            .hover(|s| s.bg(rgba(0xffffff44)))
            .cursor_pointer()
            .child(label.into())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    action(this, window, cx);
                    this.poke(cx);
                }),
            )
            .into_any_element()
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .top_6()
            .left_6()
            .right_6()
            .flex()
            .items_center()
            .gap_4()
            .text_color(rgb(0xf4f5f7))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.button("back", "‹ Library", cx, |this, _, cx| this.exit(cx)))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(0x101218c8))
                    .child(self.title.clone()),
            )
            .into_any_element()
    }

    fn controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let progress = if self.duration > 0.0 {
            (self.position / self.duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let play_label = if self.paused { "Play" } else { "Pause" };
        let time = format!(
            "{} / {}",
            format_time(self.position),
            format_time(self.duration)
        );

        div()
            .absolute()
            .bottom_6()
            .left_6()
            .right_6()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded_xl()
            .bg(rgba(0x101218c8))
            .text_color(rgb(0xf4f5f7))
            .text_sm()
            // The bar is opaque to clicks so it doesn't toggle pause underneath.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("seek")
                    .h_2()
                    .w_full()
                    .rounded_full()
                    .bg(rgba(0xffffff33))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_seek))
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(progress))
                            .rounded_full()
                            .bg(rgb(0xf4f5f7)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.button("play", play_label, cx, |this, _, _| {
                        this.video.toggle_pause()
                    }))
                    .child(time)
                    .child(self.button("fullscreen", "Fullscreen", cx, |_, window, _| {
                        window.toggle_fullscreen()
                    })),
            )
            .into_any_element()
    }
}

impl Render for Player {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // No background: transparent pixels are where the video shows through.
        let overlay = if self.controls_visible() {
            vec![self.top_bar(cx), self.controls(cx)]
        } else {
            Vec::new()
        };
        div()
            .id("stage")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| this.poke(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.video.toggle_pause();
                    this.poke(cx);
                }),
            )
            .children(overlay)
    }
}
