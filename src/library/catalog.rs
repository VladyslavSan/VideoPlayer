//! What the library holds right now: local files, each server's items, the
//! folders scanned and the servers signed in to. The `Library` changes it;
//! views keep a snapshot (`Arc<Catalog>`) to draw from.

use std::path::PathBuf;

use super::{Item, Source};
use crate::jellyfin::{Client, Server};

/// Every item, folder and server the library knows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    /// Files found in the scanned folders, as the database has them.
    pub(super) local: Vec<Item>,
    /// Every signed-in server's items.
    pub(super) remote: Vec<Item>,
    pub(super) roots: Vec<PathBuf>,
    pub(super) servers: Vec<Server>,
}

impl Catalog {
    /// Local files first, then the servers' items.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.local.iter().chain(self.remote.iter())
    }

    pub fn item(&self, key: &str) -> Option<&Item> {
        self.items().find(|item| item.key() == key)
    }

    pub fn local(&self) -> &[Item] {
        &self.local
    }

    pub fn remote(&self) -> &[Item] {
        &self.remote
    }

    /// The folders scanned for local files.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn servers(&self) -> &[Server] {
        &self.servers
    }

    pub fn server(&self, id: i64) -> Option<&Server> {
        self.servers.iter().find(|server| server.id == id)
    }

    /// The account's id on a signed-in server.
    pub fn user_id(&self, server: i64) -> Option<String> {
        self.server(server).map(|server| server.user_id.clone())
    }

    /// A client for a signed-in server, calling itself `device_id`.
    pub fn client(&self, server: i64, device_id: &str) -> Option<Client> {
        self.server(server)
            .map(|server| Client::for_server(server, device_id))
    }

    /// Whether there's a folder or a server to show anything from.
    pub fn has_sources(&self) -> bool {
        !self.roots.is_empty() || !self.servers.is_empty()
    }

    /// Whether anything of `server`'s is held (fetched, or from its cache).
    pub(super) fn has_items_of(&self, server: i64) -> bool {
        self.remote.iter().any(|item| is_from(item, server))
    }

    /// Where `server`'s item `id` sits among the servers' items.
    pub(super) fn remote_index(&self, server: i64, id: &str) -> Option<usize> {
        self.remote.iter().position(|item| {
            matches!(&item.source, Source::Jellyfin { server: s, id: i } if *s == server && i == id)
        })
    }

    /// Puts `items` in place of everything held of `server`'s.
    pub(super) fn replace_server_items(&mut self, server: i64, items: Vec<Item>) {
        self.remove_server_items(server);
        self.remote.extend(items);
    }

    pub(super) fn remove_server_items(&mut self, server: i64) {
        self.remote.retain(|item| !is_from(item, server));
    }

    /// Marks a server's item watched or not; either way its resume point
    /// goes. `now` (Unix seconds) is when it was watched.
    pub(super) fn mark_watched(&mut self, key: &str, watched: bool, now: i64) {
        let Some(item) = self.remote.iter_mut().find(|item| item.key() == key) else {
            return;
        };
        item.watched = watched;
        item.position = 0.0;
        if watched {
            item.last_played = Some(now);
        }
    }
}

fn is_from(item: &Item, server: i64) -> bool {
    matches!(&item.source, Source::Jellyfin { server: s, .. } if *s == server)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A movie on `server`, never played.
    pub(in crate::library) fn movie(server: i64, id: &str) -> Item {
        Item {
            library: None,
            source: Source::Jellyfin {
                server,
                id: id.into(),
            },
            title: format!("Movie {id}"),
            year: None,
            show: None,
            season: None,
            episode: None,
            duration: Some(6000.0),
            position: 0.0,
            watched: false,
            last_played: None,
            thumb: None,
            added: None,
        }
    }

    pub(in crate::library) fn server(id: i64, name: &str) -> Server {
        Server {
            id,
            url: format!("http://{name}.invalid"),
            name: name.into(),
            server_id: format!("server-{id}"),
            user_id: format!("user-{id}"),
            user_name: "viewer".into(),
            token: "token".into(),
        }
    }

    fn local_file(id: i64) -> Item {
        Item {
            source: Source::Local {
                id,
                path: PathBuf::from(format!("/films/{id}.mkv")),
            },
            ..movie(0, "unused")
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            local: vec![local_file(7)],
            remote: vec![movie(1, "a"), movie(2, "a"), movie(1, "b")],
            roots: vec![PathBuf::from("/films")],
            servers: vec![server(1, "attic"), server(2, "cellar")],
        }
    }

    #[test]
    fn items_are_found_by_key_across_local_files_and_servers() {
        let catalog = catalog();
        assert_eq!(catalog.items().count(), 4);
        assert_eq!(catalog.item("local-7"), Some(&catalog.local[0]));
        assert_eq!(catalog.item("jf-2-a"), Some(&catalog.remote[1]));
        assert_eq!(catalog.item("jf-3-a"), None);
    }

    #[test]
    fn the_same_item_id_on_two_servers_is_two_items() {
        let catalog = catalog();
        assert_eq!(catalog.remote_index(1, "a"), Some(0));
        assert_eq!(catalog.remote_index(2, "a"), Some(1));
        assert_eq!(catalog.remote_index(2, "b"), None);
    }

    #[test]
    fn a_server_is_looked_up_by_its_row() {
        let catalog = catalog();
        assert_eq!(catalog.server(2).map(|s| s.name.as_str()), Some("cellar"));
        assert_eq!(catalog.user_id(1).as_deref(), Some("user-1"));
        assert_eq!(catalog.user_id(9), None);
        assert!(catalog.client(9, "device").is_none());
    }

    #[test]
    fn a_library_with_no_folder_and_no_server_has_no_sources() {
        assert!(!Catalog::default().has_sources());
        assert!(catalog().has_sources());
    }

    #[test]
    fn a_fetch_replaces_only_its_own_servers_items() {
        let mut catalog = catalog();
        catalog.replace_server_items(1, vec![movie(1, "c")]);
        let keys: Vec<String> = catalog.remote.iter().map(Item::key).collect();
        assert_eq!(keys, ["jf-2-a", "jf-1-c"]);
        assert!(catalog.has_items_of(2));
        catalog.remove_server_items(2);
        assert!(!catalog.has_items_of(2));
    }

    #[test]
    fn marking_watched_drops_the_resume_point() {
        let mut catalog = catalog();
        catalog.remote[0].position = 300.0;
        catalog.mark_watched("jf-1-a", true, 1_000);
        let item = &catalog.remote[0];
        assert!(item.watched);
        assert_eq!(item.position, 0.0);
        assert_eq!(item.last_played, Some(1_000));

        // Unwatching keeps when it was last played.
        catalog.mark_watched("jf-1-a", false, 2_000);
        let item = &catalog.remote[0];
        assert!(!item.watched);
        assert_eq!(item.last_played, Some(1_000));
    }
}
