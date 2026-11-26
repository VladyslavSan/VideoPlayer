//! The seek bar and the volume slider under the mouse: what is being
//! dragged, where the seek bar is scrubbed to, and its hover preview.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, Context, MouseDownEvent, Pixels, Point, Window};

use super::Player;
use super::previews::Previews;

/// Fraction of `bounds`' width at horizontal position `x`, clamped to 0..=1.
pub(super) fn fraction_at(bounds: Bounds<Pixels>, x: Pixels) -> f64 {
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return 0.0;
    }
    (f32::from(x - bounds.origin.x) / width).clamp(0.0, 1.0) as f64
}

/// What a held mouse button is dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Drag {
    Seek,
    Volume,
}

/// The seek bar under the mouse.
pub(super) struct SeekBar {
    /// Where the bar is being dragged to, shown instead of the position.
    scrub: Option<f64>,
    /// Mouse x over the bar, for the time preview.
    hover: Option<Pixels>,
    /// Where the time preview was last shown (drawn there while it fades out).
    preview_x: Pixels,
    /// Pictures for the hover preview, when available.
    pub(super) previews: Option<Previews>,
    /// The bar's laid-out bounds: written during paint, read by mouse handlers.
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl SeekBar {
    /// The time to show: where the bar is dragged to, else `position`.
    pub(super) fn shown(&self, position: f64) -> f64 {
        self.scrub.unwrap_or(position)
    }

    /// The time under `x` in a video `duration` long.
    pub(super) fn time_at(&self, x: Pixels, duration: f64) -> f64 {
        fraction_at(self.bounds.get(), x) * duration
    }

    /// The cell the bar's bounds are written to as it's painted.
    pub(super) fn bounds_cell(&self) -> Rc<Cell<Bounds<Pixels>>> {
        self.bounds.clone()
    }

    pub(super) fn bounds(&self) -> Bounds<Pixels> {
        self.bounds.get()
    }

    /// The mouse moved to `position`: the preview follows it over the bar,
    /// and anywhere while the bar is dragged.
    fn mouse_moved(&mut self, position: Point<Pixels>, dragged: bool) {
        let over = self.bounds.get().contains(&position) || dragged;
        self.hover = over.then_some(position.x);
    }

    /// Where the time preview belongs now, if it should show. The last place
    /// is kept for drawing it as it fades out.
    pub(super) fn update_preview(&mut self, duration: f64) -> bool {
        let preview = self.hover.filter(|_| duration > 0.0);
        if let Some(x) = preview {
            self.preview_x = x;
        }
        preview.is_some()
    }

    pub(super) fn preview_x(&self) -> Pixels {
        self.preview_x
    }
}

/// The control bar's two sliders.
pub(super) struct Sliders {
    drag: Option<Drag>,
    pub(super) seek: SeekBar,
    /// The volume slider's laid-out bounds, as the seek bar's.
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Sliders {
    pub(super) fn new() -> Self {
        Self {
            drag: None,
            seek: SeekBar {
                scrub: None,
                hover: None,
                preview_x: Pixels::ZERO,
                previews: None,
                bounds: Rc::default(),
            },
            volume_bounds: Rc::default(),
        }
    }

    pub(super) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub(super) fn volume_bounds_cell(&self) -> Rc<Cell<Bounds<Pixels>>> {
        self.volume_bounds.clone()
    }

    /// The volume (0 to 100) under `x` on the volume slider.
    fn volume_at(&self, x: Pixels) -> f64 {
        fraction_at(self.volume_bounds.get(), x) * 100.0
    }
}

impl Player {
    pub(super) fn on_seek_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.sliders.drag = Some(Drag::Seek);
        self.scrub_to(event.position);
        self.poke(cx);
    }

    pub(super) fn on_volume_down(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.sliders.drag = Some(Drag::Volume);
        self.set_volume(self.sliders.volume_at(event.position.x));
        self.poke(cx);
    }

    fn scrub_to(&mut self, position: Point<Pixels>) {
        if self.status.duration > 0.0 {
            let time = self.sliders.seek.time_at(position.x, self.status.duration);
            self.sliders.seek.scrub = Some(time);
            self.video.seek_to_keyframe(time);
        }
    }

    /// The mouse moved to `position`: carries a drag on (or ends one whose
    /// button was let go outside the window), and moves the time preview.
    pub(super) fn drag_to(&mut self, position: Point<Pixels>, button_held: bool) {
        match self.sliders.drag {
            Some(_) if !button_held => self.finish_drag(),
            Some(Drag::Seek) => self.scrub_to(position),
            Some(Drag::Volume) => self.set_volume(self.sliders.volume_at(position.x)),
            None => {}
        }
        let dragged = self.sliders.drag == Some(Drag::Seek);
        self.sliders.seek.mouse_moved(position, dragged);
    }

    pub(super) fn finish_drag(&mut self) {
        if self.sliders.drag.take() == Some(Drag::Seek)
            && let Some(time) = self.sliders.seek.scrub.take()
        {
            // Land exactly where the bar was released.
            self.video.seek_to(time);
            self.status.position = time;
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::*;

    fn bar() -> Bounds<Pixels> {
        Bounds::new(point(px(100.0), px(500.0)), size(px(400.0), px(28.0)))
    }

    fn sliders() -> Sliders {
        let sliders = Sliders::new();
        sliders.seek.bounds.set(bar());
        sliders
    }

    #[test]
    fn a_place_on_the_bar_is_a_fraction_of_its_width() {
        assert_eq!(fraction_at(bar(), px(100.0)), 0.0);
        assert_eq!(fraction_at(bar(), px(200.0)), 0.25);
        assert_eq!(fraction_at(bar(), px(500.0)), 1.0);
    }

    #[test]
    fn places_beyond_the_bar_clamp_to_its_ends() {
        assert_eq!(fraction_at(bar(), px(20.0)), 0.0);
        assert_eq!(fraction_at(bar(), px(900.0)), 1.0);
        assert_eq!(fraction_at(Bounds::default(), px(50.0)), 0.0);
    }

    #[test]
    fn the_bar_shows_where_it_is_dragged_to() {
        let mut sliders = sliders();
        assert_eq!(sliders.seek.shown(30.0), 30.0);
        sliders.seek.scrub = Some(sliders.seek.time_at(px(300.0), 1000.0));
        assert_eq!(sliders.seek.shown(30.0), 500.0);
    }

    #[test]
    fn the_preview_follows_the_mouse_over_the_bar_only() {
        let mut sliders = sliders();
        sliders.seek.mouse_moved(point(px(200.0), px(510.0)), false);
        assert!(sliders.seek.update_preview(1000.0));
        assert_eq!(sliders.seek.preview_x(), px(200.0));
        sliders.seek.mouse_moved(point(px(250.0), px(300.0)), false);
        assert!(!sliders.seek.update_preview(1000.0));
        // It fades out where it last was.
        assert_eq!(sliders.seek.preview_x(), px(200.0));
    }

    #[test]
    fn the_preview_follows_a_drag_off_the_bar() {
        let mut sliders = sliders();
        sliders.seek.mouse_moved(point(px(250.0), px(300.0)), true);
        assert!(sliders.seek.update_preview(1000.0));
        assert_eq!(sliders.seek.preview_x(), px(250.0));
    }

    #[test]
    fn there_is_no_preview_before_the_length_is_known() {
        let mut sliders = sliders();
        sliders.seek.mouse_moved(point(px(200.0), px(510.0)), false);
        assert!(!sliders.seek.update_preview(0.0));
    }
}
