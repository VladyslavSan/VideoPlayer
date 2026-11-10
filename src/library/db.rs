use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::parse::Parsed;
use super::{Item, MIN_RESUME_SECONDS, Source, SubtitlePref, TrackPrefs, WATCHED_FRACTION};
use crate::jellyfin::Server;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS roots (path TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS items (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    root TEXT NOT NULL,
    title TEXT NOT NULL,
    year INTEGER,
    show TEXT,
    season INTEGER,
    episode INTEGER,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    added INTEGER NOT NULL,
    duration REAL,
    position REAL NOT NULL DEFAULT 0,
    watched INTEGER NOT NULL DEFAULT 0,
    last_played INTEGER,
    -- NULL: not generated yet; '': generation failed.
    thumb TEXT
);
CREATE INDEX IF NOT EXISTS items_root ON items(root);
CREATE TABLE IF NOT EXISTS servers (
    id INTEGER PRIMARY KEY,
    url TEXT NOT NULL,
    name TEXT NOT NULL,
    server_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    user_name TEXT NOT NULL,
    -- Always '' now: tokens live in the system credential store. Versions
    -- before that stored them here; they are moved out on startup.
    token TEXT NOT NULL,
    UNIQUE (server_id, user_id)
);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- Remembered track choices. scope: 'general' or 'show:<name>'.
-- subtitle: NULL = no choice yet, '' = off, else a language code.
CREATE TABLE IF NOT EXISTS track_prefs (scope TEXT PRIMARY KEY, audio TEXT, subtitle TEXT);
";

/// A video file found on disk.
pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
    pub parsed: Parsed,
}

pub struct Db {
    conn: Connection,
}

pub fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Runs `f` in one transaction, which makes bulk scans much faster.
    pub fn transaction<T>(&self, f: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN")?;
        match f(self) {
            Ok(value) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    pub fn roots(&self) -> Result<Vec<PathBuf>> {
        let mut stmt = self.conn.prepare("SELECT path FROM roots ORDER BY path")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows
            .map(|r| r.map(PathBuf::from))
            .collect::<Result<_, _>>()?)
    }

    /// Returns true if the folder was not already in the library.
    pub fn add_root(&self, root: &Path) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO roots (path) VALUES (?1)",
            [path_key(root)],
        )?;
        Ok(n > 0)
    }

    /// Inserts a file, or refreshes its metadata while keeping watch progress.
    /// A changed size or mtime means a new file, so its thumbnail is redone.
    pub fn upsert_file(&self, root: &Path, file: &FileInfo) -> Result<()> {
        let (title, year, show, season, episode) = match &file.parsed {
            Parsed::Movie { title, year } => (title.clone(), *year, None, None, None),
            Parsed::Episode {
                show,
                season,
                episode,
                title,
            } => (
                title
                    .clone()
                    .unwrap_or_else(|| format!("Episode {episode}")),
                None,
                Some(show.clone()),
                Some(*season),
                Some(*episode),
            ),
        };
        self.conn.execute(
            "INSERT INTO items (path, root, title, year, show, season, episode, size, mtime, added)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(path) DO UPDATE SET
                root = excluded.root, title = excluded.title, year = excluded.year,
                show = excluded.show, season = excluded.season, episode = excluded.episode,
                thumb = CASE WHEN items.size != excluded.size OR items.mtime != excluded.mtime
                             THEN NULL ELSE items.thumb END,
                size = excluded.size, mtime = excluded.mtime",
            params![
                path_key(&file.path),
                path_key(root),
                title,
                year,
                show,
                season,
                episode,
                file.size as i64,
                file.mtime,
                now(),
            ],
        )?;
        Ok(())
    }

    /// Deletes items under `root` whose paths were not seen in the latest scan.
    pub fn remove_missing(&self, root: &Path, present: &HashSet<String>) -> Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM items WHERE root = ?1")?;
        let stale: Vec<i64> = stmt
            .query_map([path_key(root)], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .filter_map(Result::ok)
            .filter(|(_, path)| !present.contains(path))
            .map(|(id, _)| id)
            .collect();
        for id in &stale {
            self.conn.execute("DELETE FROM items WHERE id = ?1", [id])?;
        }
        Ok(stale.len())
    }

    pub fn items(&self) -> Result<Vec<Item>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, title, year, show, season, episode, duration, position,
                    watched, last_played, thumb, added
             FROM items
             ORDER BY show COLLATE NOCASE, season, episode, title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], item_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    #[cfg(test)]
    pub fn item(&self, id: i64) -> Result<Option<Item>> {
        Ok(self.items()?.into_iter().find(|i| i.local_id() == Some(id)))
    }

    /// Records where playback stopped. Near the end, the item is marked
    /// watched and its position is reset so it starts over next time.
    pub fn save_progress(&self, id: i64, position: f64, duration: f64) -> Result<()> {
        let finished = duration > 0.0 && position >= duration * WATCHED_FRACTION;
        let position = if finished || position < MIN_RESUME_SECONDS {
            0.0
        } else {
            position
        };
        self.conn.execute(
            "UPDATE items SET position = ?2, duration = ?3, last_played = ?4,
                    watched = CASE WHEN ?5 THEN 1 ELSE watched END
             WHERE id = ?1",
            params![id, position, duration, now(), finished],
        )?;
        Ok(())
    }

    /// `None` records a failed attempt so it isn't retried on every launch.
    /// Marks a file watched or not; either way it starts from the beginning.
    pub fn set_watched(&self, id: i64, watched: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET watched = ?2, position = 0, last_played = ?3 WHERE id = ?1",
            params![id, watched, watched.then(now)],
        )?;
        Ok(())
    }

    pub fn set_thumb(&self, id: i64, thumb: Option<&Path>) -> Result<()> {
        let value = thumb.map(path_key).unwrap_or_default();
        self.conn.execute(
            "UPDATE items SET thumb = ?2 WHERE id = ?1",
            params![id, value],
        )?;
        Ok(())
    }

    pub fn pending_thumbs(&self) -> Result<Vec<(i64, PathBuf)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM items WHERE thumb IS NULL ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, PathBuf::from(r.get::<_, String>(1)?)))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

