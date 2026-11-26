//! Sound filters: the night ones lift quiet moments (dialogue, whispers)
//! and turn loud ones (explosions, music stings) down, so they end up
//! closer in volume. Like Dolby's own night mode they leave a band around
//! dialogue level alone, so normal speech and music sound as mixed. Others
//! bring dialogue forward or even out loudness. FFmpeg filters in a row,
//! each one skipped when it's off:
//! - surround folded to stereo with dialogue up front (`Downmix`), for 5.1
//!   and 7.1 heard on stereo speakers or headphones;
//! - a compressor turns down what's above `cut_above`, by `cut_ratio`,
//!   reacting in `attack` (fast enough to catch a bang, slow enough that
//!   hits keep their snap) and letting go over `release`;
//! - a second one lifts what's below `lift_below`, by `lift_ratio`;
//! - a leveller evens out loudness over a few seconds (`dynaudnorm`);
//! - `boost` raises the result, and a limiter at `ceiling` catches the
//!   first instant of a bang.
//!
//! Settings and the player pick one of the presets or one of two custom
//! tunings (`SoundSettings`). The Night preset (`Balanced`) was tuned on
//! real film audio (EBU R128 short-term loudness): Blade Runner 2049's
//! opening keeps its loudness, with quiet moments +5 dB and loud ones
//! -2 dB. An earlier, harder setting (-30 dB, 5 ms) flattened everything
//! and sounded dull.

use serde::{Deserialize, Serialize};

/// How surround sound is folded to stereo, if at all. Jellyfin's two
/// community recipes (`DownMixAlgorithmsHelper.cs`); 7.1 is folded to 5.1
/// first, the AC-4 way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Downmix {
    /// As mpv does it for the speakers there are.
    #[default]
    Off,
    /// NightmodeDialogue (Robert Collier, Doom9): the centre channel, where
    /// dialogue is, at full strength, the rest at 30%.
    Dialogue,
    /// Dave750 (SuperUser): centre and bass folded in, front and back
    /// lowered 3 dB; balanced, if bass-heavy.
    Balanced,
}

impl Downmix {
    pub const ALL: [Downmix; 3] = [Downmix::Off, Downmix::Dialogue, Downmix::Balanced];

    pub fn label(self) -> &'static str {
        match self {
            Downmix::Off => "Off",
            Downmix::Dialogue => "Dialogue first",
            Downmix::Balanced => "Balanced",
        }
    }

    /// The `pan` filters for a track with this many channels; none for
    /// stereo, mono or layouts the recipes don't cover.
    fn filter(self, channels: Option<u32>) -> Option<String> {
        // 7.1 to 5.1(side), then the recipe.
        const FOLD_71: &str =
            "pan=5.1(side)|c0=c0|c1=c1|c2=c2|c3=c3|c4=0.707*c4+0.707*c6|c5=0.707*c5+0.707*c7";
        let recipe = match self {
            Downmix::Off => return None,
            Downmix::Dialogue => "pan=stereo|c0=c2+0.30*c0+0.30*c4|c1=c2+0.30*c1+0.30*c5",
            Downmix::Balanced => {
                "pan=stereo|c0=0.5*c2+0.707*c0+0.707*c4+0.5*c3|c1=0.5*c2+0.707*c1+0.707*c5+0.5*c3"
            }
        };
        match channels? {
            6 => Some(recipe.to_string()),
            8 => Some(format!("{FOLD_71},{recipe}")),
            _ => None,
        }
    }
}

/// How a sound filter sounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundTuning {
    /// Sound above this (dBFS) is turned down.
    pub cut_above: f32,
    /// How hard: 4 turns 8 dB over into 2 dB over; 1 is off.
    pub cut_ratio: f32,
    /// How fast it reacts to a loud sound, in milliseconds.
    pub attack: f32,
    /// How slowly it lets go, in milliseconds.
    pub release: f32,
    /// How gradually it starts around `cut_above`, in dB.
    pub knee: f32,
    /// Sound below this (dBFS) is lifted.
    pub lift_below: f32,
    /// How much: 1 is off.
    pub lift_ratio: f32,
    /// Raised afterwards, in dB.
    pub boost: f32,
    /// Never louder than this (dBFS).
    pub ceiling: f32,
    pub downmix: Downmix,
    /// Loudness evened out over a few seconds as well.
    pub leveller: bool,
}

impl Default for SoundTuning {
    fn default() -> Self {
        SoundPreset::Balanced.tuning()
    }
}

