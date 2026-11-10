use std::collections::HashSet;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Result, bail};
use walkdir::{DirEntry, WalkDir};

use super::db::{Db, FileInfo, path_key};
use super::parse::parse_path;

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "ts", "m2ts", "webm", "mpg", "mpeg", "flv", "ogv",
];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanStats {
    pub found: usize,
    pub removed: usize,
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

fn is_skipped_dir(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy().to_lowercase();
    entry.depth() > 0
        && entry.file_type().is_dir()
        && (name.starts_with('.')
            || name.starts_with('$')
            || matches!(
                name.as_str(),
                "system volume information" | "sample" | "samples" | "extras" | "featurettes"
            ))
}

fn is_sample(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    stem == "sample" || stem.ends_with("-sample") || stem.ends_with(".sample")
}

/// Adds or refreshes every video under `root` and drops items that vanished.
pub fn scan_root(db: &Db, root: &Path) -> Result<ScanStats> {
    // An unplugged drive must not wipe its items, and their progress, from the library.
    if !root.is_dir() {
        bail!("{} is not available", root.display());
    }
    db.transaction(|db| {
        let mut present = HashSet::new();
        let walker = WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| !is_skipped_dir(e))
            .filter_map(Result::ok);
        for entry in walker {
            let path = entry.path();
            if !entry.file_type().is_file() || !is_video(path) || is_sample(path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            db.upsert_file(
                root,
                &FileInfo {
                    path: path.to_path_buf(),
                    size: meta.len(),
                    mtime,
                    parsed: parse_path(path),
                },
            )?;
            present.insert(path_key(path));
        }
        let removed = db.remove_missing(root, &present)?;
        Ok(ScanStats {
            found: present.len(),
            removed,
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn scans_videos_and_skips_noise() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        for f in [
            "Movies/Heat (1995)/Heat (1995).mkv",
            "Movies/Heat (1995)/sample.mkv",
            "Movies/Heat (1995)/Heat.nfo",
            "TV/Severance/Season 1/Severance.S01E01.mkv",
            "TV/Severance/Season 1/Severance.S01E02.MP4",
            "TV/Severance/Extras/Making Of.mkv",
            ".hidden/secret.mkv",
        ] {
            let path = root.join(f);
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(&path, b"x")?;
        }

        let db = Db::open_in_memory()?;
        let stats = scan_root(&db, root)?;
        assert_eq!(
            stats,
            ScanStats {
                found: 3,
                removed: 0
            }
        );
        let mut names: Vec<_> = db.items()?.into_iter().map(|i| i.display_title()).collect();
        names.sort();
        assert_eq!(
            names,
            ["Heat", "Severance · S1 · E1", "Severance · S1 · E2"]
        );

        fs::remove_file(root.join("TV/Severance/Season 1/Severance.S01E02.MP4"))?;
        assert_eq!(
            scan_root(&db, root)?,
            ScanStats {
                found: 2,
                removed: 1
            }
        );
        Ok(())
    }

    #[test]
    fn missing_root_keeps_items() -> Result<()> {
        let db = Db::open_in_memory()?;
        let missing = Path::new("Z:/definitely/not/here");
        assert!(scan_root(&db, missing).is_err());
        Ok(())
    }
}