impl Db {
    pub fn servers(&self) -> Result<Vec<Server>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, url, name, server_id, user_id, user_name, token FROM servers ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Server {
                id: r.get(0)?,
                url: r.get(1)?,
                name: r.get(2)?,
                server_id: r.get(3)?,
                user_id: r.get(4)?,
                user_name: r.get(5)?,
                token: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Saves a sign-in, keyed by server and user. The token is not stored;
    /// callers put it in the credential store.
    pub fn save_server(&self, server: &Server) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO servers (url, name, server_id, user_id, user_name, token)
             VALUES (?1, ?2, ?3, ?4, ?5, '')
             ON CONFLICT(server_id, user_id) DO UPDATE SET
                url = excluded.url, name = excluded.name,
                user_name = excluded.user_name, token = ''",
            params![
                server.url,
                server.name,
                server.server_id,
                server.user_id,
                server.user_name,
            ],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM servers WHERE server_id = ?1 AND user_id = ?2",
            params![server.server_id, server.user_id],
            |r| r.get(0),
        )?)
    }

    pub fn remove_server(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM servers WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Wipes a token left by an older version, once it has been moved out.
    pub fn clear_token(&self, id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE servers SET token = '' WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn track_prefs(&self, scope: &str) -> Result<TrackPrefs> {
        let row: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT audio, subtitle FROM track_prefs WHERE scope = ?1",
                [scope],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (audio, subtitle) = row.unwrap_or_default();
        Ok(TrackPrefs {
            audio,
            subtitle: subtitle.map(|s| {
                if s.is_empty() {
                    SubtitlePref::Off
                } else {
                    SubtitlePref::Language(s)
                }
            }),
        })
    }

    pub fn save_audio_pref(&self, scope: &str, lang: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO track_prefs (scope, audio) VALUES (?1, ?2)
             ON CONFLICT(scope) DO UPDATE SET audio = excluded.audio",
            [scope, lang],
        )?;
        Ok(())
    }

    pub fn save_subtitle_pref(&self, scope: &str, pref: &SubtitlePref) -> Result<()> {
        let value = match pref {
            SubtitlePref::Off => "",
            SubtitlePref::Language(lang) => lang.as_str(),
        };
        self.conn.execute(
            "INSERT INTO track_prefs (scope, subtitle) VALUES (?1, ?2)
             ON CONFLICT(scope) DO UPDATE SET subtitle = excluded.subtitle",
            [scope, value],
        )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// A random id for this installation, created on first use. Jellyfin
    /// uses it to tell this player's sessions apart from other devices.
    pub fn device_id(&self) -> Result<String> {
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'device_id'",
                [],
                |r| r.get(0),
            )
            .ok();
        if let Some(id) = existing {
            return Ok(id);
        }
        use std::hash::{BuildHasher, RandomState};
        let id = format!(
            "{:016x}{:016x}",
            RandomState::new().hash_one(now()),
            RandomState::new().hash_one(std::process::id())
        );
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES ('device_id', ?1)",
            [&id],
        )?;
        Ok(id)
    }
}

