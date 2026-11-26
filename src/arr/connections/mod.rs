//! Sonarr and Radarr, set up per Jellyfin server: they fill that server's
//! libraries, so its shows and movies can be requested through them.
//! Addresses live in the library settings, API keys in the credential
//! store, and both go when the server is signed out of. `Connections` keeps
//! them in step with the signed-in servers; pages read its `Apps`.

use std::rc::Rc;

use anyhow::Result;
use gpui::{App, Context, Entity, EventEmitter};

use crate::arr::{self, Client, Kind};
use crate::jellyfin::Server;
use crate::library::{self, Library, Profile};
use crate::secrets;

mod apps;

use apps::Saved;
pub use apps::{Apps, ConnectedApp, Connection, Id, Status};

/// What the connections tell whoever subscribes to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The apps were matched to the signed-in servers again.
    Synced,
    /// A connection was checked and saved: `id` is set up now.
    Saved { id: Id },
}

/// The signed-in servers' Sonarr and Radarr connections.
pub struct Connections {
    profile: Profile,
    library: Entity<Library>,
    /// What is known now; copied when it changes, as pages hold on to it.
    apps: Rc<Apps>,
}

impl EventEmitter<Event> for Connections {}

impl Connections {
    /// Follows `library`'s servers from now on.
    pub fn new(profile: Profile, library: Entity<Library>, cx: &mut Context<Self>) -> Self {
        cx.subscribe(&library, |this, _, event, cx| match event {
            library::Event::ServersChanged => this.sync(cx),
        })
        .detach();
        Self {
            profile,
            library,
            apps: Rc::default(),
        }
    }

    /// Every signed-in server's apps, as of now.
    pub fn apps(&self) -> Rc<Apps> {
        self.apps.clone()
    }

    /// The address a form for `id` starts with: the saved one, or a guess
    /// from the server's own.
    pub fn address(&self, id: Id, cx: &App) -> Option<String> {
        let catalog = self.library.read(cx).catalog();
        let server = catalog.server(id.server)?;
        self.saved_address(id.kind, server)
            .or_else(|| arr::guess_url(id.kind, &server.url))
    }

    /// Forgets a server's apps, for signing out of it.
    pub fn forget(&mut self, server: &Server, cx: &mut Context<Self>) {
        for kind in Kind::ALL {
            if let Err(e) = self.delete(kind, server) {
                eprintln!("[{}] removing: {e:#}", kind.name());
            }
            let id = Id {
                server: server.id,
                kind,
            };
            Rc::make_mut(&mut self.apps).remove(id);
        }
        cx.notify();
    }

    /// "Save and Test": checks the form's address and key, and saves them
    /// once they work. An empty key keeps the saved one.
    pub fn save(&mut self, id: Id, address: &str, typed_key: &str, cx: &mut Context<Self>) {
        let Some(connection) = self.apps.connection(id) else {
            return;
        };
        match apps::entry(id.kind, address, typed_key, connection.saved.as_ref()) {
            Ok(entry) => self.check(id, Some(entry), cx),
            Err(problem) => {
                let problem = problem.into();
                self.set_status(id, Status::Failed { problem }, cx);
            }
        }
    }

    /// Takes a server's app out of use: its address and key are deleted.
    pub fn remove(&mut self, id: Id, cx: &mut Context<Self>) {
        let catalog = self.library.read(cx).catalog();
        let Some(server) = catalog.server(id.server) else {
            return;
        };
        let removed = self.delete(id.kind, server);
        if let Some(connection) = Rc::make_mut(&mut self.apps).connection_mut(id) {
            connection.saved = None;
            connection.status = match removed {
                Ok(()) => Status::Idle,
                Err(e) => Status::Failed {
                    problem: format!("Couldn't remove it: {e:#}").into(),
                },
            };
        }
        cx.notify();
    }

    /// Stops showing what went wrong with `id`, when its form is closed.
    pub fn clear_problem(&mut self, id: Id, cx: &mut Context<Self>) {
        let failed = self
            .apps
            .connection(id)
            .is_some_and(|connection| matches!(connection.status, Status::Failed { .. }));
        if failed {
            self.set_status(id, Status::Idle, cx);
        }
    }

