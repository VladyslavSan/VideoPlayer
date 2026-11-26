//! Sliders: a value picked along a track by clicking or dragging. A view
//! keeps one `Sliders` for its page: where each track was drawn, which one
//! is being dragged, and values gliding to where they were set from
//! elsewhere. It's a handle, shared with the tracks' mouse handlers.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, Bounds, Canvas, Context, Div, FontWeight, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, SharedString, canvas, div, prelude::*, px, rgb, rgba,
};

use super::{ACCENT, MUTED};
use crate::motion::SWITCH;

/// What a slider runs over: `min` to `max` in steps of `step`.
#[derive(Clone, Copy)]
pub(super) struct Range {
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

impl Range {
    pub(super) fn fraction(self, value: f32) -> f32 {
        ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    pub(super) fn at(self, fraction: f32) -> f32 {
        let value = self.min + fraction.clamp(0.0, 1.0) * (self.max - self.min);
        ((value / self.step).round() * self.step).clamp(self.min, self.max)
    }
}

/// A slider's value, and the one it glides from since it was last set.
struct Glide {
    value: f32,
    from: f32,
    started: Instant,
    /// Counts its glides, to key each one's animation.
    count: u32,
}

/// A knob on its way: where it glides from, and the key of its animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Gliding {
    pub from: f32,
    pub key: u32,
}

#[derive(Default)]
struct State {
    /// Where each slider's track was drawn, to turn the mouse into a value.
    tracks: RefCell<HashMap<&'static str, Bounds<Pixels>>>,
    dragging: Cell<Option<&'static str>>,
    glides: RefCell<HashMap<&'static str, Glide>>,
}

/// A slider with its text, as a row: what `Sliders::row` draws.
pub(super) struct SliderRow {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub value: f32,
    pub range: Range,
    /// Writes the value shown beside the track.
    pub show: fn(f32) -> String,
}

/// A view's sliders: their tracks as drawn, the drag, the glides.
#[derive(Clone, Default)]
pub(super) struct Sliders(Rc<State>);

impl Sliders {
    /// An element that fills slider `id`'s track and notes where it is.
    pub(super) fn track_marker(&self, id: &'static str) -> Canvas<()> {
        let sliders = self.clone();
        canvas(
            move |bounds, _, _| {
                sliders.0.tracks.borrow_mut().insert(id, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// The value under the mouse at `x` on slider `id`'s track, once drawn.
    pub(super) fn value_at(&self, id: &'static str, range: Range, x: Pixels) -> Option<f32> {
        let track = self.0.tracks.borrow().get(id).copied()?;
        Some(range.at((x - track.left()) / track.size.width))
    }

    pub(super) fn start_drag(&self, id: &'static str) {
        self.0.dragging.set(Some(id));
    }

    pub(super) fn end_drag(&self) {
        self.0.dragging.set(None);
    }

    pub(super) fn is_dragging(&self, id: &'static str) -> bool {
        self.0.dragging.get() == Some(id)
    }

    /// Whether slider `id`, showing `value`, is gliding there: it is just
    /// after its value changed other than by dragging it.
    pub(super) fn glide(&self, id: &'static str, value: f32) -> Option<Gliding> {
        let now = Instant::now();
        let dragging = self.is_dragging(id);
        let mut glides = self.0.glides.borrow_mut();
        let glide = glides.entry(id).or_insert(Glide {
            value,
            from: value,
            started: now,
            count: 0,
        });
        if glide.value != value {
            if dragging {
                glide.from = value;
            } else {
                glide.from = glide.value;
                glide.started = now;
                glide.count += 1;
            }
            glide.value = value;
        }
        (glide.from != glide.value && glide.started.elapsed() < SWITCH.time()).then_some(Gliding {
            from: glide.from,
            key: glide.count,
        })
    }

    /// A setting picked along a track: the text on the left, the track and
    /// the value on the right. Click or drag anywhere on the row to `set`.
    pub(super) fn row<V: 'static>(
        &self,
        row: SliderRow,
        cx: &mut Context<V>,
        set: impl Fn(&mut V, f32, &mut Context<V>) + 'static,
    ) -> AnyElement {
        let SliderRow {
            id,
            title,
            description,
            value,
            range,
            show,
        } = row;
        let sliders = self.clone();
        let pick = Rc::new(move |view: &mut V, x: Pixels, cx: &mut Context<V>| {
            if let Some(value) = sliders.value_at(id, range, x) {
                set(view, value, cx);
            }
        });
        let pick_on_move = pick.clone();
        let (pressed, moved, released, released_outside) =
            (self.clone(), self.clone(), self.clone(), self.clone());
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_between()
            .gap_6()
            .px_4()
            .py_3()
            .rounded_lg()
            .bg(rgba(0xffffff0a))
            .child(row_text(title, description))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_4()
                    .child(self.row_track(id, range.fraction(value)))
                    .child(
                        div()
                            .w(px(64.0))
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(show(value)),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                    pressed.start_drag(id);
                    pick(view, event.position.x, cx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
                if moved.is_dragging(id) && event.pressed_button == Some(MouseButton::Left) {
                    pick_on_move(view, event.position.x, cx);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, move |_, _, _| released.end_drag())
            .on_mouse_up_out(MouseButton::Left, move |_, _, _| {
                released_outside.end_drag()
            })
            .into_any_element()
    }

    /// A row's track, filled and with its knob `fraction` of the way along.
    fn row_track(&self, id: &'static str, fraction: f32) -> Div {
        div()
            .relative()
            .w(px(ROW_TRACK))
            .h(px(ROW_KNOB))
            .cursor_pointer()
            .child(self.track_marker(id))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(ROW_KNOB / 2.0 - 2.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(rgba(0xffffff2e)),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(ROW_KNOB / 2.0 - 2.0))
                    .w(px(ROW_TRACK * fraction))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(rgb(ACCENT)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left(px((ROW_TRACK - ROW_KNOB) * fraction))
                    .size(px(ROW_KNOB))
                    .rounded_full()
                    .bg(rgb(0xffffff)),
            )
    }
}

/// How long a row's track is, and how wide its knob.
const ROW_TRACK: f32 = 220.0;
const ROW_KNOB: f32 = 16.0;

/// A row's title over its description. It takes the room the track leaves,
/// so a long description wraps (and the row grows) instead of spilling over.
fn row_text(title: &'static str, description: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap_1()
        .min_w(px(0.0))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(SharedString::from(description)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliders_snap_to_steps() {
        let range = Range {
            min: 40.0,
            max: 1500.0,
            step: 10.0,
        };
        assert_eq!(range.at(0.0), 40.0);
        assert_eq!(range.at(1.0), 1500.0);
        assert_eq!(range.at(0.1234) % 10.0, 0.0);
        assert!((range.fraction(range.at(0.5)) - 0.5).abs() < 0.01);
    }

    #[test]
    fn a_value_set_from_elsewhere_glides_from_the_old_one() {
        let sliders = Sliders::default();
        assert_eq!(sliders.glide("knob", 2.0), None);
        let gliding = sliders.glide("knob", 5.0);
        assert_eq!(gliding, Some(Gliding { from: 2.0, key: 1 }));
        // Still gliding when drawn again, under the same key.
        assert_eq!(sliders.glide("knob", 5.0), gliding);
    }

    #[test]
    fn a_dragged_slider_follows_the_mouse_without_gliding() {
        let sliders = Sliders::default();
        sliders.glide("knob", 2.0);
        sliders.start_drag("knob");
        assert_eq!(sliders.glide("knob", 5.0), None);
        sliders.end_drag();
        assert_eq!(sliders.glide("knob", 5.0), None);
        // Another slider's drag doesn't stop this one gliding.
        sliders.start_drag("other");
        assert!(sliders.glide("knob", 7.0).is_some());
    }
}
