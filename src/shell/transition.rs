//! Going from one page to the next: the page just left fades out, scrolled
//! where it was, under the new one fading in from its top. A page come
//! back to gets its keyboard cursor again, scrolled into view.

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, ScrollStrategy, Window, div,
    prelude::*, px,
};

use super::{Grid, Shell, browse};

impl Shell {
    /// The page just left starts fading out (keeping where it was
    /// scrolled); it's gone once the new page has faded in.
    fn start_leaving(&mut self, cx: &mut Context<Self>) {
        self.arr.update(cx, |arr, _| arr.start_leaving());
        if !self.navigation.start_leaving() {
            return;
        }
        let generation = self.motion.generation();
        let page_time = crate::motion::PAGE.time();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(page_time).await;
            this.update(cx, |this, cx| {
                if this.motion.generation() == generation {
                    this.navigation.stop_leaving();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The page being left, drawn with its own scroll positions.
    pub(super) fn leaving_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let screen = self.navigation.fading_out()?;
        self.navigation.swap_leaving_scrolls();
        let motion = self.motion.clone();
        let page = motion.drawing_leaving(|| self.page(&screen, window, cx));
        self.navigation.swap_leaving_scrolls();
        Some(page)
    }

    /// Scrolling for the page about to be drawn: a new screen starts at the
    /// top, with the cursor it had; a show page opened at an episode keeps
    /// scrolling to it for a moment (see `Navigation::follow_episode`).
    pub(super) fn update_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation.is_new_screen() {
            self.motion.page_opened();
            self.start_leaving(cx);
            let cursor = self.navigation.screen_drawn(self.cards.cursor());
            self.cards.set_cursor(cursor);
        }
        if self.navigation.reveals_cursor() {
            self.reveal_restored_cursor(window, cx);
        }
        if self.navigation.follow_episode(self.cards.highlighted()) {
            window.request_animation_frame();
        }
    }

    /// Brings a cursor put back on return into view: in a grid by its
    /// index, on a page once the page has been drawn.
    fn reveal_restored_cursor(&mut self, window: &mut Window, cx: &App) {
        let Some(cursor) = self.cards.cursor() else {
            self.navigation.cursor_revealed();
            return;
        };
        if let Some(grid) = Grid::of(self.navigation.screen()) {
            let columns = browse::columns(window);
            let ids = self.library_pages.read(cx).grid_card_ids(&grid);
            let index = ids.iter().position(|id| *id == cursor);
            if let Some(index) = index {
                self.navigation
                    .grid_scroll()
                    .scroll_to_item(index / columns, ScrollStrategy::Center);
            }
            self.navigation.cursor_revealed();
            return;
        }
        if !self.navigation.cards_are_current() {
            window.request_animation_frame();
            return;
        }
        if let Some(card) = self.cards.bounds(&cursor) {
            self.navigation.reveal(card);
        }
        self.navigation.cursor_revealed();
    }

    /// The new page fading in, in place, over the one it replaces fading
    /// out, so one turns into the other.
    pub(super) fn crossfade(&self, content: AnyElement, leaving: Option<AnyElement>) -> AnyElement {
        if !crate::motion::enabled() {
            return content;
        }
        let page_time = crate::motion::PAGE.time();
        let entering = div()
            .id("entering")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            // The page being left doesn't react to the mouse.
            .occlude()
            .child(content)
            .with_animation(
                ("page", self.motion.generation()),
                Animation::new(page_time).with_easing(crate::motion::ease_in_out),
                |page, t| page.opacity(t),
            );
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .children(leaving.map(|page| {
                div()
                    .id("leaving")
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .child(page)
                    .with_animation(
                        ("leaving", self.motion.generation()),
                        Animation::new(page_time).with_easing(crate::motion::ease_in_out),
                        |page, t| page.opacity(1.0 - t),
                    )
            }))
            .child(entering)
            .into_any_element()
    }
}
