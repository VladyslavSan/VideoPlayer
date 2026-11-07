//! Thumbnails grabbed from a frame of the video itself, as a stand-in until
//! online artwork (TMDB) is wired up.

use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use libmpv2::Mpv;
use libmpv2::events::Event;

const WIDTH: u32 = 640;
const TIMEOUT_SECONDS: f64 = 20.0;

/// Writes a JPEG of the frame 20% into `video` to `out`.
pub fn generate(video: &Path, out: &Path) -> Result<()> {
    let work = out.with_extension("work");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = render_frame(video, &work).and_then(|()| {
        let frame = fs::read_dir(&work)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "jpg"))
            .ok_or_else(|| anyhow!("mpv produced no frame"))?;
        fs::rename(frame, out)?;
        Ok(())
    });
    let _ = fs::remove_dir_all(&work);
    result
}

fn render_frame(video: &Path, outdir: &Path) -> Result<()> {
    let mpv = Mpv::with_initializer(|init| {
        init.set_option("vo", "image")?;
        init.set_option("vo-image-format", "jpg")?;
        init.set_option("vo-image-jpeg-quality", 85i64)?;
        init.set_option("vo-image-outdir", outdir.to_string_lossy().as_ref())?;
        init.set_option("vf", format!("scale=w={WIDTH}:h=-2").as_str())?;
        init.set_option("start", "20%")?;
        init.set_option("frames", 1i64)?;
        init.set_option("audio", "no")?;
        init.set_option("sub", "no")?;
        init.set_option("hwdec", "no")?;
        init.set_option("idle", "no")?;
        init.set_option("config", "no")?;
        init.set_option("load-scripts", "no")?;
        Ok(())
    })
    .map_err(|e| anyhow!("initialising mpv: {e}"))?;

    let path = video.to_string_lossy();
    mpv.command("loadfile", &[path.as_ref()])
        .map_err(|e| anyhow!("loading {path}: {e}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(TIMEOUT_SECONDS);
    while std::time::Instant::now() < deadline {
        match mpv.wait_event(1.0) {
            Some(Ok(Event::EndFile(_) | Event::Shutdown)) => return Ok(()),
            Some(Err(e)) => bail!("mpv: {e}"),
            _ => {}
        }
    }
    bail!("timed out after {TIMEOUT_SECONDS}s")
}
