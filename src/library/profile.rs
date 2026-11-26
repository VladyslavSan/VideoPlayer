//! This install's own things: its database, its folders and the id it
//! shows servers. One `Profile` is handed to the library and to each view
//! that reads or saves settings.

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result};

use super::Db;

/// Where the library database and thumbnails live.
#[derive(Debug, Clone)]
pub struct Paths {
    pub db: PathBuf,
    pub thumbs: PathBuf,
    /// Diagnostics of release builds, which have no console.
    #[cfg_attr(debug_assertions, allow(dead_code))]
    pub log: PathBuf,
}

impl Paths {
    /// `%LOCALAPPDATA%\VideoPlayer`, or `VIDEO_PLAYER_DATA_DIR` if set.
    pub fn resolve() -> Result<Self> {
        let base = match std::env::var_os("VIDEO_PLAYER_DATA_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => dirs::data_local_dir()
                .context("no local app data directory")?
                .join("VideoPlayer"),
        };
        Ok(Self {
            db: base.join("library.db"),
            thumbs: base.join("thumbs"),
            log: base.join("app.log"),
        })
    }

    /// Seek bar preview pictures of the video playing (temporary).
    pub fn previews(&self) -> PathBuf {
        self.db.with_file_name("previews")
    }

    /// Pictures from Sonarr, Radarr and the metadata sites. (Their old
    /// folder, `arr`, may hold ones that show up green: `arr::readable`.)
    pub fn arr_pictures(&self) -> PathBuf {
        self.thumbs.join("arr-pictures")
    }

    /// Where the library last fetched from a server is kept.
    pub fn library_cache(&self, server: i64) -> PathBuf {
        self.db.with_file_name(format!("jellyfin-{server}.json"))
    }
}

/// This install's database, folders and device id.
#[derive(Debug, Clone)]
pub struct Profile {
    /// The one connection the UI thread uses, shared by everything on it.
    pub db: Rc<Db>,
    pub paths: Paths,
    /// What this install calls itself to Jellyfin servers.
    pub device_id: String,
}

impl Profile {
    /// Opens (or creates) the database at `paths.db`.
    pub fn open(paths: Paths) -> Result<Self> {
        let db = Db::open(&paths.db)?;
        let device_id = db.device_id()?;
        Ok(Self {
            db: Rc::new(db),
            paths,
            device_id,
        })
    }
}
