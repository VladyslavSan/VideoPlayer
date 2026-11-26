//! The Jellyfin half of the library: signing in to and out of servers,
//! fetching what each one has (shown from its cache until then), and
//! downloading the pictures for the cards.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use gpui::Context;

use super::{Event, Item, Library, sync};
use crate::jellyfin::{self, Client, ImageRef, Server};
use crate::secrets;

/// Fetching the servers' libraries and artwork, as far as it has come.
#[derive(Debug, Default)]
pub(super) struct Fetching {
    /// The servers whose library is being fetched.
    pub(super) loading: HashSet<i64>,
    /// Servers that changed during their fetch: fetched again after it.
    refetch: HashSet<i64>,
    /// Pictures still to download.
    pub(super) artwork_left: usize,
    /// When the servers' libraries were last fetched.
    pub(super) refreshed: Option<Instant>,
}

/// A fetch of one server's library under way.
struct Fetch {
    server: Server,
    client: Client,
    started: Instant,
}

/// An item a server has, and the picture for its card.
struct Fetched {
    item: Item,
    image: Option<ImageRef>,
}

/// A picture to download, and the items (by key) that show it.
struct Artwork {
    image: ImageRef,
    keys: Vec<String>,
}

impl Library {
    /// A client for a signed-in server.
    pub(super) fn client(&self, server: i64) -> Option<Client> {
        self.catalog.client(server, &self.profile.device_id)
    }

    /// Keeps a server just signed in to, and fetches its library.
    ///
    /// # Errors
    ///
    /// When the database can't save it; nothing has changed then.
    pub fn add_server(&mut self, server: &Server, cx: &mut Context<Self>) -> Result<()> {
        self.profile.db.save_server(server)?;
        self.reload(cx);
        self.refresh_remote(cx);
        Ok(())
    }

    /// Signs out of a server: its token, its items and its cache go.
    pub fn sign_out(&mut self, server_id: i64, cx: &mut Context<Self>) {
        if let Some(client) = self.client(server_id) {
            // Best effort: invalidate the token on the server too.
            cx.background_executor()
                .spawn(async move {
                    let _ = client.logout();
                })
                .detach();
        }
        let account = self
            .catalog
            .server(server_id)
            .map(|server| server.token_account(&self.profile.device_id));
        let removed = account
            .map_or(Ok(()), |account| secrets::delete(&account))
            .and_then(|()| self.profile.db.remove_server(server_id));
        if let Err(e) = removed {
            self.error = Some(format!("Couldn't sign out: {e:#}").into());
        }
        let _ = fs::remove_file(self.profile.paths.library_cache(server_id));
        if self.catalog.has_items_of(server_id) {
            self.catalog_mut(cx).remove_server_items(server_id);
        }
        self.reload(cx);
        self.sync_live(cx);
        cx.emit(Event::ServersChanged);
        cx.notify();
    }

    /// Fetches every server's library; artwork follows in the background.
    pub(super) fn refresh_remote(&mut self, cx: &mut Context<Self>) {
        self.fetching.refreshed = Some(Instant::now());
        self.sync_live(cx);
        cx.emit(Event::ServersChanged);
        for server in self.catalog.servers().to_vec() {
            if server.token.is_empty() {
                // Removed from the credential store outside the app.
                let advice = "signed out; sign in again in Settings → Library";
                self.error = Some(format!("{}: {advice}", server.name).into());
                continue;
            }
            if !self.fetching.loading.insert(server.id) {
                self.fetching.refetch.insert(server.id);
                continue;
            }
            self.fetch_server(server, cx);
        }
        cx.notify();
    }

    fn fetch_server(&mut self, server: Server, cx: &mut Context<Self>) {
        let client = Client::for_server(&server, &self.profile.device_id);
        let thumbs = self.profile.paths.thumbs.clone();
        let cache = self.profile.paths.library_cache(server.id);
        let fetch = Fetch {
            server,
            client,
            started: Instant::now(),
        };
        cx.spawn(async move |this, cx| {
            let (client, user_id) = (fetch.client.clone(), fetch.server.user_id.clone());
            let server = fetch.server.id;
            let fetched = cx
                .background_executor()
                .spawn(async move {
                    let items = client.library(&user_id)?;
                    if let Err(e) = jellyfin::save_library_cache(&cache, &items) {
                        eprintln!("[jellyfin] saving the library cache: {e:#}");
                    }
                    let fetched = items.iter().map(|base| Fetched {
                        item: base.to_item(server, &thumbs),
                        image: base.card_image(),
                    });
                    anyhow::Ok(fetched.collect::<Vec<_>>())
                })
                .await;
            this.update(cx, |this, cx| this.server_fetched(fetch, fetched, cx))
                .ok();
        })
        .detach();
    }

