//! A card's picture: once it can be drawn it dissolves in over whatever
//! the card showed while it waited (its letter tile, a plain tile, or the
//! picture it had before), so a poster never pops in. A picture ready
//! when its card first shows just comes with the page.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::FutureExt;
use gpui::{
    Animation, AnimationExt, AnyElement, Context, ImgResourceLoader, ObjectFit, Resource,
    SharedString, div, img, prelude::*, rgb,
};

use super::Motion;
use crate::motion::{self, APPEAR};

/// Ready this soon after the card showed, it came with the card.
const WITH_THE_CARD: Duration = Duration::from_millis(30);
/// A tile with nothing on it yet.
const BLANK: u32 = 0x1a1d26;

/// What a card shows under its picture while that loads or fades in.
#[derive(Clone)]
enum Under {
    Blank,
    Letter,
    Picture(PathBuf),
}

struct CardPicture {
    path: Option<PathBuf>,
    /// Since when it has shown `path` (or waited for it).
    since: Instant,
    /// When it could be drawn, and whether it loaded.
    settled: Option<(Instant, bool)>,
    under: Under,
}

/// Each card's picture, by card id.
#[derive(Default)]
pub(super) struct Pictures(HashMap<SharedString, CardPicture>);

/// Whether the picture at `path` is loaded (`Some(true)`) or failed. `None`
/// while it's still loading; the view is redrawn when it's done.
fn loaded<V: 'static>(path: &Path, cx: &mut Context<V>) -> Option<bool> {
    let source = Resource::Path(path.into());
    let (task, first) = cx.fetch_asset::<ImgResourceLoader>(&source);
    if let Some(result) = task.clone().now_or_never() {
        return Some(result.is_ok());
    }
    if first {
        cx.spawn(async move |this, cx| {
            task.await.ok();
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }
    None
}

impl Motion {
    /// The picture part of card `id`: `thumb` if there is one, else
    /// `letter` (its placeholder tile).
    pub(in crate::shell) fn card_picture<V: 'static>(
        &self,
        id: &SharedString,
        thumb: Option<PathBuf>,
        letter: impl Fn() -> AnyElement,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let ready = thumb.as_deref().and_then(|path| loaded(path, cx));
        let now = Instant::now();
        let mut pictures = self.pictures.borrow_mut();
        let entry = pictures.0.entry(id.clone()).or_insert_with(|| CardPicture {
            path: thumb.clone(),
            since: now,
            settled: None,
            under: Under::Blank,
        });
        if entry.path != thumb {
            // What it showed until now goes under the new one.
            entry.under = match (&entry.path, entry.settled) {
                (Some(old), Some((_, true))) => Under::Picture(old.clone()),
                (Some(_), None) => Under::Blank,
                _ => Under::Letter,
            };
            entry.path = thumb.clone();
            entry.since = now;
            entry.settled = None;
        }
        let Some(path) = thumb else {
            return letter();
        };
        if entry.settled.is_none()
            && let Some(ok) = ready
        {
            entry.settled = Some((now, ok));
        }
        let draw_under = |under: &Under| match under {
            Under::Blank => div().size_full().bg(rgb(BLANK)).into_any_element(),
            Under::Letter => letter(),
            Under::Picture(old) => picture(old.clone()),
        };
        let Some((at, ok)) = entry.settled else {
            return draw_under(&entry.under);
        };
        let shown = if ok { picture(path.clone()) } else { letter() };
        let late = at.duration_since(entry.since) > WITH_THE_CARD;
        if !late || !self.animates() || at.elapsed() >= APPEAR.time() {
            return shown;
        }
        div()
            .relative()
            .size_full()
            .child(draw_under(&entry.under))
            .child(div().absolute().inset_0().child(shown).with_animation(
                SharedString::from(format!("picture-{id}-{}", path.display())),
                Animation::new(APPEAR.time()).with_easing(motion::ease_in_out),
                |layer, t| layer.opacity(t),
            ))
            .into_any_element()
    }
}

fn picture(path: PathBuf) -> AnyElement {
    img(path)
        .size_full()
        .object_fit(ObjectFit::Cover)
        .into_any_element()
}
