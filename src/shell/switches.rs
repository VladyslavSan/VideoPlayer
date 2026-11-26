//! Things with two states that move when flipped: a switch's knob gliding
//! across, an entry folding open, a tooltip fading in under the mouse. A
//! view keeps one `Switches` for its page; it's a handle, so that a row's
//! click handler can note the flip before it tells the view.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, ClickEvent, Context, FontWeight, SharedString, Window,
    div, prelude::*, px, rgb, rgba,
};

use super::{ACCENT, MUTED};
use crate::motion::{self, APPEAR, SWITCH};

/// A tooltip waits this long under the mouse before fading in.
pub(super) const TIP_DELAY: Duration = Duration::from_millis(450);

/// A tooltip `elapsed` after the mouse went onto its thing: nothing for a
/// moment, then fading in.
pub(super) fn tip_shown(elapsed: Duration) -> f32 {
    let t = (elapsed.saturating_sub(TIP_DELAY)).as_secs_f32() / APPEAR.time().as_secs_f32();
    motion::ease_in_out(t.min(1.0))
}

/// Where a tooltip is. A `key` names its animation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Tip {
    Hidden,
    /// Waiting, then fading in.
    Showing {
        key: u32,
    },
    /// Fading out from this visible.
    Leaving {
        from: f32,
        key: u32,
    },
}

/// How often something was flipped, and when last.
struct Flips {
    count: u32,
    last: Instant,
}

/// The tooltip asked for by hovering.
#[derive(Clone, Copy)]
struct TipAsked {
    id: &'static str,
    since: Instant,
    key: u32,
}

/// The tooltip before, fading out from how visible it was.
#[derive(Clone, Copy)]
struct TipLeaving {
    id: &'static str,
    from: f32,
    key: u32,
}

#[derive(Default)]
struct State {
    /// By id; kept across pages, so a switch only moves when it's flipped.
    flips: HashMap<SharedString, Flips>,
    /// Entries that unfold (sound filters in Settings), by id: open or not.
    open: HashMap<&'static str, bool>,
    /// Their content's natural height, measured as they draw.
    fold_heights: HashMap<&'static str, Rc<Cell<f32>>>,
    tip: Option<TipAsked>,
    tip_leaving: Option<TipLeaving>,
    /// Counts tooltips shown and hidden, to key each one's animation.
    tips: u32,
}

impl State {
    fn flipped(&mut self, id: &str) {
        let now = Instant::now();
        let flips = self
            .flips
            .entry(SharedString::from(id.to_string()))
            .or_insert(Flips {
                count: 0,
                last: now,
            });
        flips.count += 1;
        flips.last = now;
    }

    fn moving(&self, id: &str) -> Option<u32> {
        self.flips
            .get(id)
            .filter(|flips| flips.last.elapsed() < SWITCH.time())
            .map(|flips| flips.count)
    }

    fn is_open(&self, id: &'static str) -> bool {
        self.open.get(id).copied().unwrap_or(false)
    }

    fn hover_tip(&mut self, id: &'static str, hovered: bool) {
        let current = self.tip.is_some_and(|tip| tip.id == id);
        if hovered && !current {
            self.hide_tip();
            self.tips += 1;
            self.tip = Some(TipAsked {
                id,
                since: Instant::now(),
                key: self.tips,
            });
        } else if !hovered && current {
            self.hide_tip();
        }
    }

    fn hide_tip(&mut self) {
        let Some(tip) = self.tip.take() else { return };
        let shown = tip_shown(tip.since.elapsed());
        if shown > 0.0 {
            self.tips += 1;
            self.tip_leaving = Some(TipLeaving {
                id: tip.id,
                from: shown,
                key: self.tips,
            });
        }
    }

    fn tip(&self, id: &'static str) -> Tip {
        match (self.tip, self.tip_leaving) {
            (Some(tip), _) if tip.id == id => Tip::Showing { key: tip.key },
            (_, Some(tip)) if tip.id == id => Tip::Leaving {
                from: tip.from,
                key: tip.key,
            },
            _ => Tip::Hidden,
        }
    }
}

/// A setting that's on or off, as a row: what `Switches::row` draws.
pub(super) struct SwitchRow {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub on: bool,
}

/// A view's switches, unfolding entries and tooltips: what moves, since when.
#[derive(Clone, Default)]
pub(super) struct Switches(Rc<RefCell<State>>);

impl Switches {
    /// Notes that `id` changed state just now, so it's drawn moving.
    pub(super) fn flipped(&self, id: &str) {
        self.0.borrow_mut().flipped(id);
    }

    /// For something still on its way across: how many times it was
    /// flipped (to key its animation). None once it's at rest.
    pub(super) fn moving(&self, id: &str) -> Option<u32> {
        self.0.borrow().moving(id)
    }

    pub(super) fn is_open(&self, id: &'static str) -> bool {
        self.0.borrow().is_open(id)
    }

    /// Opens or closes `id`; its fold moves like a switch.
    pub(super) fn toggle_open(&self, id: &'static str) {
        let mut state = self.0.borrow_mut();
        let open = !state.is_open(id);
        state.open.insert(id, open);
        state.flipped(id);
    }

