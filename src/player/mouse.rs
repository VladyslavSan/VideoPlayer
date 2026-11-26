//! The mouse over the video: clicks show or hide the controls, moves bring
//! them back and carry slider drags (`seek_bar` has the sliders themselves).

use std::time::Instant;

use gpui::{Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Window};

use super::Player;

impl Player {
    pub(super) fn on_stage_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Clicking the video never pauses it (too easy to do by accident):
        // a click shows or hides the controls, a double-click is fullscreen.
        if self.panels.open().is_some() {
            // A click outside the panel just closes it.
            self.panels.close();
        } else if event.click_count == 2 {
            window.toggle_fullscreen();
        } else if self.controls_visible() {
            // They stay up while paused or while a panel or countdown shows.
            self.activity.hide_controls();
            return cx.notify();
        }
        self.poke(cx);
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.controls_visible() && !self.activity.reveals(event.position, Instant::now()) {
            return;
        }
        let button_held = event.pressed_button == Some(MouseButton::Left);
        self.drag_to(event.position, button_held);
        self.poke(cx);
    }

    pub(super) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Only the end of a slider drag counts as activity; otherwise the
        // release of a click that hid the controls would bring them back.
        if self.sliders.is_dragging() {
            self.finish_drag();
            self.poke(cx);
        }
    }
}
