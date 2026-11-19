//! The player's keys, which the viewer can change (Settings → Controls):
//! any key, with Ctrl, Alt or Shift, for any action, and how far the skip
//! and volume keys go. Esc and `?` stay fixed, so nobody locks themselves
//! out.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Action {
    /// The skip button (intro, recap, preview) or, during "Up next", the
    /// next episode. Only while one of those shows; otherwise the key does
    /// whatever else it's set to.
    Skip,
    PlayPause,
    Back,
    Forward,
    PreviousMark,
    NextMark,
    VolumeUp,
    VolumeDown,
    Mute,
    NextSubtitle,
    NextAudio,
    TracksPanel,
    Slower,
    Faster,
    SubtitlesEarlier,
    SubtitlesLater,
    Fullscreen,
    NextEpisode,
    EpisodesPanel,
    Shortcuts,
    Leave,
}

impl Action {
    /// In the order keys are tried: the skip first, since it only applies
    /// while offered.
    pub const ALL: [Action; 21] = [
        Action::Skip,
        Action::PlayPause,
        Action::Back,
        Action::Forward,
        Action::PreviousMark,
        Action::NextMark,
        Action::VolumeUp,
        Action::VolumeDown,
        Action::Mute,
        Action::NextSubtitle,
        Action::NextAudio,
        Action::TracksPanel,
        Action::Slower,
        Action::Faster,
        Action::SubtitlesEarlier,
        Action::SubtitlesLater,
        Action::Fullscreen,
        Action::NextEpisode,
        Action::EpisodesPanel,
        Action::Shortcuts,
        Action::Leave,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::Skip => "Skip the intro, recap or preview (or play the next episode now)",
            Action::PlayPause => "Play or pause",
            Action::Back => "Back",
            Action::Forward => "Forward",
            Action::PreviousMark => "Back to the previous intro, recap, credits or chapter",
            Action::NextMark => "On to the next intro, recap, credits or chapter",
            Action::VolumeUp => "Volume up",
            Action::VolumeDown => "Volume down",
            Action::Mute => "Mute",
            Action::NextSubtitle => "Next subtitle track (or off)",
            Action::NextAudio => "Next audio track",
            Action::TracksPanel => "Audio and subtitles panel",
            Action::Slower => "Slower",
            Action::Faster => "Faster",
            Action::SubtitlesEarlier => "Subtitles earlier",
            Action::SubtitlesLater => "Subtitles later",
            Action::Fullscreen => "Fullscreen",
            Action::NextEpisode => "Next episode",
            Action::EpisodesPanel => "Episodes",
            Action::Shortcuts => "This list of keys",
            Action::Leave => "Leave the player",
        }
    }

    /// Applies only now and then, so it can share keys with others.
    pub fn contextual(self) -> bool {
        self == Action::Skip
    }

    fn default_keys(self) -> &'static [&'static str] {
        match self {
            Action::Skip => &["enter", "right", "l"],
            Action::PlayPause => &["space", "k"],
            Action::Back => &["left", "j"],
            Action::Forward => &["right", "l"],
            Action::PreviousMark => &["ctrl-left"],
            Action::NextMark => &["ctrl-right"],
            Action::VolumeUp => &["up"],
            Action::VolumeDown => &["down"],
            Action::Mute => &["m"],
            Action::NextSubtitle => &["s"],
            Action::NextAudio => &["a"],
            Action::TracksPanel => &["c"],
            Action::Slower => &["["],
            Action::Faster => &["]"],
            Action::SubtitlesEarlier => &["z"],
            Action::SubtitlesLater => &["x"],
            Action::Fullscreen => &["f"],
            Action::NextEpisode => &["n"],
            Action::EpisodesPanel => &["e"],
            Action::Shortcuts => &["f1"],
            Action::Leave => &["backspace"],
        }
    }
}

/// Keys that can't be given to an action.
pub const FIXED_KEYS: [&str; 2] = ["escape", "shift-/"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    /// Keys as GPUI writes them: `right`, `ctrl-right`, `shift-l`.
    pub keys: BTreeMap<Action, Vec<String>>,
    pub back_seconds: f64,
    pub forward_seconds: f64,
    pub volume_step: f64,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            keys: Action::ALL
                .iter()
                .map(|a| (*a, a.default_keys().iter().map(|k| k.to_string()).collect()))
                .collect(),
            back_seconds: 10.0,
            forward_seconds: 10.0,
            volume_step: 5.0,
        }
    }
}

