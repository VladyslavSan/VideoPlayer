//! Settings → Controls: the player's keys, changed like in a game. Click
//! "+" next to an action and press a key (with Ctrl, Alt or Shift if you
//! like); click a key to take it off. How far the skip and volume keys go
//! is set here too.

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, KeyDownEvent, SharedString, div, prelude::*, px,
    rgb, rgba,
};

use super::{ACCENT, MUTED, Shell, TEXT};
use crate::player::controls::{Action, Controls, bindable, key_label};

const SETTING: &str = "controls";
const SKIP_CHOICES: [f64; 6] = [5.0, 10.0, 15.0, 30.0, 60.0, 85.0];
const VOLUME_CHOICES: [f64; 3] = [2.0, 5.0, 10.0];

/// Which action is waiting for a key, and what the last change did.
#[derive(Default)]
pub(super) struct KeyCapture {
    waiting: Option<Action>,
    note: Option<String>,
}

impl Shell {
    pub(super) fn controls(&self) -> Controls {
        match self.db.setting(SETTING).ok().flatten() {
            Some(json) => Controls::from_json(&json),
            None => Controls::default(),
        }
    }

    fn save_controls(&mut self, controls: &Controls) {
        if let Err(e) = self.db.set_setting(SETTING, &controls.to_json()) {
            eprintln!("[settings] saving the controls: {e:#}");
        }
    }

    /// While an action waits for its new key, the key goes to it (Esc
    /// cancels). True if the key was taken.
    pub(super) fn capture_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
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
        let mut controls = self.controls();
        self.key_capture.note = match controls.bind(action, &key) {
            None => Some(format!("{} is kept for closing and help.", key_label(&key))),
            Some(taken) if taken.is_empty() => None,
            Some(taken) => Some(format!(
                "{} was on “{}”; it's on “{}” now.",
                key_label(&key),
                taken
                    .iter()
                    .map(|a| a.label())
                    .collect::<Vec<_>>()
                    .join("”, “"),
                action.label()
            )),
        };
        self.save_controls(&controls);
        cx.notify();
        true
    }

    pub(super) fn controls_settings(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let controls = self.controls();
        let waiting = self.key_capture.waiting;
        let rows: Vec<AnyElement> = Action::ALL
            .into_iter()
            .map(|action| self.action_row(&controls, action, waiting == Some(action), cx))
            .collect();
        let keys = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(super::settings::note_text(
                "Click + and press a key, with Ctrl, Alt or Shift if you like. Click a key to \
                 take it off. A key can do one thing, except the skip, which shares: it only \
                 acts while a skip button or “Up next” shows, and otherwise the key does its \
                 other job. Esc (close, go back) and ? (this list in the player) stay as they are.",
            ))
            .children(
                self.key_capture
                    .note
                    .clone()
                    .map(|note| div().mt_2().text_sm().text_color(rgb(ACCENT)).child(note)),
            )
            .child(div().mt_2().flex().flex_col().gap_1().children(rows))
            .child(
                div().mt_3().flex().child(
                    div()
                        .id("controls-reset")
                        .px_3()
                        .py_1p5()
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
                        .bg(rgba(0xffffff14))
                        .hover(|s| s.bg(rgba(0xffffff26)))
                        .child("Reset to defaults")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            let defaults = Controls::default();
                            this.save_controls(&defaults);
                            this.key_capture = KeyCapture::default();
                            cx.notify();
                        })),
                ),
            );
        let amount = |id: &str,
                      title: &str,
                      choices: &[f64],
                      current: f64,
                      unit: &str,
                      cx: &mut Context<Self>,
                      set: fn(&mut Controls, f64)| {
            let choices = choices.to_vec();
            let options = choices
                .iter()
                .map(|c| (format!("{c:.0} {unit}"), (*c - current).abs() < 0.01))
                .collect();
            super::finder::chips(id, title, options, cx, move |this, i| {
                let mut controls = this.controls();
                set(&mut controls, choices[i]);
                this.save_controls(&controls);
            })
        };
        let amounts = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(amount(
                "controls-back",
                "Back goes",
                &SKIP_CHOICES,
                controls.back_seconds,
                "seconds",
                cx,
                |c, v| c.back_seconds = v,
            ))
            .child(amount(
                "controls-forward",
                "Forward goes",
                &SKIP_CHOICES,
                controls.forward_seconds,
                "seconds",
                cx,
                |c, v| c.forward_seconds = v,
            ))
            .child(amount(
                "controls-volume",
                "Volume up and down change it by",
                &VOLUME_CHOICES,
                controls.volume_step,
                "%",
                cx,
                |c, v| c.volume_step = v,
            ));
        vec![
            Self::section("Keys in the player", keys),
            Self::section("How far", amounts),
        ]
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
        let keys = controls.keys(action).to_vec();
        let chip = |id: SharedString, text: String| {
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
        };
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
                    .children(keys.into_iter().map(|key| {
                        let remove = key.clone();
                        chip(
                            SharedString::from(format!("key-{id}-{key}")),
                            key_label(&key),
                        )
                        .bg(rgba(0xffffff1a))
                        .hover(|s| s.bg(rgba(0xff7a7a40)))
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, _, cx| {
                                let mut controls = this.controls();
                                controls.unbind(action, &remove);
                                this.save_controls(&controls);
                                this.key_capture.note = None;
                                cx.notify();
                            },
                        ))
                    }))
                    .child(if waiting {
                        chip(
                            SharedString::from(format!("add-{id}")),
                            "Press a key…".to_string(),
                        )
                        .bg(rgb(ACCENT))
                        .text_color(rgb(TEXT))
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.key_capture.waiting = None;
                                cx.notify();
                            },
                        ))
                    } else {
                        chip(SharedString::from(format!("add-{id}")), "+".to_string())
                            .text_color(rgb(MUTED))
                            .bg(rgba(0xffffff0d))
                            .hover(|s| s.bg(rgba(0xffffff26)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.key_capture.waiting = Some(action);
                                this.key_capture.note = None;
                                // Keys come to the app, not a text field.
                                window.focus(&this.focus);
                                cx.notify();
                            }))
                    }),
            )
            .into_any_element()
    }
}
