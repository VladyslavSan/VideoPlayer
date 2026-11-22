//! Mouse wheel scrolling that glides to where it's going instead of jumping
//! there a few lines at a time. (Patched in; see PATCHES.md.)
//!
//! A wheel step adds to how far a scrolling element still has to go; each
//! frame it covers part of the rest (eased out), until it's there, hits an
//! end, or something else moves it (a touchpad, a scrollbar, the app).

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use crate::{Pixels, Point, px};

/// How quickly it gets there: about 95% of the way in three of these.
const TIME_CONSTANT: f32 = 0.05;
/// Close enough to stop.
const CLOSE: f32 = 0.5;

type Offset = Rc<RefCell<Point<Pixels>>>;

struct Glide {
    offset: Weak<RefCell<Point<Pixels>>>,
    /// Still to go.
    remaining: Point<Pixels>,
    last_step: Option<Instant>,
    /// Where the last step left it, after clamping.
    left_at: Option<Point<Pixels>>,
}

thread_local! {
    static GLIDES: RefCell<HashMap<usize, Glide>> = RefCell::new(HashMap::new());
}

fn key(offset: &Offset) -> usize {
    Rc::as_ptr(offset) as usize
}

/// A wheel step for the element scrolled by `offset`.
pub(crate) fn add(offset: &Offset, delta: Point<Pixels>) {
    GLIDES.with_borrow_mut(|glides| {
        let fresh = || Glide {
            offset: Rc::downgrade(offset),
            remaining: Point::default(),
            last_step: None,
            left_at: None,
        };
        let glide = glides.entry(key(offset)).or_insert_with(fresh);
        // The same address may now belong to another element.
        if !glide.offset.upgrade().is_some_and(|o| Rc::ptr_eq(&o, offset)) {
            *glide = fresh();
        }
        // Reversing drops what was left going the other way.
        if glide.remaining.y.0 * delta.y.0 < 0. {
            glide.remaining.y = px(0.);
        }
        if glide.remaining.x.0 * delta.x.0 < 0. {
            glide.remaining.x = px(0.);
        }
        glide.remaining += delta;
    });
}

/// Moves `current` (the offset of `offset`) on by this frame's part, before
/// it's clamped. True if it's gliding: then call [`settle`] after clamping.
pub(crate) fn step(offset: &Offset, current: &mut Point<Pixels>) -> bool {
    GLIDES.with_borrow_mut(|glides| {
        let key = key(offset);
        let Some(glide) = glides.get_mut(&key) else {
            return false;
        };
        let moved_elsewhere = glide.left_at.is_some_and(|at| at != *current);
        if moved_elsewhere || !glide.offset.upgrade().is_some_and(|o| Rc::ptr_eq(&o, offset)) {
            glides.remove(&key);
            return false;
        }
        let now = Instant::now();
        let dt = glide
            .last_step
            .map_or(Duration::from_millis(16), |last| now - last)
            .as_secs_f32()
            .clamp(0.001, 0.1);
        glide.last_step = Some(now);
        let part = 1. - (-dt / TIME_CONSTANT).exp();
        let mut moved = glide.remaining * part;
        if (glide.remaining.x - moved.x).0.abs() < CLOSE
            && (glide.remaining.y - moved.y).0.abs() < CLOSE
        {
            moved = glide.remaining;
        }
        glide.remaining -= moved;
        *current += moved;
        // Where it'd be unclamped; `settle` sees what clamping took off.
        glide.left_at = Some(*current);
        true
    })
}

/// After clamping: stops at an end. True if there's more to go next frame.
pub(crate) fn settle(offset: &Offset, clamped: Point<Pixels>) -> bool {
    GLIDES.with_borrow_mut(|glides| {
        let key = key(offset);
        let Some(glide) = glides.get_mut(&key) else {
            return false;
        };
        if let Some(wanted) = glide.left_at {
            if wanted.x != clamped.x {
                glide.remaining.x = px(0.);
            }
            if wanted.y != clamped.y {
                glide.remaining.y = px(0.);
            }
        }
        glide.left_at = Some(clamped);
        if glide.remaining.x.0.abs() < f32::EPSILON && glide.remaining.y.0.abs() < f32::EPSILON {
            glides.remove(&key);
            false
        } else {
            true
        }
    })
}
