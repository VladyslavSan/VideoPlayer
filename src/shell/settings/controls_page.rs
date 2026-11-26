//! Settings → Controls: the player's keys, changed like in a game. Click
//! "+" next to an action and press a key (with Ctrl, Alt or Shift if you
//! like); click a key to take it off. How far the skip and volume keys go
//! is set here too.

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, KeyDownEvent, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};

use super::{Settings, note};
use crate::player::controls::{Action, Controls, bindable, key_label};
use crate::shell::{ACCENT, MUTED, TEXT, widgets};

const SKIP_CHOICES: [f64; 6] = [5.0, 10.0, 15.0, 30.0, 60.0, 85.0];
const VOLUME_CHOICES: [f64; 3] = [2.0, 5.0, 10.0];

/// Which action is waiting for a key, and what the last change did.
#[derive(Default)]
pub(super) struct KeyCapture {
    waiting: Option<Action>,
    note: Option<String>,
}

/// An amount the keys act by, picked from a few choices.
struct Amount {
    id: &'static str,
    title: &'static str,
    choices: &'static [f64],
    unit: &'static str,
    current: f64,
    set: fn(&mut Controls, f64),
}

/// What binding `key` to `action` did to other actions, if worth saying.
fn bound_note(controls: &mut Controls, action: Action, key: &str) -> Option<String> {
    match controls.bind(action, key) {
        None => Some(format!("{} is kept for closing and help.", key_label(key))),
        Some(taken) if taken.is_empty() => None,
        Some(taken) => {
            let taken: Vec<&str> = taken.iter().map(|action| action.label()).collect();
            Some(format!(
                "{} was on “{}”; it's on “{}” now.",
                key_label(key),
                taken.join("”, “"),
                action.label()
            ))
        }
    }
}

/// A key on an action's row, or its "+".
fn chip(id: SharedString, text: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(text)
}

impl Settings {
    /// While an action waits for its new key, the key goes to it (Esc
    /// cancels). True if the key was taken.
    pub(in crate::shell) fn capture_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(action) = self.key_capture.waiting else {
            return false;
        };
        let key = event.keystroke.unparse();
        if !bindable(&key) {
            return true; // a modifier on its own: wait for the rest
        }
        self.key_capture.waiting = None;
        if key == "escape" {
            self.key_capture.note = None;
            cx.notify();
            return true;
        }
        let mut controls = self.preferences().controls();
        self.key_capture.note = bound_note(&mut controls, action, &key);
        self.preferences().set_controls(&controls);
        cx.notify();
        true
    }

    fn reset_controls(&mut self, cx: &mut Context<Self>) {
        self.preferences().set_controls(&Controls::default());
        self.key_capture = KeyCapture::default();
        cx.notify();
    }

    fn unbind_key(&mut self, action: Action, key: &str, cx: &mut Context<Self>) {
        let mut controls = self.preferences().controls();
        controls.unbind(action, key);
        self.preferences().set_controls(&controls);
        self.key_capture.note = None;
        cx.notify();
    }

    /// The action's "+" was clicked: its next key is the one pressed now.
    fn wait_for_key(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        self.key_capture.waiting = Some(action);
        self.key_capture.note = None;
        // Keys come to the app, not a text field.
        window.focus(&self.page_focus);
        cx.notify();
    }

    fn stop_waiting_for_key(&mut self, cx: &mut Context<Self>) {
        self.key_capture.waiting = None;
        cx.notify();
    }

    fn set_amount(&mut self, set: fn(&mut Controls, f64), amount: f64) {
        let mut controls = self.preferences().controls();
        set(&mut controls, amount);
        self.preferences().set_controls(&controls);
    }

    pub(super) fn controls_page(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let controls = self.preferences().controls();
        vec![
            widgets::section("Keys in the player", self.keys(&controls, cx)),
            widgets::section("How far", self.amounts(&controls, cx)),
        ]
    }

    /// Every action with its keys, under how to change them.
    fn keys(&self, controls: &Controls, cx: &mut Context<Self>) -> gpui::Div {
        let waiting = self.key_capture.waiting;
        let rows: Vec<AnyElement> = Action::ALL
            .into_iter()
            .map(|action| self.action_row(controls, action, waiting == Some(action), cx))
            .collect();
        let last_change = self
            .key_capture
            .note
            .clone()
            .map(|note| div().mt_2().text_sm().text_color(rgb(ACCENT)).child(note));
        let reset = div()
            .id("controls-reset")
            .px_3()
            .py_1p5()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .bg(rgba(0xffffff14))
            .hover(|style| style.bg(rgba(0xffffff26)))
            .child("Reset to defaults")
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reset_controls(cx)));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(note(
                "Click + and press a key, with Ctrl, Alt or Shift if you like. Click a key to \
                 take it off. A key can do one thing, except the skip, which shares: it only \
                 acts while a skip button or “Up next” shows, and otherwise the key does its \
                 other job. Esc (close, go back) and ? (this list in the player) stay as they are.",
            ))
            .children(last_change)
            .child(div().mt_2().flex().flex_col().gap_1().children(rows))
            .child(div().mt_3().flex().child(reset))
    }

    /// How far the skip keys go and how much the volume keys change.
    fn amounts(&self, controls: &Controls, cx: &mut Context<Self>) -> gpui::Div {
        let amounts = [
            Amount {
                id: "controls-back",
                title: "Back goes",
                choices: &SKIP_CHOICES,
                unit: "seconds",
                current: controls.back_seconds,
                set: |controls, seconds| controls.back_seconds = seconds,
            },
            Amount {
                id: "controls-forward",
                title: "Forward goes",
                choices: &SKIP_CHOICES,
                unit: "seconds",
                current: controls.forward_seconds,
                set: |controls, seconds| controls.forward_seconds = seconds,
            },
            Amount {
                id: "controls-volume",
                title: "Volume up and down change it by",
                choices: &VOLUME_CHOICES,
                unit: "%",
                current: controls.volume_step,
                set: |controls, step| controls.volume_step = step,
            },
        ];
        div()
            .flex()
            .flex_col()
            .gap_4()
            .children(amounts.map(|amount| amount_chips(&amount, cx)))
    }

    fn action_row(
        &self,
        controls: &Controls,
        action: Action,
        waiting: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = format!("{action:?}");
        let label = match action {
            Action::Back => format!("Back {:.0} seconds", controls.back_seconds),
            Action::Forward => format!("Forward {:.0} seconds", controls.forward_seconds),
            other => other.label().to_string(),
        };
        let keys =
            controls.keys(action).iter().map(|key| {
                let remove = key.clone();
                chip(
                    SharedString::from(format!("key-{id}-{key}")),
                    key_label(key),
                )
                .bg(rgba(0xffffff1a))
                .hover(|style| style.bg(rgba(0xff7a7a40)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.unbind_key(action, &remove, cx)
                }))
            });
        let keys: Vec<_> = keys.collect();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_4()
            .py_2()
            .rounded_lg()
            .bg(rgba(0xffffff0a))
            .child(div().min_w(px(0.0)).text_sm().child(label))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_wrap()
                    .justify_end()
                    .items_center()
                    .gap_1p5()
                    .children(keys)
                    .child(add_key_chip(&id, action, waiting, cx)),
            )
            .into_any_element()
    }
}