/// Ready-made tunings. (Saved by these names: the first three were night
/// mode's Gentle, Balanced and Strong.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundPreset {
    Gentle,
    Balanced,
    Strong,
    Dialogue,
    Leveller,
}

// A preset added is counted, or its changes would have nowhere to go.
const _: () = assert!(SoundPreset::Leveller as usize + 1 == SoundPreset::COUNT);

impl SoundPreset {
    pub const COUNT: usize = 5;

    /// Where the preset's changes are kept in `SoundSettings::edited`.
    fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            SoundPreset::Gentle => "Gentle night",
            SoundPreset::Balanced => "Night",
            SoundPreset::Strong => "Strong night",
            SoundPreset::Dialogue => "Dialogue",
            SoundPreset::Leveller => "Leveller",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            SoundPreset::Gentle => "Takes the edge off the loudest moments; the mix stays lively.",
            SoundPreset::Balanced => {
                "Quiet moments lifted, loud ones turned down; overall loudness stays as it was."
            }
            SoundPreset::Strong => {
                "Much less between whispers and explosions, for late nights and thin walls."
            }
            SoundPreset::Dialogue => {
                "Night, with 5.1 and 7.1 folded to stereo and the dialogue channel up front \
                 (Jellyfin's NightmodeDialogue), for speakers or headphones."
            }
            SoundPreset::Leveller => {
                "No compressor: loudness evened out over a few seconds (a community dynaudnorm \
                 recipe). Takes a moment to settle after switching on."
            }
        }
    }

    pub fn tuning(self) -> SoundTuning {
        let balanced = SoundTuning {
            cut_above: -24.0,
            cut_ratio: 4.0,
            attack: 15.0,
            release: 300.0,
            knee: 6.0,
            lift_below: -40.0,
            lift_ratio: 2.5,
            boost: 5.0,
            ceiling: -1.0,
            downmix: Downmix::Off,
            leveller: false,
        };
        match self {
            SoundPreset::Balanced => balanced,
            SoundPreset::Gentle => SoundTuning {
                cut_above: -20.0,
                cut_ratio: 2.5,
                attack: 20.0,
                release: 350.0,
                lift_below: -45.0,
                lift_ratio: 1.75,
                boost: 3.0,
                ..balanced
            },
            SoundPreset::Strong => SoundTuning {
                cut_above: -28.0,
                cut_ratio: 6.0,
                attack: 10.0,
                release: 250.0,
                lift_below: -38.0,
                lift_ratio: 3.0,
                boost: 7.0,
                ..balanced
            },
            // The dialogue-first fold is about 3 dB quieter than mpv's own.
            SoundPreset::Dialogue => SoundTuning {
                downmix: Downmix::Dialogue,
                boost: 8.0,
                ..balanced
            },
            SoundPreset::Leveller => SoundTuning {
                cut_ratio: 1.0,
                lift_ratio: 1.0,
                boost: 0.0,
                leveller: true,
                ..balanced
            },
        }
    }
}

/// Which tuning the sound filter uses: a preset, or one of the custom ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundProfile {
    Preset(SoundPreset),
    /// Custom 1 or 2 (0 or 1).
    Custom(usize),
}

impl SoundProfile {
    pub const CUSTOM_SLOTS: usize = 2;

    /// Every choice, in the order Settings and the player show them.
    pub const ALL: [SoundProfile; 7] = [
        SoundProfile::Preset(SoundPreset::Gentle),
        SoundProfile::Preset(SoundPreset::Balanced),
        SoundProfile::Preset(SoundPreset::Strong),
        SoundProfile::Preset(SoundPreset::Dialogue),
        SoundProfile::Preset(SoundPreset::Leveller),
        SoundProfile::Custom(0),
        SoundProfile::Custom(1),
    ];

    /// The presets among them.
    #[cfg(test)]
    pub fn presets() -> impl Iterator<Item = SoundPreset> {
        Self::ALL.into_iter().filter_map(|p| match p {
            SoundProfile::Preset(preset) => Some(preset),
            SoundProfile::Custom(_) => None,
        })
    }

    pub fn label(self) -> String {
        match self {
            SoundProfile::Preset(preset) => preset.label().to_string(),
            SoundProfile::Custom(slot) => format!("Custom {}", slot + 1),
        }
    }
}

/// The sound filter's settings: what it uses, the custom tunings (kept
/// while a preset is picked), and the presets as changed, if they were.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    pub profile: SoundProfile,
    pub custom: [SoundTuning; SoundProfile::CUSTOM_SLOTS],
    /// Each preset's knobs as turned (by `SoundPreset::index`); None as it
    /// comes, so a preset left alone follows any retuning of it here.
    pub edited: [Option<SoundTuning>; SoundPreset::COUNT],
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            profile: SoundProfile::Preset(SoundPreset::Balanced),
            custom: [SoundTuning::default(); SoundProfile::CUSTOM_SLOTS],
            edited: [None; SoundPreset::COUNT],
        }
    }
}

