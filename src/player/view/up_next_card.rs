//! The "Up next" card that counts down to the next episode, and the skip
//! button for a marked part.

use gpui::{AnyElement, Context, FontWeight, MouseButton, div, prelude::*, px, rgb, rgba};

use super::widgets::Emphasis;
use super::{BAR_CLEARANCE, BORDER, MUTED, SURFACE, TEXT, season_name};
use crate::player::Player;
use crate::player::fade::UpNextShown;
use crate::player::playing::Episode;

/// Distance of the card and the skip button from the window's right edge.
const RIGHT_MARGIN: f32 = 24.0;

impl Player {
    /// The skip button ("Skip Intro"), while it's on screen.
    pub(super) fn skip_layer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.fades.skip.on_screen().then(|| {
            let label = self.fades.shown.skip;
            let button = Self::text_button("skip", label, Emphasis::Primary, cx, |this, cx| {
                this.skip_segment();
                this.poke(cx);
            });
            div()
                .absolute()
                .right(px(RIGHT_MARGIN))
                .bottom(px(BAR_CLEARANCE))
                .opacity(self.fades.skip.value())
                .child(button)
                .into_any_element()
        })
    }

    /// The up-next card, while it's on screen.
    pub(super) fn up_next_layer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.fades.up_next.on_screen() {
            return None;
        }
        let card = self.up_next_card(self.fades.shown.up_next, cx)?;
        Some(
            div()
                .absolute()
                .inset_0()
                .opacity(self.fades.up_next.value())
                .child(card)
                .into_any_element(),
        )
    }

    /// Shown when an episode ends: counts down to the next one.
    fn up_next_card(&self, shown: UpNextShown, cx: &mut Context<Self>) -> Option<AnyElement> {
        let next = self.playing.next()?;
        Some(
            div()
                .absolute()
                .right(px(RIGHT_MARGIN))
                .bottom(px(BAR_CLEARANCE))
                .w(px(380.0))
                .p_4()
                .rounded_2xl()
                .bg(rgba(SURFACE))
                .border_1()
                .border_color(rgba(BORDER))
                .text_color(rgb(TEXT))
                .flex()
                .flex_col()
                .gap_3()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(MUTED))
                        .child("UP NEXT"),
                )
                .child(Self::next_episode_summary(next, shown.seconds))
                .child(Self::up_next_buttons(shown.ended, cx))
                .into_any_element(),
        )
    }

    /// The next episode's picture, name and the countdown.
    fn next_episode_summary(next: &Episode, seconds: u64) -> AnyElement {
        let heading = match next.number {
            Some(n) => format!("{} · Episode {n}", season_name(next.season)),
            None => season_name(next.season),
        };
        div()
            .flex()
            .gap_3()
            .child(Self::episode_thumb(next, 128.0))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().text_xs().text_color(rgb(MUTED)).child(heading))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(next.title.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(MUTED))
                            .child(format!("Playing in {seconds} s")),
                    ),
            )
            .into_any_element()
    }

    /// "Play now" and, once the video is over, the way back; in the credits,
    /// the choice to keep watching them.
    fn up_next_buttons(ended: bool, cx: &mut Context<Self>) -> AnyElement {
        let play_now =
            Self::text_button("play-now", "Play now", Emphasis::Primary, cx, |this, cx| {
                this.play_next(cx)
            });
        let other = if ended {
            Self::text_button(
                "up-next-back",
                "Back to library",
                Emphasis::Quiet,
                cx,
                |this, cx| this.exit(cx),
            )
        } else {
            Self::text_button(
                "watch-credits",
                "Watch credits",
                Emphasis::Quiet,
                cx,
                |this, cx| {
                    this.up_next.watch_credits();
                    this.poke(cx);
                },
            )
        };
        div()
            .flex()
            .gap_2()
            .child(play_now)
            .child(other)
            .into_any_element()
    }
}