    /// Reads what was saved for each app of each signed-in server (checking
    /// it in the background) and drops the apps of servers signed out of.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let catalog = self.library.read(cx).catalog();
        let servers = catalog.servers();
        Rc::make_mut(&mut self.apps).keep_servers(servers);
        self.move_single_connections(servers);
        for id in self.apps.unknown(servers) {
            let Some(server) = catalog.server(id.server) else {
                continue;
            };
            let saved = self.load(id.kind, server);
            let is_set_up = saved.is_some();
            let connection = Connection {
                saved,
                status: Status::Idle,
            };
            Rc::make_mut(&mut self.apps).insert(id, connection);
            if is_set_up {
                self.check(id, None, cx);
            }
        }
        cx.emit(Event::Synced);
        cx.notify();
    }

    /// With a single server, the one Sonarr and one Radarr an older build
    /// kept for the whole app become that server's.
    fn move_single_connections(&self, servers: &[Server]) {
        let [server] = servers else {
            return;
        };
        for kind in Kind::ALL {
            let Ok(Some(url)) = self.profile.db.setting(kind.url_setting()) else {
                continue;
            };
            if let Err(e) = self.move_single_connection(kind, &url, server) {
                eprintln!("[{}] moving the old connection: {e:#}", kind.name());
            }
        }
    }

    fn move_single_connection(&self, kind: Kind, url: &str, server: &Server) -> Result<()> {
        let device_id = &self.profile.device_id;
        let old_account = apps::single_key_account(kind, device_id);
        if let Some(key) = secrets::load(&old_account)?
            && !url.is_empty()
        {
            secrets::save(&apps::key_account(kind, device_id, server), &key)?;
            self.profile
                .db
                .set_setting(&apps::url_setting(kind, server), url)?;
        }
        secrets::delete(&old_account)?;
        self.profile.db.delete_setting(kind.url_setting())
    }

    /// Checks an address and key (`save`, or the saved ones) and saves
    /// `save` once it works.
    fn check(&mut self, id: Id, save: Option<Saved>, cx: &mut Context<Self>) {
        let Some(connection) = self.apps.connection(id) else {
            return;
        };
        let Some(entry) = save.clone().or_else(|| connection.saved.clone()) else {
            return;
        };
        self.set_status(id, Status::Testing, cx);
        let client = Client::new(id.kind, &entry.url, &entry.key);
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async move { client.status() })
                .await;
            this.update(cx, |this, cx| this.checked(id, &answer, save, cx))
                .ok();
        })
        .detach();
    }

    /// The app answered the check (or didn't): shows it, and saves `save`
    /// if it did.
    fn checked(
        &mut self,
        id: Id,
        answer: &Result<arr::Status>,
        save: Option<Saved>,
        cx: &mut Context<Self>,
    ) {
        let catalog = self.library.read(cx).catalog();
        let Some(server) = catalog.server(id.server) else {
            return;
        };
        let mut status = apps::status_after(answer);
        let mut saved = None;
        if let (Ok(_), Some(entry)) = (answer, save) {
            match self.store(id.kind, server, &entry) {
                Ok(()) => saved = Some(entry),
                Err(e) => {
                    status = Status::Failed {
                        problem: format!("Couldn't save the connection: {e:#}").into(),
                    }
                }
            }
        }
        let Some(connection) = Rc::make_mut(&mut self.apps).connection_mut(id) else {
            return;
        };
        connection.status = status;
        if let Some(saved) = saved {
            connection.saved = Some(saved);
            cx.emit(Event::Saved { id });
        }
        cx.notify();
    }

    fn set_status(&mut self, id: Id, status: Status, cx: &mut Context<Self>) {
        if let Some(connection) = Rc::make_mut(&mut self.apps).connection_mut(id) {
            connection.status = status;
            cx.notify();
        }
    }

    /// The address saved for a server's app, if any.
    fn saved_address(&self, kind: Kind, server: &Server) -> Option<String> {
        self.profile
            .db
            .setting(&apps::url_setting(kind, server))
            .ok()
            .flatten()
            .filter(|url| !url.is_empty())
    }

    /// What was saved for a server's app: both its address and its key.
    fn load(&self, kind: Kind, server: &Server) -> Option<Saved> {
        let url = self.saved_address(kind, server);
        let account = apps::key_account(kind, &self.profile.device_id, server);
        let key = secrets::load(&account).unwrap_or_else(|e| {
            eprintln!("[{}] reading the API key: {e:#}", kind.name());
            None
        });
        url.zip(key).map(|(url, key)| Saved { url, key })
    }

    fn store(&self, kind: Kind, server: &Server, entry: &Saved) -> Result<()> {
        let account = apps::key_account(kind, &self.profile.device_id, server);
        secrets::save(&account, &entry.key)?;
        self.profile
            .db
            .set_setting(&apps::url_setting(kind, server), &entry.url)
    }

    fn delete(&self, kind: Kind, server: &Server) -> Result<()> {
        let account = apps::key_account(kind, &self.profile.device_id, server);
        secrets::delete(&account)?;
        self.profile
            .db
            .delete_setting(&apps::url_setting(kind, server))
    }
}
