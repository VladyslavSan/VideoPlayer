//! The local half of the library: the folders the viewer added, scanning
//! them for videos, and making a thumbnail for each one found.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui::Context;

use super::{Db, Library, scan, thumbs};

/// Scanning folders and making thumbnails, as far as each has come.
#[derive(Debug, Default)]
pub(super) struct Scanning {
    pub(super) scanning: bool,
    thumbnails_running: bool,
    pub(super) thumbnails_left: usize,
}

impl Library {
    /// Adds folders to scan, and scans.
    pub fn add_folders(&mut self, folders: &[PathBuf], cx: &mut Context<Self>) {
        for folder in folders {
            if let Err(e) = self.profile.db.add_root(folder) {
                self.error = Some(format!("Couldn't add {}: {e}", folder.display()).into());
            }
        }
        self.reload(cx);
        self.rescan(cx);
    }

    /// Stops scanning a folder; what was found in it goes.
    pub fn remove_folder(&mut self, folder: &Path, cx: &mut Context<Self>) {
        if let Err(e) = self.profile.db.remove_root(folder) {
            self.error = Some(format!("Couldn't remove {}: {e:#}", folder.display()).into());
        }
        self.reload(cx);
        cx.notify();
    }

    /// Looks through the folders again, in the background.
    pub(super) fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.scanning.scanning || self.catalog.roots().is_empty() {
            return;
        }
        self.scanning.scanning = true;
        let roots = self.catalog.roots().to_vec();
        let db_path = self.profile.paths.db.clone();
        cx.spawn(async move |this, cx| {
            let errors = cx
                .background_executor()
                .spawn(async move { scan_roots(&db_path, &roots) })
                .await;
            this.update(cx, |this, cx| {
                this.scanning.scanning = false;
                if !errors.is_empty() {
                    this.error = Some(errors.join("; ").into());
                }
                this.reload(cx);
                this.generate_thumbnails(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Generates missing local thumbnails one at a time in the background.
    pub(super) fn generate_thumbnails(&mut self, cx: &mut Context<Self>) {
        if self.scanning.thumbnails_running {
            return;
        }
        let pending = self.profile.db.pending_thumbs().unwrap_or_default();
        if pending.is_empty() {
            return;
        }
        self.scanning.thumbnails_running = true;
        self.scanning.thumbnails_left = pending.len();
        let folder = self.profile.paths.thumbs.clone();
        cx.spawn(async move |this, cx| {
            for (id, video) in pending {
                let picture = folder.join(format!("{id}.jpg"));
                let target = picture.clone();
                let made = cx
                    .background_executor()
                    .spawn(async move { make_thumbnail(&video, &target) })
                    .await;
                let made = made
                    .map(|()| picture)
                    .map_err(|e| eprintln!("[thumbs] item {id}: {e}"));
                let alive = this.update(cx, |this, cx| {
                    this.thumbnail_made(id, made.ok().as_deref(), cx)
                });
                if alive.is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.scanning.thumbnails_running = false;
                // Pick up anything a scan added while this batch ran.
                this.generate_thumbnails(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Records an item's thumbnail, or (None) that it couldn't be made.
    fn thumbnail_made(&mut self, id: i64, picture: Option<&Path>, cx: &mut Context<Self>) {
        if let Err(e) = self.profile.db.set_thumb(id, picture) {
            eprintln!("[thumbs] saving item {id}: {e}");
        }
        self.scanning.thumbnails_left = self.scanning.thumbnails_left.saturating_sub(1);
        self.reload(cx);
        cx.notify();
    }
}

/// Makes a video's thumbnail at `target`, and the folder for it if needed.
fn make_thumbnail(video: &Path, target: &Path) -> Result<()> {
    if let Some(folder) = target.parent() {
        fs::create_dir_all(folder)?;
    }
    thumbs::generate(video, target)
}

/// Scans every folder into the database at `db_path`. Returns what failed.
fn scan_roots(db_path: &Path, roots: &[PathBuf]) -> Vec<String> {
    // SQLite connections aren't shared across threads; open one here.
    let db = match Db::open(db_path) {
        Ok(db) => db,
        Err(e) => return vec![e.to_string()],
    };
    roots
        .iter()
        .filter_map(|root| scan::scan_root(&db, root).err())
        .map(|e| e.to_string())
        .collect()
}