/// The row of choices for one amount; picking one saves it.
fn amount_chips(amount: &Amount, cx: &mut Context<Settings>) -> AnyElement {
    let (choices, current, set) = (amount.choices, amount.current, amount.set);
    let options = choices
        .iter()
        .map(|choice| {
            let label = format!("{choice:.0} {}", amount.unit);
            (label, (*choice - current).abs() < 0.01)
        })
        .collect();
    widgets::chips(
        amount.id,
        amount.title,
        options,
        cx,
        move |this: &mut Settings, index| this.set_amount(set, choices[index]),
    )
}

/// An action's "+", or "Press a key…" while it waits for one.
fn add_key_chip(
    id: &str,
    action: Action,
    waiting: bool,
    cx: &mut Context<Settings>,
) -> gpui::Stateful<gpui::Div> {
    let add = SharedString::from(format!("add-{id}"));
    if waiting {
        chip(add, "Press a key…".to_string())
            .bg(rgb(ACCENT))
            .text_color(rgb(TEXT))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.stop_waiting_for_key(cx)))
    } else {
        chip(add, "+".to_string())
            .text_color(rgb(MUTED))
            .bg(rgba(0xffffff0d))
            .hover(|style| style.bg(rgba(0xffffff26)).text_color(rgb(TEXT)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.wait_for_key(action, window, cx)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_key_is_bound_without_a_word() {
        let mut controls = Controls::default();
        assert_eq!(bound_note(&mut controls, Action::Mute, "f9"), None);
        assert!(controls.keys(Action::Mute).contains(&"f9".to_string()));
    }

    #[test]
    fn a_key_taken_from_another_action_says_where_it_went() {
        let mut controls = Controls::default();
        let note = bound_note(&mut controls, Action::PlayPause, "m");
        let expected = format!(
            "M was on “{}”; it's on “{}” now.",
            Action::Mute.label(),
            Action::PlayPause.label()
        );
        assert_eq!(note, Some(expected));
    }

    #[test]
    fn the_keys_kept_for_closing_and_help_say_so() {
        let mut controls = Controls::default();
        let note = bound_note(&mut controls, Action::Mute, "escape");
        assert_eq!(note.as_deref(), Some("Esc is kept for closing and help."));
    }
}