    /// Takes in what a fetch brought, or that it failed.
    fn server_fetched(
        &mut self,
        fetch: Fetch,
        fetched: Result<Vec<Fetched>>,
        cx: &mut Context<Self>,
    ) {
        let server = fetch.server.id;
        self.fetching.loading.remove(&server);
        match fetched {
            Ok(fetched) => self.show_fetched(fetch, fetched, cx),
            Err(e) => {
                let name = &fetch.server.name;
                eprintln!("[jellyfin] {name}: {e:#}");
                self.error = Some(
                    if self.catalog.has_items_of(server) {
                        format!("{name} is offline; showing the saved library")
                    } else {
                        format!("{name}: {e:#}")
                    }
                    .into(),
                );
            }
        }
        if self.fetching.refetch.remove(&server) {
            self.refresh_remote(cx);
        }
        cx.notify();
    }

    /// Puts a server's fetched items in place of the ones held, keeping
    /// what was learned about them while the fetch ran.
    fn show_fetched(&mut self, fetch: Fetch, fetched: Vec<Fetched>, cx: &mut Context<Self>) {
        let Fetch {
            server,
            client,
            started,
        } = fetch;
        // Back online: the "offline" notice goes.
        if self
            .error
            .as_ref()
            .is_some_and(|error| is_about(error, &server.name))
        {
            self.error = None;
        }
        let fresh = &mut self.play_states.fresh;
        let kept = fresh.newer_than(self.catalog.remote(), server.id, started);
        let mut wanted = Vec::new();
        let mut items = Vec::with_capacity(fetched.len());
        for Fetched { item, image } in fetched {
            if item.thumb.is_none()
                && let Some(image) = image
            {
                wanted.push((item.key(), image));
            }
            items.push(item);
        }
        let catalog = self.catalog_mut(cx);
        catalog.replace_server_items(server.id, items);
        sync::restore(&mut catalog.remote, &kept);
        self.fetch_artwork(client, wanted, cx);
    }

    /// Shows each server's last fetched library right away; the refresh
    /// that follows replaces it.
    pub(super) fn load_cached_remote(&mut self, cx: &mut Context<Self>) {
        let mut cached = Vec::new();
        for server in self.catalog.servers() {
            let path = self.profile.paths.library_cache(server.id);
            if !path.exists() {
                continue;
            }
            match jellyfin::load_library_cache(&path) {
                Ok(items) => cached.extend(
                    items
                        .iter()
                        .map(|base| base.to_item(server.id, &self.profile.paths.thumbs)),
                ),
                Err(e) => eprintln!("[jellyfin] reading {}: {e:#}", path.display()),
            }
        }
        if !cached.is_empty() {
            self.catalog_mut(cx).remote.extend(cached);
        }
    }

    /// Downloads the pictures `wanted` (item key, picture), one at a time.
    fn fetch_artwork(
        &mut self,
        client: Client,
        wanted: Vec<(String, ImageRef)>,
        cx: &mut Context<Self>,
    ) {
        // Episodes can share artwork (a show backdrop), so download each file once.
        let mut by_file: BTreeMap<PathBuf, Artwork> = BTreeMap::new();
        for (key, image) in wanted {
            let path = image.cache_path(&self.profile.paths.thumbs);
            let artwork = by_file.entry(path).or_insert_with(|| Artwork {
                image,
                keys: Vec::new(),
            });
            artwork.keys.push(key);
        }
        if by_file.is_empty() {
            return;
        }
        self.fetching.artwork_left += by_file.len();
        cx.spawn(async move |this, cx| {
            for (path, Artwork { image, keys }) in by_file {
                let target = path.clone();
                let download = client.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { download_artwork(&download, &image, &target) })
                    .await;
                let downloaded = result
                    .map_err(|e| eprintln!("[jellyfin] artwork {}: {e:#}", path.display()))
                    .is_ok();
                let alive = this.update(cx, |this, cx| {
                    this.fetching.artwork_left = this.fetching.artwork_left.saturating_sub(1);
                    if downloaded {
                        this.set_artwork(&keys, &path, cx);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Shows a downloaded picture on the items it's for.
    fn set_artwork(&mut self, keys: &[String], picture: &Path, cx: &mut Context<Self>) {
        let remote = self.catalog.remote().iter().enumerate();
        let shown_on: Vec<usize> = remote
            .filter(|(_, item)| keys.contains(&item.key()))
            .map(|(index, _)| index)
            .collect();
        if shown_on.is_empty() {
            return;
        }
        let remote = &mut self.catalog_mut(cx).remote;
        for index in shown_on {
            remote[index].thumb = Some(picture.to_path_buf());
        }
    }
}

/// Downloads a picture to `target`, making the folder for it if needed.
fn download_artwork(client: &Client, image: &ImageRef, target: &Path) -> Result<()> {
    if let Some(folder) = target.parent() {
        fs::create_dir_all(folder)?;
    }
    client.download_image(image, target)
}

/// Whether the notice in the header is about the server called `name`.
fn is_about(notice: &str, name: &str) -> bool {
    notice
        .strip_prefix(name)
        .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with(':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_about_the_server_it_starts_with() {
        assert!(is_about(
            "Attic is offline; showing the saved library",
            "Attic"
        ));
        assert!(is_about("Attic: connection refused", "Attic"));
        assert!(!is_about(
            "Attic2 is offline; showing the saved library",
            "Attic"
        ));
        assert!(!is_about("Couldn't play Attic: no such file", "Attic"));
    }
}