impl SoundSettings {
    /// The tuning in use.
    pub fn tuning(&self) -> SoundTuning {
        match self.profile {
            SoundProfile::Preset(preset) => {
                self.edited[preset.index()].unwrap_or_else(|| preset.tuning())
            }
            SoundProfile::Custom(slot) => self.custom[slot.min(SoundProfile::CUSTOM_SLOTS - 1)],
        }
    }

    /// What the tuning in use goes back to: a preset as it comes; for a
    /// custom tuning, Night (what it starts as).
    pub fn original(&self) -> SoundTuning {
        match self.profile {
            SoundProfile::Preset(preset) => preset.tuning(),
            SoundProfile::Custom(_) => SoundTuning::default(),
        }
    }

    /// Whether the tuning in use is other than `original`.
    pub fn changed(&self) -> bool {
        self.tuning() != self.original()
    }

    /// Changes the tuning in use. A preset changed back to how it comes
    /// is the original again.
    pub fn tune(&mut self, change: impl FnOnce(&mut SoundTuning)) {
        let mut tuning = self.tuning();
        change(&mut tuning);
        let tuning = tuning.clamped();
        match self.profile {
            SoundProfile::Preset(preset) => {
                self.edited[preset.index()] = (tuning != preset.tuning()).then_some(tuning);
            }
            SoundProfile::Custom(slot) => {
                self.custom[slot.min(SoundProfile::CUSTOM_SLOTS - 1)] = tuning;
            }
        }
    }

    /// The tuning in use back to `original`.
    pub fn reset(&mut self) {
        let original = self.original();
        self.tune(|t| *t = original);
    }

    pub fn from_json(json: &str) -> Self {
        let mut settings = serde_json::from_str::<Self>(json).unwrap_or_default();
        if let SoundProfile::Custom(slot) = settings.profile
            && slot >= SoundProfile::CUSTOM_SLOTS
        {
            settings.profile = SoundProfile::Custom(0);
        }
        for tuning in &mut settings.custom {
            *tuning = tuning.clamped();
        }
        for tuning in settings.edited.iter_mut().flatten() {
            *tuning = tuning.clamped();
        }
        settings
    }

    pub fn to_json(self) -> String {
        serde_json::to_string(&self).unwrap_or_default()
    }
}

impl SoundTuning {
    /// Within what the filters take (a saved value may be from anywhere).
    pub fn clamped(self) -> Self {
        Self {
            cut_above: self.cut_above.clamp(-60.0, 0.0),
            cut_ratio: self.cut_ratio.clamp(1.0, 20.0),
            attack: self.attack.clamp(0.1, 2000.0),
            release: self.release.clamp(1.0, 9000.0),
            knee: self.knee.clamp(1.0, 8.0),
            lift_below: self.lift_below.clamp(-60.0, 0.0),
            lift_ratio: self.lift_ratio.clamp(1.0, 20.0),
            boost: self.boost.clamp(0.0, 36.0),
            ceiling: self.ceiling.clamp(-24.0, 0.0),
            ..self
        }
    }