    /// Where the natural height of `id`'s folding content is kept.
    pub(super) fn fold_height(&self, id: &'static str) -> Rc<Cell<f32>> {
        let mut state = self.0.borrow_mut();
        state.fold_heights.entry(id).or_default().clone()
    }

    /// The mouse went onto (`hovered`) or off something with a tooltip.
    pub(super) fn hover_tip(&self, id: &'static str, hovered: bool) {
        self.0.borrow_mut().hover_tip(id, hovered);
    }

    pub(super) fn tip(&self, id: &'static str) -> Tip {
        self.0.borrow().tip(id)
    }

    /// A setting's row: the text on the left, a switch on the right.
    /// Clicking anywhere on it flips the switch, then calls `toggle`.
    pub(super) fn row<V: 'static>(
        &self,
        row: SwitchRow,
        cx: &mut Context<V>,
        toggle: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> AnyElement {
        let SwitchRow {
            id,
            title,
            description,
            on,
        } = row;
        let switches = self.clone();
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
            .hover(|s| s.bg(rgba(0xffffff14)))
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_1()
                    .min_w(px(0.0))
                    .child(div().font_weight(FontWeight::MEDIUM).child(title))
                    .child(div().text_sm().text_color(rgb(MUTED)).child(description)),
            )
            .child(self.switch(id, on))
            .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                switches.flipped(id);
                toggle(view, window, cx);
            }))
            .into_any_element()
    }

    /// The switch itself. Flipped, its knob glides across as its colour
    /// changes; otherwise it's drawn at rest.
    pub(super) fn switch(&self, id: &'static str, on: bool) -> AnyElement {
        let moving = motion::enabled().then(|| self.moving(id)).flatten();
        let Some(flips) = moving else {
            return switch_at(if on { 1.0 } else { 0.0 }).into_any_element();
        };
        div()
            .flex_none()
            .w(px(SWITCH_WIDTH))
            .h(px(SWITCH_HEIGHT))
            .child(div().size_full().with_animation(
                SharedString::from(format!("{id}-switch-{flips}")),
                Animation::new(SWITCH.time()).with_easing(motion::ease_in_out),
                move |frame, t| frame.child(switch_at(if on { t } else { 1.0 - t })),
            ))
            .into_any_element()
    }
}

const SWITCH_WIDTH: f32 = 40.0;
const SWITCH_HEIGHT: f32 = 22.0;
/// The knob's width, and how far it travels.
const SWITCH_KNOB: f32 = 18.0;

/// A switch `on` of the way from off (0) to on (1).
fn switch_at(on: f32) -> gpui::Div {
    let mix = |from: u32, to: u32| (from as f32 + (to as f32 - from as f32) * on).round() as u32;
    let color = (mix(0xff, ACCENT >> 16) << 24)
        | (mix(0xff, (ACCENT >> 8) & 0xff) << 16)
        | (mix(0xff, ACCENT & 0xff) << 8)
        | mix(0x2e, 0xff);
    div()
        .flex_none()
        .w(px(SWITCH_WIDTH))
        .h(px(SWITCH_HEIGHT))
        .p(px(2.0))
        .rounded_full()
        .bg(rgba(color))
        .flex()
        .child(
            div()
                .size(px(SWITCH_KNOB))
                .ml(px(SWITCH_KNOB * on))
                .rounded_full()
                .bg(rgb(0xffffff)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flipped_switch_moves_and_counts_its_flips() {
        let switches = Switches::default();
        assert_eq!(switches.moving("leveller"), None);
        switches.flipped("leveller");
        assert_eq!(switches.moving("leveller"), Some(1));
        switches.flipped("leveller");
        assert_eq!(switches.moving("leveller"), Some(2));
        assert_eq!(switches.moving("another"), None);
    }

    #[test]
    fn toggling_an_entry_opens_it_then_closes_it() {
        let switches = Switches::default();
        assert!(!switches.is_open("entry"));
        switches.toggle_open("entry");
        assert!(switches.is_open("entry"));
        assert_eq!(switches.moving("entry"), Some(1));
        switches.toggle_open("entry");
        assert!(!switches.is_open("entry"));
    }

    #[test]
    fn a_tooltip_left_before_it_showed_just_goes() {
        let switches = Switches::default();
        switches.hover_tip("knob", true);
        assert!(matches!(switches.tip("knob"), Tip::Showing { .. }));
        // Still within its delay: nothing was visible, so nothing fades out.
        switches.hover_tip("knob", false);
        assert_eq!(switches.tip("knob"), Tip::Hidden);
    }

    #[test]
    fn hovering_another_thing_moves_the_tooltip_there() {
        let switches = Switches::default();
        switches.hover_tip("first", true);
        switches.hover_tip("second", true);
        assert_eq!(switches.tip("first"), Tip::Hidden);
        assert!(matches!(switches.tip("second"), Tip::Showing { .. }));
        // The mouse leaving the first one later changes nothing.
        switches.hover_tip("first", false);
        assert!(matches!(switches.tip("second"), Tip::Showing { .. }));
    }

    #[test]
    fn a_tooltip_stays_hidden_through_its_delay() {
        assert_eq!(tip_shown(Duration::ZERO), 0.0);
        assert_eq!(tip_shown(TIP_DELAY), 0.0);
        assert_eq!(tip_shown(TIP_DELAY + APPEAR.time()), 1.0);
    }
}