fn item_from_row(r: &Row) -> rusqlite::Result<Item> {
    let thumb: Option<String> = r.get(11)?;
    Ok(Item {
        library: None,
        source: Source::Local {
            id: r.get(0)?,
            path: PathBuf::from(r.get::<_, String>(1)?),
        },
        title: r.get(2)?,
        year: r.get(3)?,
        show: r.get(4)?,
        season: r.get(5)?,
        episode: r.get(6)?,
        duration: r.get(7)?,
        position: r.get(8)?,
        watched: r.get(9)?,
        last_played: r.get(10)?,
        thumb: thumb.filter(|t| !t.is_empty()).map(PathBuf::from),
        added: r.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64, mtime: i64) -> FileInfo {
        let path = PathBuf::from(path);
        FileInfo {
            parsed: super::super::parse::parse_path(&path),
            path,
            size,
            mtime,
        }
    }

    #[test]
    fn upsert_keeps_progress_and_resets_thumb_on_change() -> Result<()> {
        let db = Db::open_in_memory()?;
        let root = Path::new("D:/Movies");
        db.upsert_file(root, &file("D:/Movies/Heat (1995).mkv", 100, 1))?;
        let id = db.items()?[0].local_id().unwrap();
        db.save_progress(id, 600.0, 6000.0)?;
        db.set_thumb(id, Some(Path::new("C:/thumbs/1.jpg")))?;

        // Rescan with the same file: progress and thumbnail survive.
        db.upsert_file(root, &file("D:/Movies/Heat (1995).mkv", 100, 1))?;
        let item = db.item(id)?.unwrap();
        assert_eq!(item.title, "Heat");
        assert_eq!(item.year, Some(1995));
        assert_eq!(item.position, 600.0);
        assert!(item.thumb.is_some());

        // File replaced on disk: thumbnail is regenerated, progress kept.
        db.upsert_file(root, &file("D:/Movies/Heat (1995).mkv", 200, 2))?;
        let item = db.item(id)?.unwrap();
        assert_eq!(item.position, 600.0);
        let Source::Local { path, .. } = item.source else {
            unreachable!()
        };
        assert_eq!(db.pending_thumbs()?, vec![(id, path)]);
        Ok(())
    }

    #[test]
    fn progress_rules() -> Result<()> {
        let db = Db::open_in_memory()?;
        db.upsert_file(Path::new("D:"), &file("D:/a.mkv", 1, 1))?;
        let id = db.items()?[0].local_id().unwrap();

        db.save_progress(id, 5.0, 1000.0)?;
        let item = db.item(id)?.unwrap();
        assert_eq!((item.position, item.watched), (0.0, false));
        assert_eq!(item.progress(), None);

        db.save_progress(id, 500.0, 1000.0)?;
        let item = db.item(id)?.unwrap();
        assert_eq!(item.progress(), Some(0.5));
        assert!(item.last_played.is_some());

        db.save_progress(id, 950.0, 1000.0)?;
        let item = db.item(id)?.unwrap();
        assert_eq!((item.position, item.watched), (0.0, true));
        Ok(())
    }

    #[test]
    fn remove_missing_only_touches_its_root() -> Result<()> {
        let db = Db::open_in_memory()?;
        let a = Path::new("D:/A");
        let b = Path::new("D:/B");
        db.upsert_file(a, &file("D:/A/one.mkv", 1, 1))?;
        db.upsert_file(a, &file("D:/A/two.mkv", 1, 1))?;
        db.upsert_file(b, &file("D:/B/three.mkv", 1, 1))?;

        let present = HashSet::from(["D:/A/one.mkv".to_string()]);
        assert_eq!(db.remove_missing(a, &present)?, 1);
        let titles: Vec<_> = db.items()?.into_iter().map(|i| i.title).collect();
        assert_eq!(titles, ["one", "three"]);
        Ok(())
    }

    #[test]
    fn failed_thumb_is_not_retried() -> Result<()> {
        let db = Db::open_in_memory()?;
        db.upsert_file(Path::new("D:"), &file("D:/a.mkv", 1, 1))?;
        let id = db.items()?[0].local_id().unwrap();
        db.set_thumb(id, None)?;
        assert!(db.pending_thumbs()?.is_empty());
        assert_eq!(db.item(id)?.unwrap().thumb, None);
        Ok(())
    }

    #[test]
    fn servers_upsert_by_user() -> Result<()> {
        let db = Db::open_in_memory()?;
        let mut server = Server {
            id: 0,
            url: "http://nas:8096".into(),
            name: "Home".into(),
            server_id: "s1".into(),
            user_id: "u1".into(),
            user_name: "me".into(),
            token: "secret".into(),
        };
        let id = db.save_server(&server)?;
        server.url = "https://nas.example".into();
        assert_eq!(db.save_server(&server)?, id);
        let saved = db.servers()?;
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, id);
        assert_eq!(saved[0].url, "https://nas.example");
        // Tokens never reach the database.
        assert_eq!(saved[0].token, "");
        db.remove_server(id)?;
        assert!(db.servers()?.is_empty());
        Ok(())
    }

    #[test]
    fn legacy_token_is_cleared() -> Result<()> {
        let db = Db::open_in_memory()?;
        db.conn.execute(
            "INSERT INTO servers (url, name, server_id, user_id, user_name, token)
             VALUES ('http://nas:8096', 'Home', 's1', 'u1', 'me', 'old-token')",
            [],
        )?;
        let saved = db.servers()?;
        assert_eq!(saved[0].token, "old-token");
        db.clear_token(saved[0].id)?;
        assert_eq!(db.servers()?[0].token, "");
        Ok(())
    }

    #[test]
    fn track_prefs_round_trip() -> Result<()> {
        let db = Db::open_in_memory()?;
        assert_eq!(db.track_prefs("show:X")?, TrackPrefs::default());
        db.save_audio_pref("show:X", "ukr")?;
        db.save_subtitle_pref("show:X", &SubtitlePref::Off)?;
        assert_eq!(
            db.track_prefs("show:X")?,
            TrackPrefs {
                audio: Some("ukr".into()),
                subtitle: Some(SubtitlePref::Off)
            }
        );
        db.save_subtitle_pref("show:X", &SubtitlePref::Language("eng".into()))?;
        let prefs = db.track_prefs("show:X")?;
        assert_eq!(prefs.audio.as_deref(), Some("ukr"));
        assert_eq!(prefs.subtitle, Some(SubtitlePref::Language("eng".into())));
        // Show choices win; gaps come from the general ones.
        db.save_audio_pref("general", "eng")?;
        db.save_subtitle_pref("general", &SubtitlePref::Off)?;
        let merged = TrackPrefs {
            audio: None,
            ..prefs
        }
        .or(db.track_prefs("general")?);
        assert_eq!(merged.audio.as_deref(), Some("eng"));
        assert_eq!(merged.subtitle, Some(SubtitlePref::Language("eng".into())));
        Ok(())
    }

    #[test]
    fn marking_watched() -> Result<()> {
        let db = Db::open_in_memory()?;
        let root = Path::new("D:/Movies");
        db.upsert_file(root, &file("D:/Movies/Film (2001).mkv", 1, 1))?;
        let id = db.items()?[0].local_id().unwrap();
        db.save_progress(id, 300.0, 1000.0)?;
        db.set_watched(id, true)?;
        let item = db.item(id)?.unwrap();
        assert!(item.watched && item.position == 0.0 && item.last_played.is_some());
        db.set_watched(id, false)?;
        assert!(!db.item(id)?.unwrap().watched);
        Ok(())
    }

    #[test]
    fn settings_round_trip() -> Result<()> {
        let db = Db::open_in_memory()?;
        assert_eq!(db.setting("volume")?, None);
        db.set_setting("volume", "80")?;
        db.set_setting("volume", "65")?;
        assert_eq!(db.setting("volume")?.as_deref(), Some("65"));
        Ok(())
    }

    #[test]
    fn device_id_is_stable() -> Result<()> {
        let db = Db::open_in_memory()?;
        let id = db.device_id()?;
        assert_eq!(id.len(), 32);
        assert_eq!(db.device_id()?, id);
        Ok(())
    }

    #[test]
    fn roots_are_unique() -> Result<()> {
        let db = Db::open_in_memory()?;
        assert!(db.add_root(Path::new("D:/Movies"))?);
        assert!(!db.add_root(Path::new("D:/Movies"))?);
        assert_eq!(db.roots()?, vec![PathBuf::from("D:/Movies")]);
        Ok(())
    }
}