    /// The labelled mpv audio filter (`af add`), for a track with
    /// `channels` channels (None: not known yet, so no downmix).
    pub fn filter(&self, channels: Option<u32>) -> String {
        let t = self.clamped();
        let mut stages = Vec::new();
        stages.extend(t.downmix.filter(channels));
        if t.cut_ratio > 1.0 {
            stages.push(format!(
                "acompressor=threshold={}dB:ratio={}:attack={}:release={}:knee={}",
                t.cut_above, t.cut_ratio, t.attack, t.release, t.knee
            ));
        }
        if t.lift_ratio > 1.0 {
            stages.push(format!(
                "acompressor=mode=upward:threshold={}dB:ratio={}:attack=30:release=500:knee={}",
                t.lift_below, t.lift_ratio, t.knee
            ));
        }
        if t.leveller {
            // As shared for clear dialogue at low TV volume (VideoHelp),
            // with a lower peak target (0.35, not 0.9): as shared it came
            // out 9 dB louder than the other presets.
            stages.push("dynaudnorm=p=0.35:m=15:s=9:g=3".to_string());
        }
        if t.boost > 0.0 {
            stages.push(format!("volume={}dB", t.boost));
        }
        stages.push(format!(
            "alimiter=limit={}dB:level=0:attack=5:release=80",
            t.ceiling
        ));
        format!("@sound:lavfi=[{}]", stages.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_filter_is_the_tuned_one() {
        assert_eq!(
            SoundTuning::default().filter(Some(6)),
            "@sound:lavfi=[acompressor=threshold=-24dB:ratio=4:attack=15:release=300:knee=6,\
             acompressor=mode=upward:threshold=-40dB:ratio=2.5:attack=30:release=500:knee=6,\
             volume=5dB,alimiter=limit=-1dB:level=0:attack=5:release=80]"
        );
    }

    #[test]
    fn downmix_only_for_surround() {
        let dialogue = SoundPreset::Dialogue.tuning();
        assert!(!dialogue.filter(Some(2)).contains("pan="));
        assert!(!dialogue.filter(None).contains("pan="));
        assert!(
            dialogue
                .filter(Some(6))
                .starts_with("@sound:lavfi=[pan=stereo|c0=c2+0.30*c0")
        );
        let seven_one = dialogue.filter(Some(8));
        assert!(seven_one.starts_with("@sound:lavfi=[pan=5.1(side)|"));
        assert_eq!(seven_one.matches("pan=").count(), 2);
    }

    #[test]
    fn stages_that_are_off_are_left_out() {
        let leveller = SoundPreset::Leveller.tuning().filter(Some(2));
        assert!(!leveller.contains("acompressor"));
        assert!(!leveller.contains("volume="));
        assert!(leveller.contains("dynaudnorm"));
        assert!(leveller.contains("alimiter"));
    }

    #[test]
    fn custom_tunings_are_kept_while_a_preset_is_used() {
        let mut settings = SoundSettings::default();
        settings.custom[1].boost = 9.0;
        settings.profile = SoundProfile::Preset(SoundPreset::Strong);
        assert_eq!(settings.tuning(), SoundPreset::Strong.tuning());
        settings.profile = SoundProfile::Custom(1);
        assert_eq!(settings.tuning().boost, 9.0);
        assert_eq!(SoundSettings::from_json(&settings.to_json()), settings);
    }

    #[test]
    fn presets_can_be_changed_and_reset() {
        let mut settings = SoundSettings {
            profile: SoundProfile::Preset(SoundPreset::Strong),
            ..SoundSettings::default()
        };
        assert!(!settings.changed());
        settings.tune(|t| t.boost = 2.0);
        assert!(settings.changed());
        assert_eq!(settings.tuning().boost, 2.0);
        assert_eq!(
            settings.tuning().cut_ratio,
            SoundPreset::Strong.tuning().cut_ratio
        );
        // Kept while another is used, and saved.
        settings.profile = SoundProfile::Preset(SoundPreset::Gentle);
        assert_eq!(settings.tuning(), SoundPreset::Gentle.tuning());
        settings.profile = SoundProfile::Preset(SoundPreset::Strong);
        assert_eq!(settings.tuning().boost, 2.0);
        assert_eq!(SoundSettings::from_json(&settings.to_json()), settings);
        // Turned back by hand, it's the original again.
        settings.tune(|t| t.boost = SoundPreset::Strong.tuning().boost);
        assert!(!settings.changed());
        assert_eq!(settings.edited, [None; SoundPreset::COUNT]);
        settings.tune(|t| t.leveller = true);
        settings.reset();
        assert_eq!(settings.tuning(), SoundPreset::Strong.tuning());
        // Settings saved before presets could change still load.
        let old = r#"{"profile":{"Preset":"Strong"},"custom":[{},{}]}"#;
        assert_eq!(
            SoundSettings::from_json(old).edited,
            [None; SoundPreset::COUNT]
        );
        assert_eq!(
            SoundSettings::from_json(old).profile,
            SoundProfile::Preset(SoundPreset::Strong)
        );
    }

    #[test]
    fn saved_settings_stay_in_range() {
        let settings = SoundSettings::from_json(
            r#"{"profile":{"Custom":7},"custom":[{"cut_ratio":99,"ceiling":5},{}]}"#,
        );
        assert_eq!(settings.profile, SoundProfile::Custom(0));
        assert_eq!(settings.custom[0].cut_ratio, 20.0);
        assert_eq!(settings.custom[0].ceiling, 0.0);
        assert_eq!(
            settings.custom[0].attack,
            SoundTuning::default().attack,
            "missing fields take their defaults"
        );
        assert_eq!(
            SoundSettings::from_json("not json"),
            SoundSettings::default()
        );
    }
}
