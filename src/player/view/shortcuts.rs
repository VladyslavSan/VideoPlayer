//! The list of keyboard shortcuts, opened with F1 or `?`.

use gpui::{
    AnyElement, Context, Div, FontWeight, MouseButton, div, prelude::*, px, relative, rgb, rgba,
};

use super::{BORDER, MUTED, TEXT};
use crate::player::Player;
use crate::player::controls::{Action, Controls, key_label};
use crate::ui::icons::Icon;

/// The list's card: the panels' surface, a little more opaque.
const CARD: u32 = 0x15171df5;
/// Behind a key's name.
const KEY_BACKGROUND: u32 = 0xffffff1a;

/// A line of the list: the keys, and what they do.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shortcut {
    keys: String,
    action: String,
}

impl Shortcut {
    fn fixed(keys: &str, action: &str) -> Self {
        Self {
            keys: keys.into(),
            action: action.into(),
        }
    }
}

/// Every shortcut that applies, as set in Settings → Controls, then the
/// fixed ones. Episode actions are left out for a movie.
fn shortcuts(controls: &Controls, has_episodes: bool) -> Vec<Shortcut> {
    let mut rows = Vec::new();
    for action in Action::ALL {
        if matches!(action, Action::NextEpisode | Action::EpisodesPanel) && !has_episodes {
            continue;
        }
        let keys = controls.keys(action);
        if keys.is_empty() {
            continue;
        }
        let keys: Vec<String> = keys.iter().map(|k| key_label(k)).collect();
        let label = match action {
            Action::Back => format!("Back {:.0} seconds", controls.back_seconds),
            Action::Forward => format!("Forward {:.0} seconds", controls.forward_seconds),
            other => other.label().to_string(),
        };
        rows.push(Shortcut {
            keys: keys.join(" / "),
            action: label,
        });
    }
    rows.push(Shortcut::fixed("Double-click", "Fullscreen"));
    rows.push(Shortcut::fixed("Click", "Show or hide the controls"));
    rows.push(Shortcut::fixed("?", "This list of keys"));
    rows.push(Shortcut::fixed(
        "Esc",
        "Close, leave fullscreen, or go back",
    ));
    rows
}

/// One of the list's two columns.
fn column(rows: Vec<Shortcut>) -> Div {
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap_2()
        .children(rows.into_iter().map(|row| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div().w(px(130.0)).flex_none().child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(rgba(KEY_BACKGROUND))
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.keys),
                    ),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child(row.action),
                )
        }))
}

impl Player {
    pub(super) fn shortcuts_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = shortcuts(&self.input.controls, self.playing.has_episodes());
        // Two columns: the list is long once every action has its keys.
        let right = rows.split_off(rows.len().div_ceil(2));
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("shortcuts")
                    .w(px(980.0))
                    .max_w(relative(0.94))
                    .p_5()
                    .rounded_2xl()
                    .bg(rgba(CARD))
                    .border_1()
                    .border_color(rgba(BORDER))
                    .text_color(rgb(TEXT))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.shortcuts_heading(cx))
                    .child(
                        div()
                            .flex()
                            .gap_6()
                            .child(column(rows))
                            .child(column(right)),
                    ),
            )
            .into_any_element()
    }

    /// The list's title and its close button.
    fn shortcuts_heading(&self, cx: &mut Context<Self>) -> Div {
        let close = self.icon_button("close-shortcuts", Icon::Close, 18.0, cx, |this, _, _| {
            this.panels.close()
        });
        div()
            .flex()
            .items_center()
            .justify_between()
            .pb_1()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Keyboard shortcuts"),
            )
            .child(close)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(rows: &[Shortcut]) -> Vec<&str> {
        rows.iter().map(|row| row.action.as_str()).collect()
    }

    #[test]
    fn a_movie_lists_no_episode_shortcuts() {
        let controls = Controls::default();
        let show = shortcuts(&controls, true);
        let movie = shortcuts(&controls, false);
        let episode_label = Action::NextEpisode.label();
        assert!(actions(&show).contains(&episode_label));
        assert!(!actions(&movie).contains(&episode_label));
    }

    #[test]
    fn skips_are_listed_with_their_lengths() {
        let controls = Controls {
            back_seconds: 7.0,
            forward_seconds: 25.0,
            ..Controls::default()
        };
        let rows = shortcuts(&controls, false);
        assert!(actions(&rows).contains(&"Back 7 seconds"));
        assert!(actions(&rows).contains(&"Forward 25 seconds"));
    }

    #[test]
    fn the_fixed_shortcuts_come_last() {
        let rows = shortcuts(&Controls::default(), false);
        let last: Vec<&str> = rows[rows.len() - 4..]
            .iter()
            .map(|row| row.keys.as_str())
            .collect();
        assert_eq!(last, ["Double-click", "Click", "?", "Esc"]);
    }
}
