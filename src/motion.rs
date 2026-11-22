//! Animation settings: whether to animate at all (not when the system asks
//! for less motion), and the app's standard timings.

use std::sync::OnceLock;
use std::time::Duration;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_runs_from_zero_to_one() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert_eq!(ease_in_out(0.5), 0.5);
        assert!(ease_in_out(0.1) < 0.1);
    }
}
