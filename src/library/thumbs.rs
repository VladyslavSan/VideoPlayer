//! Thumbnails grabbed from a frame of the video itself, as a stand-in until
//! online artwork (TMDB) is wired up.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use libmpv2::events::Event;

use crate::video::Snapshotter;

const WIDTH: u32 = 640;
const TIMEOUT: Duration = Duration::from_secs(20);

/// Writes a JPEG of the frame 20% into `video` to `out`. It's written under
/// another name first, so a half-written file is never shown.
pub fn generate(video: &Path, out: &Path) -> Result<()> {
    let partial = out.with_extension("part.jpg");
    let result = render_frame(video, &partial).and_then(|()| Ok(fs::rename(&partial, out)?));
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

fn render_frame(video: &Path, out: &Path) -> Result<()> {
    let hidden = Snapshotter::new(|init| {
        init.set_option("vf", format!("lavfi=[scale=w={WIDTH}:h=-2]").as_str())?;
        init.set_option("start", "20%")?;
        init.set_option("pause", "yes")?;
        init.set_option("keep-open", "always")?;
        init.set_option("sub", "no")?;
        init.set_option("hwdec", "no")?;
        init.set_option("screenshot-format", "jpg")?;
        init.set_option("screenshot-jpeg-quality", 85i64)?;
        Ok(())
    })?;
    let mpv = &hidden.mpv;
    let path = video.to_string_lossy();
    mpv.command("loadfile", &[path.as_ref()])
        .map_err(|e| anyhow!("loading {path}: {e}"))?;

    let deadline = Instant::now() + TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("timed out after {}s", TIMEOUT.as_secs());
        }
        match mpv.wait_event(left.as_secs_f64()) {
            Some(Ok(Event::PlaybackRestart)) => break,
            Some(Ok(Event::EndFile(_) | Event::Shutdown)) => bail!("mpv couldn't open the video"),
            Some(Err(e)) => bail!("mpv: {e}"),
            _ => {}
        }
    }
    mpv.command("screenshot-to-file", &[&out.to_string_lossy(), "video"])
        .map_err(|e| anyhow!("taking the frame: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_of_a_local_clip_has_the_thumbnail_width() {
        let Some((dir, clip)) = crate::test_clip::make() else {
            return;
        };
        let out = dir.path().join("thumb.jpg");
        generate(&clip, &out).expect("a thumbnail is made");
        assert_eq!(
            crate::test_clip::jpeg_size(&out),
            Some((WIDTH, WIDTH * 3 / 4))
        );
    }
}
