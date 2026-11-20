//! Helpers for tests that run the real libmpv on a generated clip.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// A 5 s, 320x240 clip with sound, made with `ffmpeg`. `None` (with the
/// reason printed) when `ffmpeg` can't be run, so the test is skipped.
pub fn make() -> Option<(TempDir, PathBuf)> {
    let dir = tempfile::tempdir().ok()?;
    let path = dir.path().join("clip.mp4");
    let made = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=5:size=320x240:rate=25")
        .args(["-f", "lavfi", "-i", "sine=duration=5"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .arg(&path)
        .stdin(Stdio::null())
        .status();
    match made {
        Ok(status) if status.success() => Some((dir, path)),
        Ok(status) => {
            eprintln!("skipped: ffmpeg could not make the clip ({status})");
            None
        }
        Err(e) => {
            eprintln!("skipped: ffmpeg is not available ({e})");
            None
        }
    }
}

/// Width and height of a baseline or progressive JPEG, from its SOF marker.
pub fn jpeg_size(path: &Path) -> Option<(u32, u32)> {
    let bytes = std::fs::read(path).ok()?;
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    while at + 9 < bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        let length = usize::from(bytes[at + 2]) << 8 | usize::from(bytes[at + 3]);
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = u32::from(bytes[at + 5]) << 8 | u32::from(bytes[at + 6]);
            let width = u32::from(bytes[at + 7]) << 8 | u32::from(bytes[at + 8]);
            return Some((width, height));
        }
        at += 2 + length;
    }
    None
}
