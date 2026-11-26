//! What animates on the page being drawn: late arrivals fading in, loading
//! notes, text changing in place, boxes easing to a new height and cards'
//! pictures dissolving in. The shell and the views that draw its pages share
//! one `Motion`, written while they draw.

use std::cell::{Cell, RefCell};

mod arriving;
mod pictures;
mod resize;

/// The page's animation bookkeeping, started afresh for each page opened.
#[derive(Default)]
pub(super) struct Motion {
    /// Counts the pages opened; keys each page's fade.
    generation: Cell<usize>,
    /// Set while the page being left is drawn.
    leaving: Cell<bool>,
    arrivals: RefCell<arriving::Arrivals>,
    heights: resize::Heights,
    pictures: RefCell<pictures::Pictures>,
}

impl Motion {
    /// A new page: everything on it is new, and it fades in.
    pub(super) fn page_opened(&self) {
        self.restart_page_fade();
        self.arrivals.borrow_mut().page_opened();
        self.heights.page_opened();
    }

    /// Fades the page in again, keeping what it has shown so far.
    pub(super) fn restart_page_fade(&self) {
        self.generation.set(self.generation.get() + 1);
    }

    /// Which page this is, counting from the first: the key of its fade.
    pub(super) fn generation(&self) -> usize {
        self.generation.get()
    }

    /// Draws the page being left: settled, as nothing new fades in on it.
    pub(super) fn drawing_leaving<R>(&self, draw: impl FnOnce() -> R) -> R {
        self.leaving.set(true);
        let drawn = draw();
        self.leaving.set(false);
        drawn
    }

    /// Whether the page being drawn is the one being left.
    pub(super) fn is_leaving(&self) -> bool {
        self.leaving.get()
    }

    /// Whether what's drawn now may animate: motion is on, and the page stays.
    fn animates(&self) -> bool {
        crate::motion::enabled() && !self.leaving.get()
    }
}
