//! The values mpv owns, as the player last read them.

use crate::library::SubtitleTuning;
use crate::video::VideoSurface;

/// Values mpv owns, re-read every tick so the controls never drift from it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Status {
    pub(super) position: f64,
    pub(super) duration: f64,
    pub(super) buffered: Option<f64>,
    pub(super) paused: bool,
    pub(super) buffering: bool,
    pub(super) ended: bool,
    pub(super) volume: f64,
    pub(super) muted: bool,
    pub(super) speed: f64,
    pub(super) subtitle_scale: f64,
    pub(super) subtitle_delay: f64,
}

impl Status {
    /// Before mpv has said anything: loading, at the saved volume and tuning.
    pub(super) fn starting(volume: f64, tuning: SubtitleTuning) -> Self {
        Self {
            position: 0.0,
            duration: 0.0,
            buffered: None,
            paused: false,
            buffering: true,
            ended: false,
            volume,
            muted: false,
            speed: 1.0,
            subtitle_scale: tuning.scale,
            subtitle_delay: tuning.delay,
        }
    }

    /// What mpv says now.
    pub(super) fn read(video: &VideoSurface) -> Self {
        Self {
            position: video.position(),
            duration: video.duration(),
            buffered: video.buffered_until(),
            paused: video.paused(),
            buffering: video.buffering() || !video.loaded(),
            ended: video.ended(),
            volume: video.volume(),
            muted: video.muted(),
            speed: video.speed(),
            subtitle_scale: video.subtitle_scale(),
            subtitle_delay: video.subtitle_delay(),
        }
    }
}
