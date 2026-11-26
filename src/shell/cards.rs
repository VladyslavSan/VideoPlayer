//! The poster card, and what a page's cards have in common: the one picked
//! with the arrow keys, where each was drawn (to move between them), what
//! Enter does on each, and how lit each is. The shell and the views that
//! draw its pages share one `Cards`, written while they draw, so the arrow
//! keys move over every card on the page, whichever view drew it.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Canvas, ClickEvent, Context, Div, ElementId, FontWeight, MouseButton,
    MouseDownEvent, Pixels, Point, SharedString, Stateful, Window, canvas, div, prelude::*, px,
    relative, rgb, rgba,
};

use super::header::HEADER_HEIGHT;
use super::keynav::{self, Direction};
use super::page_motion::Motion;
use super::posters::{self, Hover, Light};
use super::{ACCENT, MUTED};

pub(super) const WIDTH: f32 = 256.0;
pub(super) const HEIGHT: f32 = 144.0;
/// The tile behind a card with no picture: one of these, by its title.
const PLACEHOLDER_COLORS: [u32; 6] = [0x2b3a55, 0x3d2b55, 0x2b5547, 0x55402b, 0x552b3a, 0x2b4d55];

/// What a poster card shows.
pub(super) struct Card {
    pub id: SharedString,
    pub thumb: Option<PathBuf>,
    pub title: String,
    pub subtitle: Option<String>,
    pub progress: Option<f32>,
    pub watched: bool,
    /// Ringed: the episode a show page was opened at.
    pub highlight: bool,
}

type Click<V> = dyn Fn(&mut V, &mut Window, &mut Context<V>);
type MenuClick<V> = dyn Fn(&mut V, Point<Pixels>, &mut Context<V>);

/// What a card does for the view `V` that draws it.
pub(super) struct Clicks<V> {
    card: Rc<Click<V>>,
    text: Option<Box<Click<V>>>,
    menu: Option<Box<MenuClick<V>>>,
}

impl<V> Clicks<V> {
    /// `on_click` is what clicking the card does, and Enter on it.
    pub(super) fn new(on_click: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static) -> Self {
        Self {
            card: Rc::new(on_click),
            text: None,
            menu: None,
        }
    }

    /// The text below the picture acts on its own (like a link).
    pub(super) fn on_text(
        mut self,
        on_click: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Self {
        self.text = Some(Box::new(on_click));
        self
    }

    /// A right click opens a menu where the mouse is.
    pub(super) fn on_menu(
        mut self,
        open: impl Fn(&mut V, Point<Pixels>, &mut Context<V>) + 'static,
    ) -> Self {
        self.menu = Some(Box::new(open));
        self
    }
}

/// A card drawn in this frame, and where.
#[derive(Clone)]
struct Placed {
    id: SharedString,
    bounds: Bounds<Pixels>,
}

/// What Enter does on a card: the same as clicking it, in its own view.
type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// The page's cards: the keyboard's cursor, this frame's cards, their light.
pub(super) struct Cards {
    motion: Rc<Motion>,
    /// How bright posters are at rest and lit.
    light: Cell<Light>,
    hover: RefCell<Hover>,
    /// The card picked with the arrow keys (its element id), if any.
    cursor: RefCell<Option<SharedString>>,
    placed: RefCell<Vec<Placed>>,
    actions: RefCell<HashMap<SharedString, Action>>,
    /// Where the highlighted card was last drawn.
    highlighted: Cell<Option<Bounds<Pixels>>>,
}

impl Cards {
    pub(super) fn new(motion: Rc<Motion>, light: Light) -> Self {
        Self {
            motion,
            light: Cell::new(light),
            hover: RefCell::default(),
            cursor: RefCell::default(),
            placed: RefCell::default(),
            actions: RefCell::default(),
            highlighted: Cell::new(None),
        }
    }

    pub(super) fn set_light(&self, light: Light) {
        self.light.set(light);
    }

    pub(super) fn cursor(&self) -> Option<SharedString> {
        self.cursor.borrow().clone()
    }

    pub(super) fn set_cursor(&self, cursor: Option<SharedString>) {
        *self.cursor.borrow_mut() = cursor;
    }

    /// Forgets last frame's cards: they are noted again as they're drawn.
    pub(super) fn start_frame(&self) {
        self.placed.borrow_mut().clear();
        self.actions.borrow_mut().clear();
    }

    /// Whether no card was drawn.
    pub(super) fn is_empty(&self) -> bool {
        self.placed.borrow().is_empty()
    }

    /// Where card `id` was drawn in this frame.
    pub(super) fn bounds(&self, id: &SharedString) -> Option<Bounds<Pixels>> {
        let placed = self.placed.borrow();
        placed
            .iter()
            .find(|card| card.id == *id)
            .map(|card| card.bounds)
    }

