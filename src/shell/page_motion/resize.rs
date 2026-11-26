//! Something whose size changes while it's on screen (a one-line
//! "Loading…" turning into a five-line description) grows or shrinks
//! smoothly to its new height, instead of pushing what's below it down in
//! one frame. Its content is measured every time it's drawn; the box keeps
//! the height it had and eases to the new one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::{AnyElement, IntoElement, SharedString, canvas, div, prelude::*, px};

use super::Motion;
use crate::motion::{self, RESIZE};

/// Less than this is the box's pixel rounding moving as it does, not a
/// new size; following it would restart the easing every frame.
const MOVED: f32 = 2.0;

/// A box's height: easing from `from` to `to` since `started`.
#[derive(Clone, Copy)]
struct Height {
    from: f32,
    to: f32,
    started: Instant,
}

impl Height {
    fn settled(height: f32) -> Self {
        Self {
            from: height,
            to: height,
            started: Instant::now(),
        }
    }

    fn progress(&self) -> f32 {
        let time = RESIZE.time().as_secs_f32();
        (self.started.elapsed().as_secs_f32() / time).min(1.0)
    }

    fn now(&self) -> f32 {
        self.from + (self.to - self.from) * motion::ease_in_out(self.progress())
    }

    fn moving(&self) -> bool {
        self.from != self.to && self.progress() < 1.0
    }
}

/// The page's resizing boxes, by id; a new page starts afresh.
#[derive(Default, Clone)]
pub(super) struct Heights(Rc<RefCell<HashMap<SharedString, Height>>>);

impl Heights {
    pub(super) fn page_opened(&self) {
        self.0.borrow_mut().clear();
    }
}

impl Motion {
    /// `content` in a box that eases to its new height when it changes.
    pub(in crate::shell) fn resizing(
        &self,
        id: impl Into<SharedString>,
        content: AnyElement,
    ) -> AnyElement {
        if !self.animates() {
            return content;
        }
        let id = id.into();
        let heights = self.heights.clone();
        let height = heights.0.borrow().get(&id).map(Height::now);
        let measure = {
            let id = id.clone();
            let heights = heights.clone();
            move |bounds: gpui::Bounds<gpui::Pixels>,
                  window: &mut gpui::Window,
                  _: &mut gpui::App| {
                let natural = f32::from(bounds.size.height);
                let mut heights = heights.0.borrow_mut();
                match heights.get_mut(&id) {
                    None => {
                        heights.insert(id.clone(), Height::settled(natural));
                    }
                    Some(height) if (height.to - natural).abs() > MOVED => {
                        *height = Height {
                            from: height.now(),
                            to: natural,
                            started: Instant::now(),
                        };
                        window.refresh();
                    }
                    // At rest, a hair taller (rounding) so nothing's
                    // clipped; never back, or it could flip every frame.
                    Some(height) if !height.moving() && natural > height.to => {
                        *height = Height::settled(natural);
                        window.refresh();
                    }
                    Some(_) => {}
                }
            }
        };
        let keep_moving = move |_, _, window: &mut gpui::Window, _: &mut gpui::App| {
            if heights.0.borrow().get(&id).is_some_and(Height::moving) {
                window.request_animation_frame();
            }
        };
        div()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when_some(height, |d, height| d.h(px(height)))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(content)
                    .child(canvas(measure, keep_moving).absolute().inset_0()),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settled_box_stays_put() {
        let height = Height::settled(24.0);
        assert_eq!(height.now(), 24.0);
        assert!(!height.moving());
    }

    #[test]
    fn a_new_height_eases_from_the_old() {
        let height = Height {
            from: 24.0,
            to: 120.0,
            started: Instant::now(),
        };
        assert!(height.now() < 30.0);
        assert!(height.moving());
    }
}