impl Controls {
    /// Saved settings; actions added since they were saved get their
    /// default keys.
    pub fn from_json(json: &str) -> Self {
        let mut controls: Controls = serde_json::from_str(json).unwrap_or_default();
        for action in Action::ALL {
            controls.keys.entry(action).or_insert_with(|| {
                action
                    .default_keys()
                    .iter()
                    .map(|k| k.to_string())
                    .collect()
            });
        }
        controls
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn keys(&self, action: Action) -> &[String] {
        self.keys.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The actions on `key`, in the order to try them.
    pub fn actions_for(&self, key: &str) -> Vec<Action> {
        Action::ALL
            .into_iter()
            .filter(|a| self.keys(*a).iter().any(|k| k == key))
            .collect()
    }

    /// Puts `key` on `action`. An action that always applies gives the key
    /// up to it (one key, one action); the skip shares. Returns the actions
    /// that lost it. A fixed key is refused (None).
    pub fn bind(&mut self, action: Action, key: &str) -> Option<Vec<Action>> {
        if FIXED_KEYS.contains(&key) {
            return None;
        }
        let mut taken_from = Vec::new();
        if !action.contextual() {
            for (other, keys) in self.keys.iter_mut() {
                if *other != action && !other.contextual() && keys.iter().any(|k| k == key) {
                    keys.retain(|k| k != key);
                    taken_from.push(*other);
                }
            }
        }
        let keys = self.keys.entry(action).or_default();
        if !keys.iter().any(|k| k == key) {
            keys.push(key.to_string());
        }
        Some(taken_from)
    }

    pub fn unbind(&mut self, action: Action, key: &str) {
        if let Some(keys) = self.keys.get_mut(&action) {
            keys.retain(|k| k != key);
        }
    }
}

/// A key as people write it: `Ctrl+→`, `Shift+L`, `Space`.
pub fn key_label(key: &str) -> String {
    let mut rest = key;
    let mut parts: Vec<&str> = Vec::new();
    loop {
        let (prefix, name) = match rest {
            r if r.starts_with("ctrl-") && r.len() > 5 => ("ctrl-", "Ctrl"),
            r if r.starts_with("alt-") && r.len() > 4 => ("alt-", "Alt"),
            r if r.starts_with("shift-") && r.len() > 6 => ("shift-", "Shift"),
            r if r.starts_with("win-") && r.len() > 4 => ("win-", "Win"),
            r if r.starts_with("cmd-") && r.len() > 4 => ("cmd-", "⌘"),
            r if r.starts_with("fn-") && r.len() > 3 => ("fn-", "Fn"),
            _ => break,
        };
        parts.push(name);
        rest = &rest[prefix.len()..];
    }
    let name = match rest {
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "space" => "Space".to_string(),
        "enter" => "Enter".to_string(),
        "backspace" => "Backspace".to_string(),
        "escape" => "Esc".to_string(),
        "tab" => "Tab".to_string(),
        "pageup" => "Page Up".to_string(),
        "pagedown" => "Page Down".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "delete" => "Delete".to_string(),
        "insert" => "Insert".to_string(),
        other if other.len() > 1 && other.starts_with('f') && other[1..].parse::<u8>().is_ok() => {
            other.to_uppercase()
        }
        other => other.to_uppercase(),
    };
    let mut label = parts.join("+");
    if !label.is_empty() {
        label.push('+');
    }
    label.push_str(&name);
    label
}

/// A key press worth giving to an action: not a modifier on its own.
pub fn bindable(key: &str) -> bool {
    let bare = key.rsplit('-').next().unwrap_or(key);
    !key.is_empty()
        && !matches!(
            bare,
            "shift" | "control" | "ctrl" | "alt" | "platform" | "function" | "capslock"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_like_people_write_them() {
        assert_eq!(key_label("ctrl-right"), "Ctrl+→");
        assert_eq!(key_label("shift-l"), "Shift+L");
        assert_eq!(key_label("space"), "Space");
        assert_eq!(key_label("f1"), "F1");
        assert_eq!(key_label("["), "[");
        assert_eq!(key_label("ctrl--"), "Ctrl+-");
        assert_eq!(key_label("ctrl-alt-shift-k"), "Ctrl+Alt+Shift+K");
    }

    #[test]
    fn the_skip_shares_the_arrow_with_forward() {
        let controls = Controls::default();
        assert_eq!(
            controls.actions_for("right"),
            vec![Action::Skip, Action::Forward]
        );
        assert_eq!(controls.actions_for("ctrl-right"), vec![Action::NextMark]);
    }

    #[test]
    fn a_key_moves_from_one_action_to_another() {
        let mut controls = Controls::default();
        // Skip only on Ctrl+→: plain → is just forward again.
        controls.unbind(Action::Skip, "right");
        assert_eq!(controls.bind(Action::Skip, "ctrl-right"), Some(vec![]));
        assert_eq!(controls.actions_for("right"), vec![Action::Forward]);
        assert_eq!(
            controls.actions_for("ctrl-right"),
            vec![Action::Skip, Action::NextMark]
        );
        // An action that always applies takes the key over.
        assert_eq!(
            controls.bind(Action::Fullscreen, "m"),
            Some(vec![Action::Mute])
        );
        assert!(controls.keys(Action::Mute).is_empty());
        assert_eq!(controls.bind(Action::Mute, "escape"), None);
    }

    #[test]
    fn saved_controls_gain_new_actions() {
        let mut saved = Controls::default();
        saved.keys.remove(&Action::NextMark);
        saved.back_seconds = 5.0;
        let loaded = Controls::from_json(&saved.to_json());
        assert_eq!(loaded.keys(Action::NextMark), ["ctrl-right"]);
        assert_eq!(loaded.back_seconds, 5.0);
        assert_eq!(Controls::from_json("not json"), Controls::default());
    }

    #[test]
    fn modifiers_alone_are_not_keys() {
        assert!(!bindable("shift"));
        assert!(!bindable("ctrl-control"));
        assert!(bindable("ctrl-right"));
        assert!(bindable("-"));
    }
}