    /// The top-left card drawn within `view`, where the cursor starts.
    pub(super) fn first_in(&self, view: Bounds<Pixels>) -> Option<SharedString> {
        let placed = self.placed.borrow();
        placed
            .iter()
            .filter(|card| {
                let bounds = card.bounds;
                bounds.bottom() > view.top() + px(HEADER_HEIGHT)
                    && bounds.top() < view.bottom()
                    && bounds.left() >= view.left() - px(1.0)
            })
            .min_by(|a, b| {
                (a.bounds.top(), a.bounds.left())
                    .partial_cmp(&(b.bounds.top(), b.bounds.left()))
                    .unwrap_or(Ordering::Equal)
            })
            .map(|card| card.id.clone())
    }

    /// Moves the cursor to the nearest card in `direction` (from no card on
    /// the page: to the first one within `view`). Returns where that card is.
    pub(super) fn step_cursor(
        &self,
        direction: Direction,
        view: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let placed = self.placed.borrow().clone();
        let cursor = self.cursor();
        let current = cursor.and_then(|cursor| placed.iter().position(|card| card.id == cursor));
        let next = match current {
            Some(current) => {
                let rects: Vec<keynav::Rect> =
                    placed.iter().map(|card| rect(card.bounds)).collect();
                keynav::nearest(rects[current], &rects, direction)
            }
            None => self
                .first_in(view)
                .and_then(|first| placed.iter().position(|card| card.id == first)),
        };
        let card = placed.get(next?)?;
        self.set_cursor(Some(card.id.clone()));
        Some(card.bounds)
    }

    /// Enter on the cursor's card: what clicking it does, once the view
    /// handling the key is done. False if the cursor is on no card.
    pub(super) fn activate_cursor(&self, window: &mut Window, cx: &mut App) -> bool {
        let action = self
            .cursor()
            .and_then(|cursor| self.actions.borrow().get(&cursor).cloned());
        let Some(action) = action else {
            return false;
        };
        window.defer(cx, move |window, cx| action(window, cx));
        true
    }

    /// Where the highlighted card was last drawn, if it has been.
    pub(super) fn highlighted(&self) -> Option<Bounds<Pixels>> {
        self.highlighted.get()
    }

    /// Before another card is highlighted: the last one's place is stale.
    pub(super) fn forget_highlighted(&self) {
        self.highlighted.set(None);
    }

    /// Asks for another frame while a card is still lighting up or dimming.
    pub(super) fn keep_animating(&self, window: &mut Window) {
        if self.hover.borrow_mut().animating() {
            window.request_animation_frame();
        }
    }

