//! Animation settings: whether to animate at all (not when the system asks
//! for less motion), and the app's standard timings.

use std::sync::OnceLock;
use std::time::Duration;

/// A page appearing.
pub const PAGE: Duration = Duration::from_millis(220);
/// A season folding open or shut.
pub const FOLD: Duration = Duration::from_millis(200);

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

/// 0 → 1, fast at first and settling gently.
pub fn ease_out(delta: f32) -> f32 {
    1.0 - (1.0 - delta).powi(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_runs_from_zero_to_one() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.5);
    }
}
