//! Animation settings: whether to animate at all (not when the system asks
//! for less motion), and the app's standard timings.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// One of the app's standard timings. Each is a setting in the library
/// database (`key`), read at startup.
pub struct Timing {
    pub key: &'static str,
    pub label: &'static str,
    /// What it times, for its setting.
    pub about: &'static str,
    pub default_ms: u32,
    ms: AtomicU32,
}

impl Timing {
    const fn new(
        key: &'static str,
        label: &'static str,
        about: &'static str,
        default_ms: u32,
    ) -> Self {
        Self {
            key,
            label,
            about,
            default_ms,
            ms: AtomicU32::new(default_ms),
        }
    }

    pub fn ms(&self) -> u32 {
        self.ms.load(Ordering::Relaxed)
    }

    pub fn set_ms(&self, ms: u32) {
        self.ms
            .store(ms.clamp(TIMING_MIN_MS, TIMING_MAX_MS), Ordering::Relaxed);
    }

    /// How long it takes (stretched by `VIDEO_PLAYER_MOTION_SCALE`).
    pub fn time(&self) -> Duration {
        duration(Duration::from_millis(self.ms() as u64))
    }
}

/// The range a timing can be set to.
pub const TIMING_MIN_MS: u32 = 40;
pub const TIMING_MAX_MS: u32 = 1500;

/// A page appearing, and the one it replaces fading out.
pub static PAGE: Timing = Timing::new(
    "motion_page_ms",
    "Page change",
    "A new page fading in over the one you left.",
    220,
);
/// A picture fading in over another (a page header's full backdrop):
/// slower than a page, so one image dissolves into the next.
pub static PICTURE: Timing = Timing::new(
    "motion_picture_ms",
    "Header picture",
    "A page's full-size picture fading in over the small one.",
    500,
);
/// A season folding open or shut.
pub static FOLD: Timing = Timing::new(
    "motion_fold_ms",
    "Season fold",
    "A season's episodes opening or closing.",
    200,
);
/// A poster lighting up under the mouse, or dimming after it.
pub static HOVER: Timing = Timing::new(
    "motion_hover_ms",
    "Poster under the mouse",
    "A poster brightening under the mouse, and dimming after.",
    160,
);
/// A mouse wheel step gliding to a stop (about 95% of the way).
pub static WHEEL: Timing = Timing::new(
    "motion_wheel_ms",
    "Wheel scroll",
    "A mouse wheel step gliding to a stop.",
    150,
);

/// Something arriving on a page already open (Sonarr's answer, an
/// overview) fading in.
pub static APPEAR: Timing = Timing::new(
    "motion_appear_ms",
    "Content arriving",
    "Something that loads after its page opened fading in.",
    350,
);
/// How long something takes to load before a note says it's loading.
pub static WAIT: Timing = Timing::new(
    "motion_wait_ms",
    "Loading note delay",
    "How long a wait goes on before \"Loading…\" fades in.",
    400,
);

pub static TIMINGS: [&Timing; 7] = [&PAGE, &PICTURE, &APPEAR, &WAIT, &FOLD, &HOVER, &WHEEL];

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

/// Slow at both ends, for something dissolving in (a picture over another,
/// text arriving): easing out alone does most of the change in the first
/// moments, and reads as a pop.
pub fn ease_in_out(delta: f32) -> f32 {
    delta * delta * (3.0 - 2.0 * delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timings_stay_in_range() {
        let timing = Timing::new("test", "Test", "A test.", 200);
        timing.set_ms(0);
        assert_eq!(timing.ms(), TIMING_MIN_MS);
        timing.set_ms(99_999);
        assert_eq!(timing.ms(), TIMING_MAX_MS);
    }

    #[test]
    fn easing_runs_from_zero_to_one() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.5);
    }
}