    /// A poster card, for the view `V` being drawn.
    pub(super) fn card<V: 'static>(
        self: &Rc<Self>,
        card: Card,
        clicks: Clicks<V>,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let Clicks {
            card: on_click,
            text: on_text_click,
            menu: on_menu,
        } = clicks;
        let id = card.id.clone();
        let leaving = self.motion.is_leaving();
        if !leaving {
            let action = enter_action(on_click.clone(), cx);
            self.actions.borrow_mut().insert(id.clone(), action);
        }
        let (cards, hover_id) = (self.clone(), id.clone());
        div()
            .id(id.clone())
            .relative()
            .flex_none()
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .gap_2()
            .cursor_pointer()
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                cards.hover.borrow_mut().set(&hover_id, *hovered);
                cx.notify();
            }))
            .child(self.place_marker(id, leaving))
            .child(self.picture(&card, cx))
            .child(text(&card, on_text_click, cx))
            .on_click(
                cx.listener(move |view, _: &ClickEvent, window, cx| on_click(view, window, cx)),
            )
            .when_some(on_menu, |card, on_menu| {
                card.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        on_menu(view, event.position, cx);
                    }),
                )
            })
            .into_any_element()
    }

    /// Fills the card and notes where it is, for the arrow keys. A card on
    /// the page being left isn't theirs to move to.
    fn place_marker(self: &Rc<Self>, id: SharedString, leaving: bool) -> Canvas<()> {
        let cards = self.clone();
        canvas(
            move |bounds, _, _| {
                if !leaving {
                    cards.placed.borrow_mut().push(Placed { id, bounds });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// The card's picture (or its letter tile) and what's laid over it.
    fn picture<V: 'static>(self: &Rc<Self>, card: &Card, cx: &mut Context<V>) -> Div {
        let selected = self.cursor.borrow().as_ref() == Some(&card.id);
        let lit = if selected {
            1.0
        } else {
            self.hover.borrow().level(&card.id)
        };
        let brightness = self.light.get().at(lit);
        let color = placeholder_color(&card.title);
        let initial: String = card
            .title
            .chars()
            .next()
            .map(String::from)
            .unwrap_or_default();
        let letter = move || letter_tile(color, initial.clone());
        let art = self
            .motion
            .card_picture(&card.id, card.thumb.clone(), letter, cx);
        div()
            .relative()
            .w_full()
            .h(px(HEIGHT))
            .rounded_lg()
            .overflow_hidden()
            .bg(rgb(color))
            .when(card.highlight || selected, |picture| {
                picture.border_2().border_color(rgb(ACCENT))
            })
            .child(art)
            .child(posters::shade(brightness))
            .children(card.highlight.then(|| self.highlight_marker()))
            .children(card.progress.map(progress_bar))
            .children(card.watched.then(watched_badge))
    }

    /// Fills the highlighted card's picture and notes where it is, so the
    /// page can scroll to it.
    fn highlight_marker(self: &Rc<Self>) -> Canvas<()> {
        let cards = self.clone();
        canvas(
            move |bounds, _, _| cards.highlighted.set(Some(bounds)),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// Enter's action for a card of `cx`'s view: `on_click`, in that view.
fn enter_action<V: 'static>(on_click: Rc<Click<V>>, cx: &mut Context<V>) -> Action {
    let view = cx.weak_entity();
    Rc::new(move |window, cx| {
        view.update(cx, |view, cx| on_click(view, window, cx)).ok();
    })
}

fn rect(bounds: Bounds<Pixels>) -> keynav::Rect {
    keynav::Rect {
        left: f32::from(bounds.left()),
        top: f32::from(bounds.top()),
        right: f32::from(bounds.right()),
        bottom: f32::from(bounds.bottom()),
    }
}

/// The same colour every time for a title.
fn placeholder_color(title: &str) -> u32 {
    let sum = title.bytes().map(usize::from).sum::<usize>();
    PLACEHOLDER_COLORS[sum % PLACEHOLDER_COLORS.len()]
}

/// What a card with no picture shows: its title's first letter on a colour.
fn letter_tile(color: u32, initial: String) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(color))
        .text_3xl()
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(0xffffffb0))
        .child(initial)
        .into_any_element()
}

/// How far through it the viewer is, along the picture's bottom edge.
fn progress_bar(progress: f32) -> Div {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h_1()
        .bg(rgba(0x00000099))
        .child(div().h_full().w(relative(progress)).bg(rgb(ACCENT)))
}

fn watched_badge() -> Div {
    div()
        .absolute()
        .top_2()
        .right_2()
        .px_2()
        .rounded_md()
        .text_xs()
        .bg(rgba(0x000000a0))
        .child("Watched")
}

/// The title and subtitle below the picture; with `on_click`, a link.
fn text<V: 'static>(
    card: &Card,
    on_click: Option<Box<Click<V>>>,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let id = ElementId::NamedChild(Box::new(card.id.clone().into()), "text".into());
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(card.title.clone()),
        )
        .child(
            div()
                .h(px(16.0))
                .text_xs()
                .text_color(rgb(MUTED))
                .truncate()
                .child(card.subtitle.clone().unwrap_or_default()),
        )
        .when_some(on_click, |text, on_click| {
            text.hover(|s| s.underline()).on_click(cx.listener(
                move |view, _: &ClickEvent, window, cx| {
                    // Instead of the card's own action.
                    cx.stop_propagation();
                    on_click(view, window, cx)
                },
            ))
        })
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;

    fn cards_at(places: &[(&'static str, f32, f32)]) -> Cards {
        let cards = Cards::new(
            Rc::default(),
            Light {
                rest: 0.8,
                lit: 1.0,
            },
        );
        for (id, left, top) in places {
            cards.placed.borrow_mut().push(Placed {
                id: (*id).into(),
                bounds: Bounds::new(point(px(*left), px(*top)), size(px(WIDTH), px(200.0))),
            });
        }
        cards
    }

    fn page() -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), size(px(1200.0), px(800.0)))
    }

    #[test]
    fn the_cursor_starts_at_the_first_card_in_view() {
        // The first card is under the header's bar; the next row isn't.
        let cards = cards_at(&[
            ("hidden", 0.0, -150.0),
            ("b", 300.0, 100.0),
            ("a", 0.0, 100.0),
        ]);
        assert_eq!(cards.first_in(page()), Some("a".into()));
        let moved = cards.step_cursor(Direction::Down, page());
        assert_eq!(cards.cursor(), Some("a".into()));
        assert_eq!(moved, cards.bounds(&"a".into()));
    }

    #[test]
    fn the_cursor_moves_to_the_nearest_card_that_way() {
        let cards = cards_at(&[("a", 0.0, 100.0), ("b", 300.0, 100.0), ("c", 0.0, 400.0)]);
        cards.set_cursor(Some("a".into()));
        cards.step_cursor(Direction::Right, page());
        assert_eq!(cards.cursor(), Some("b".into()));
        cards.step_cursor(Direction::Down, page());
        assert_eq!(cards.cursor(), Some("c".into()));
    }

    #[test]
    fn the_cursor_stays_at_the_edge() {
        let cards = cards_at(&[("a", 0.0, 100.0), ("b", 300.0, 100.0)]);
        cards.set_cursor(Some("b".into()));
        assert_eq!(cards.step_cursor(Direction::Right, page()), None);
        assert_eq!(cards.cursor(), Some("b".into()));
    }

    #[test]
    fn a_new_frame_forgets_the_cards_drawn_before() {
        let cards = cards_at(&[("a", 0.0, 100.0)]);
        assert!(!cards.is_empty());
        cards.start_frame();
        assert!(cards.is_empty());
        assert_eq!(cards.step_cursor(Direction::Down, page()), None);
    }

    #[test]
    fn a_title_always_gets_the_same_placeholder_colour() {
        assert_eq!(placeholder_color("Dune"), placeholder_color("Dune"));
        assert!(PLACEHOLDER_COLORS.contains(&placeholder_color("")));
    }
}
