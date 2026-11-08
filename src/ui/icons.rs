//! Icons drawn by GPUI's SVG element. GPUI uses an SVG's shape as a mask and
//! colors it with the element's text color, so the files' own colors don't matter.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString, Svg, svg};

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon { $($variant),* }

        impl Icon {
            fn path(self) -> &'static str {
                match self { $(Icon::$variant => concat!("icons/", $file)),* }
            }
        }

        const FILES: &[(&str, &[u8])] = &[
            $((concat!("icons/", $file), include_bytes!(concat!("../../assets/icons/", $file)))),*
        ];
    };
}

icons! {
    Back => "back.svg",
    Check => "check.svg",
    ChevronDown => "chevron-down.svg",
    Close => "close.svg",
    Episodes => "episodes.svg",
    NextEpisode => "next-episode.svg",
    Fullscreen => "fullscreen.svg",
    FullscreenExit => "fullscreen-exit.svg",
    Minus => "minus.svg",
    Pause => "pause.svg",
    Play => "play.svg",
    Plus => "plus.svg",
    SkipBack => "skip-back.svg",
    SkipForward => "skip-forward.svg",
    Subtitles => "subtitles.svg",
    Volume => "volume.svg",
    VolumeMuted => "volume-muted.svg",
}

/// An icon element; size it and set its color with `text_color`.
pub fn icon(icon: Icon) -> Svg {
    svg().path(icon.path())
}

/// Assets compiled into the binary, so the app runs from any folder.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
