//! The keyboard on the shell's pages: the arrow keys move a cursor over the
//! cards and Enter opens the one under it, Escape goes back or clears the
//! search, and Settings gets the keys while it's showing.

use gpui::{Context, Focusable as _, KeyDownEvent, ScrollStrategy, Window};

use super::settings::KeyOutcome;
use super::{Grid, Screen, Shell, browse, keynav};

impl Shell {
    pub(super) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handled = self.settings_captures(event, cx)
            || self.closes_card_menu(event, cx)
            || self.browses_with(event, window, cx);
        if handled {
            return cx.stop_propagation();
        }
        self.page_key(event, window, cx);
    }

    /// An action in Settings → Controls waiting for its key gets it first.
    fn settings_captures(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let settings = self.settings.clone();
        settings.update(cx, |settings, cx| settings.capture_key(event, cx))
    }

    /// Escape closes a card's menu, if one is open.
    fn closes_card_menu(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let pages = self.library_pages.clone();
        event.keystroke.key == "escape" && pages.update(cx, |pages, cx| pages.close_card_menu(cx))
    }

    /// The arrow keys and Enter, unmodified, on a page with cards. False
    /// if the key did nothing there.
    fn browses_with(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let browsing = !matches!(self.navigation.screen(), Screen::Settings);
        let modifiers = &event.keystroke.modifiers;
        let plain = !(modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift);
        if !(browsing && plain) {
            return false;
        }
        let key = event.keystroke.key.as_str();
        let searching = self.search.focus_handle(cx).is_focused(window);
        match keynav::Direction::from_key(key) {
            // From the search field, Down goes into the results.
            Some(keynav::Direction::Down) if searching => {
                window.focus(&self.focus);
                self.cards.set_cursor(None);
                if self.move_cursor(keynav::Direction::Down, window, cx) {
                    return true;
                }
            }
            Some(direction) if !searching && self.move_cursor(direction, window, cx) => {
                return true;
            }
            _ => {}
        }
        key == "enter" && !searching && self.activates(window, cx)
    }

    /// Enter: what the card under the cursor does. False if there was
    /// nothing for it to do.
    fn activates(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cards.activate_cursor(window, cx) {
            return true;
        }
        // A movie's page has no cards: Enter is its Play button.
        if let Screen::Details(item) = self.navigation.screen().clone() {
            self.play(&item, window, cx);
            return true;
        }
        false
    }

    /// The keys that depend on the screen: going back, the search field,
    /// and everything on Settings.
    fn page_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        match (self.navigation.screen(), key) {
            (Screen::Settings, _) => self.settings_key(event, window, cx),
            (
                Screen::Show(_) | Screen::Details(_) | Screen::Tracked(..) | Screen::Found(..),
                "escape" | "backspace",
            ) => self.leave_page(window, cx),
            (screen, "escape") if screen.is_tab() => self.clear_search(window, cx),
            (screen, "f") if screen.is_tab() && event.keystroke.modifiers.secondary() => {
                window.focus(&self.search.focus_handle(cx));
                cx.notify();
            }
            _ => {}
        }
    }

    /// A key on Settings: it handles it, or asks to be left.
    fn settings_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        let outcome = settings.update(cx, |settings, cx| settings.key_down(event, window, cx));
        if outcome == KeyOutcome::Leave {
            self.leave_page(window, cx);
        }
    }

    /// Clears the search (and an app's name field) and leaves the field.
    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.set_text("", cx));
        if let Screen::Arr(_, kind) = *self.navigation.screen() {
            self.arr.update(cx, |arr, cx| arr.clear_name(kind, cx));
        }
        window.focus(&self.focus);
        cx.notify();
    }

    /// Moves the keyboard cursor; false if there's nothing to move between.
    fn move_cursor(
        &mut self,
        direction: keynav::Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let grid = Grid::of(self.navigation.screen()).filter(|_| self.search_query(cx).is_empty());
        if let Some(grid) = grid {
            return self.move_grid_cursor(&grid, direction, window, cx);
        }
        if self.cards.is_empty() {
            return false;
        }
        let view = self.navigation.content_scroll().bounds();
        if let Some(card) = self.cards.step_cursor(direction, view) {
            self.navigation.reveal(card);
        }
        cx.notify();
        true
    }

    /// Moves the cursor in a library grid, by index: only the rows on
    /// screen are drawn. False if the grid is empty.
    fn move_grid_cursor(
        &mut self,
        grid: &Grid,
        direction: keynav::Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let ids = self.library_pages.read(cx).grid_card_ids(grid);
        let columns = browse::columns(window);
        let cursor = self.cards.cursor();
        let current = cursor.and_then(|cursor| ids.iter().position(|id| *id == cursor));
        let next = match current {
            // Start at the first card in view.
            None => {
                let view = self
                    .navigation
                    .grid_scroll()
                    .0
                    .borrow()
                    .base_handle
                    .bounds();
                self.cards
                    .first_in(view)
                    .and_then(|first| ids.iter().position(|id| *id == first))
                    .or((!ids.is_empty()).then_some(0))
            }
            Some(i) => keynav::grid_step(i, ids.len(), columns, direction),
        };
        if let Some(next) = next.filter(|n| *n < ids.len()) {
            self.cards.set_cursor(Some(ids[next].clone()));
            self.navigation
                .grid_scroll()
                .scroll_to_item(next / columns, ScrollStrategy::Top);
        }
        cx.notify();
        !ids.is_empty()
    }
}
