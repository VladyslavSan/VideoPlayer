//! Animation settings: whether to animate at all (not when the system asks
//! for less motion), and the app's standard timings.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// One of the app's standard timings. They're all here, so each kind of
/// motion is tuned in one place.
pub struct Timing {
    ms: u64,
}

impl Timing {
    const fn ms(ms: u64) -> Self {
        Self { ms }
    }

    /// How long it takes (stretched by `VIDEO_PLAYER_MOTION_SCALE`).
    pub fn time(&self) -> Duration {
        duration(Duration::from_millis(self.ms))
    }
}

/// A page appearing, and the one it replaces fading out.
pub const PAGE: Timing = Timing::ms(350);
/// A picture fading in over another (a page header's full backdrop).
pub const PICTURE: Timing = Timing::ms(350);
/// Something that loads after its page opened (Sonarr's answer, an
/// overview) fading in.
pub const APPEAR: Timing = Timing::ms(350);
/// Something on a page growing or shrinking to its new size (a loading
/// note turning into a long description).
pub const RESIZE: Timing = Timing::ms(350);
/// How long something takes to load before a note says it's loading.
pub const WAIT: Timing = Timing::ms(350);
/// A season folding open or shut.
pub const FOLD: Timing = Timing::ms(350);
/// A poster lighting up under the mouse, or dimming after it.
pub const HOVER: Timing = Timing::ms(350);
/// The player's controls, panels and notes fading in or out, and menus
/// opening and closing (also the subtitles moving out of the control bar's
/// way, and the black lifting off the video once it's on screen).
pub const OVERLAY: Timing = Timing::ms(350);
/// A mouse wheel step gliding to a stop (about 95% of the way).
pub const WHEEL: Timing = Timing::ms(350);

/// `base`, stretched by `VIDEO_PLAYER_MOTION_SCALE` (tests slow animations
/// down to see them midway).
pub fn duration(base: Duration) -> Duration {
    static SCALE: OnceLock<f32> = OnceLock::new();
    let scale = *SCALE.get_or_init(|| {
        std::env::var("VIDEO_PLAYER_MOTION_SCALE")
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0)
    });
    base.mul_f32(scale)
}

/// How far a page rises as it appears, in pixels.
pub const PAGE_RISE: f32 = 14.0;

/// Off with Windows' "Animation effects" or macOS's "Reduce motion", or
/// with `VIDEO_PLAYER_NO_MOTION` set.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIDEO_PLAYER_NO_MOTION").is_none() && system_allows())
}

#[cfg(windows)]
fn system_allows() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut on: i32 = 1;
    let asked = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut i32 as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    asked.is_err() || on != 0
}

#[cfg(target_os = "macos")]
fn system_allows() -> bool {
    use objc::runtime::{BOOL, NO, Object};
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() {
            return true;
        }
        let reduce: BOOL = msg_send![workspace, accessibilityDisplayShouldReduceMotion];
        reduce == NO
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_allows() -> bool {
    true
}

/// 0 → 1, slow at both ends: the app's one easing curve. (Easing out
/// alone does most of the change in its first moments, and reads as a
/// pop.)
pub fn ease_in_out(delta: f32) -> f32 {
    delta * delta * (3.0 - 2.0 * delta)
}

/// Something that's shown or not, fading between the two.
#[derive(Debug, Clone, Copy)]
pub struct Fade {
    shown: bool,
    /// Where it started from when it last turned.
    from: f32,
    at: Instant,
}

impl Fade {
    pub fn new(shown: bool) -> Self {
        Self {
            shown,
            from: if shown { 1.0 } else { 0.0 },
            at: Instant::now(),
        }
    }

    /// Starts fading in or out, from wherever it is now.
    pub fn set(&mut self, shown: bool) {
        if shown != self.shown {
            self.from = self.value();
            self.shown = shown;
            self.at = Instant::now();
        }
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// How visible it is now: 0 (gone) to 1.
    pub fn value(&self) -> f32 {
        if !enabled() {
            return self.target();
        }
        fade_value(self.from, self.target(), self.at.elapsed(), OVERLAY.time())
    }

    /// Still to be drawn: shown, or fading out.
    pub fn on_screen(&self) -> bool {
        self.shown || self.value() > 0.0
    }

    /// On its way in or out.
    pub fn moving(&self) -> bool {
        self.value() != self.target()
    }

    fn target(&self) -> f32 {
        if self.shown { 1.0 } else { 0.0 }
    }
}

/// From `from` toward `to`, `elapsed` into a fade that takes `full` to
/// cross the whole way (a shorter way takes less).
fn fade_value(from: f32, to: f32, elapsed: Duration, full: Duration) -> f32 {
    let span = (to - from).abs();
    let total = full.as_secs_f32() * span;
    if total <= 0.0 {
        return to;
    }
    let progress = elapsed.as_secs_f32() / total;
    if progress >= 1.0 {
        // Exactly there, so it stops counting as moving.
        return to;
    }
    from + (to - from) * ease_in_out(progress.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: Duration = Duration::from_millis(400);

    #[test]
    fn fades_ease_all_the_way() {
        assert_eq!(fade_value(0.0, 1.0, Duration::ZERO, FULL), 0.0);
        assert_eq!(fade_value(0.0, 1.0, FULL / 2, FULL), 0.5);
        assert_eq!(fade_value(0.0, 1.0, FULL, FULL), 1.0);
        assert_eq!(fade_value(1.0, 0.0, FULL * 2, FULL), 0.0);
    }

    #[test]
    fn turning_back_halfway_takes_half_as_long() {
        // From halfway out, back in: done in half the time, no jump.
        assert_eq!(fade_value(0.5, 1.0, Duration::ZERO, FULL), 0.5);
        assert_eq!(fade_value(0.5, 1.0, FULL / 2, FULL), 1.0);
        assert_eq!(fade_value(0.3, 1.0, FULL, FULL), 1.0);
        assert!(fade_value(0.5, 1.0, FULL / 4, FULL) > 0.5);
    }

    #[test]
    fn easing_runs_from_zero_to_one() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert_eq!(ease_in_out(0.5), 0.5);
        assert!(ease_in_out(0.1) < 0.1);
    }
}
