//! What is known about each server's Sonarr and Radarr, and the rules that
//! need no database or credential store: which apps a set of servers has,
//! where their addresses and keys are kept, what a form may save.

use std::collections::HashMap;
use std::fmt;

use gpui::SharedString;

use crate::arr::{self, Client, Kind};
use crate::jellyfin::Server;

/// One app of one server (by the server's row id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id {
    pub server: i64,
    pub kind: Kind,
}

/// A connection that was checked and saved: the app's address and API key.
#[derive(Clone, PartialEq, Eq)]
pub struct Saved {
    pub url: String,
    pub(super) key: String,
}

impl fmt::Debug for Saved {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Saved")
            .field("url", &self.url)
            .field("key", &"(hidden)")
            .finish()
    }
}

/// How the last check of a connection went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not checked since it was read or removed.
    Idle,
    Testing,
    /// The app answered; `version` is its name and version ("Sonarr 4.0.15").
    Connected {
        version: String,
    },
    /// What went wrong, as a sentence to show.
    Failed {
        problem: SharedString,
    },
}

/// One app of a signed-in server: set up or not, and how its check went.
#[derive(Debug, Clone)]
pub struct Connection {
    pub saved: Option<Saved>,
    pub status: Status,
}

/// A set-up app and a client to ask it with.
pub struct ConnectedApp {
    pub id: Id,
    pub client: Client,
}

/// Every signed-in server's Sonarr and Radarr, set up or not.
#[derive(Debug, Clone, Default)]
pub struct Apps {
    connections: HashMap<Id, Connection>,
}

impl Apps {
    pub fn connection(&self, id: Id) -> Option<&Connection> {
        self.connections.get(&id)
    }

    /// Every app of every signed-in server, in no order.
    pub fn ids(&self) -> impl Iterator<Item = Id> + '_ {
        self.connections.keys().copied()
    }

    /// A client for a server's set-up app.
    pub fn client(&self, id: Id) -> Option<Client> {
        let saved = self.connections.get(&id)?.saved.as_ref()?;
        Some(Client::new(id.kind, &saved.url, &saved.key))
    }

    /// The set-up apps, by server, Sonarr before Radarr.
    pub fn connected_ids(&self) -> Vec<Id> {
        let mut ids: Vec<Id> = self
            .connections
            .iter()
            .filter(|(_, connection)| connection.saved.is_some())
            .map(|(id, _)| *id)
            .collect();
        ids.sort_by_key(|id| (id.server, id.kind == Kind::Radarr));
        ids
    }

    /// Every set-up app with its client, in the order of [`Self::connected_ids`].
    pub fn connected(&self) -> Vec<ConnectedApp> {
        self.connected_ids()
            .into_iter()
            .filter_map(|id| {
                let client = self.client(id)?;
                Some(ConnectedApp { id, client })
            })
            .collect()
    }

    /// Drops the apps of servers that aren't in `servers` any more.
    pub(super) fn keep_servers(&mut self, servers: &[Server]) {
        self.connections
            .retain(|id, _| servers.iter().any(|server| server.id == id.server));
    }

    /// The apps of `servers` not known yet, in the servers' order, Sonarr first.
    pub(super) fn unknown(&self, servers: &[Server]) -> Vec<Id> {
        servers
            .iter()
            .flat_map(|server| {
                Kind::ALL.map(|kind| Id {
                    server: server.id,
                    kind,
                })
            })
            .filter(|id| !self.connections.contains_key(id))
            .collect()
    }

    pub(super) fn insert(&mut self, id: Id, connection: Connection) {
        self.connections.insert(id, connection);
    }

    pub(super) fn remove(&mut self, id: Id) {
        self.connections.remove(&id);
    }

    pub(super) fn connection_mut(&mut self, id: Id) -> Option<&mut Connection> {
        self.connections.get_mut(&id)
    }
}

/// Ties a setting or key to the server and user, as Jellyfin tokens are.
fn scope(server: &Server) -> String {
    format!("{}:{}", server.server_id, server.user_id)
}

/// The library setting holding the address of a server's app.
pub(super) fn url_setting(kind: Kind, server: &Server) -> String {
    format!("{}:{}", kind.url_setting(), scope(server))
}

/// The credential store entry holding the API key of a server's app. It has
/// the device id in it, so a test library never touches the real one's key.
pub(super) fn key_account(kind: Kind, device_id: &str, server: &Server) -> String {
    format!("{}:{device_id}:{}", kind.key_account(), scope(server))
}

/// Where the key was kept when the whole app had one Sonarr and one Radarr.
pub(super) fn single_key_account(kind: Kind, device_id: &str) -> String {
    format!("{}:{device_id}", kind.key_account())
}

/// Error text as shown on its own: starting with a capital.
pub(super) fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// What a check of the app's status says about the connection.
pub(super) fn status_after(answer: &anyhow::Result<arr::Status>) -> Status {
    match answer {
        Ok(status) => Status::Connected {
            version: format!("{} {}", status.app_name, status.version),
        },
        Err(e) => Status::Failed {
            problem: sentence(&format!("{e:#}")).into(),
        },
    }
}

/// What "Save and Test" checks and saves, or what the form still needs.
/// An empty key field keeps the key already saved.
pub(super) fn entry(
    kind: Kind,
    address: &str,
    typed_key: &str,
    saved: Option<&Saved>,
) -> Result<Saved, String> {
    let typed_key = typed_key.trim();
    let key = if typed_key.is_empty() {
        saved.map(|saved| saved.key.clone())
    } else {
        Some(typed_key.to_string())
    };
    if address.trim().is_empty() {
        return Err(format!("Enter {}'s address.", kind.name()));
    }
    let Some(key) = key else {
        return Err(format!(
            "Enter the API key from {} → Settings → General.",
            kind.name()
        ));
    };
    Ok(Saved {
        url: arr::normalize_url(address),
        key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: i64) -> Server {
        Server {
            id,
            url: "http://moto-moto-srv.local:8096".into(),
            name: "moto".into(),
            server_id: "srv".into(),
            user_id: "usr".into(),
            user_name: "me".into(),
            token: String::new(),
        }
    }

    fn saved(url: &str) -> Saved {
        Saved {
            url: url.into(),
            key: "secret-key".into(),
        }
    }

    fn id(server: i64, kind: Kind) -> Id {
        Id { server, kind }
    }

    fn apps(connections: &[(Id, Option<Saved>)]) -> Apps {
        let mut apps = Apps::default();
        for (id, saved) in connections {
            let connection = Connection {
                saved: saved.clone(),
                status: Status::Idle,
            };
            apps.insert(*id, connection);
        }
        apps
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(sentence("that's Sonarr"), "That's Sonarr");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn keys_belong_to_a_server_and_user() {
        let server = server(1);
        assert_eq!(url_setting(Kind::Sonarr, &server), "sonarr_url:srv:usr");
        assert_eq!(
            key_account(Kind::Radarr, "dev", &server),
            "radarr:dev:srv:usr"
        );
        assert_eq!(single_key_account(Kind::Radarr, "dev"), "radarr:dev");
    }

    #[test]
    fn each_signed_in_server_gets_both_apps_once() {
        let servers = [server(1), server(2)];
        let mut known = apps(&[(id(1, Kind::Sonarr), None), (id(7, Kind::Radarr), None)]);

        known.keep_servers(&servers);

        assert!(known.connection(id(7, Kind::Radarr)).is_none());
        assert_eq!(
            known.unknown(&servers),
            [
                id(1, Kind::Radarr),
                id(2, Kind::Sonarr),
                id(2, Kind::Radarr)
            ]
        );
    }

    #[test]
    fn only_set_up_apps_are_connected_sonarr_first() {
        let known = apps(&[
            (id(2, Kind::Radarr), Some(saved("http://two:7878"))),
            (id(2, Kind::Sonarr), Some(saved("http://two:8989"))),
            (id(1, Kind::Radarr), Some(saved("http://one:7878"))),
            (id(1, Kind::Sonarr), None),
        ]);
        assert_eq!(
            known.connected_ids(),
            [
                id(1, Kind::Radarr),
                id(2, Kind::Sonarr),
                id(2, Kind::Radarr)
            ]
        );
        assert!(known.client(id(1, Kind::Sonarr)).is_none());
        assert!(known.client(id(1, Kind::Radarr)).is_some());
        assert_eq!(known.connected().len(), 3);
    }

    #[test]
    fn a_form_needs_an_address_and_a_key() {
        assert_eq!(
            entry(Kind::Sonarr, "  ", "key", None),
            Err("Enter Sonarr's address.".to_string())
        );
        assert_eq!(
            entry(Kind::Radarr, "radarr.local:7878/", " ", None),
            Err("Enter the API key from Radarr → Settings → General.".to_string())
        );
        assert_eq!(
            entry(Kind::Radarr, "radarr.local:7878/", " typed ", None),
            Ok(Saved {
                url: "http://radarr.local:7878".into(),
                key: "typed".into()
            })
        );
    }

    #[test]
    fn an_empty_key_field_keeps_the_saved_key() {
        let before = saved("http://old:8989");
        assert_eq!(
            entry(Kind::Sonarr, "https://new/sonarr", "", Some(&before)),
            Ok(Saved {
                url: "https://new/sonarr".into(),
                key: "secret-key".into()
            })
        );
    }

    #[test]
    fn a_check_says_what_answered_or_what_went_wrong() {
        let answered = Ok(arr::Status {
            app_name: "Sonarr".into(),
            version: "4.0.15".into(),
        });
        assert_eq!(
            status_after(&answered),
            Status::Connected {
                version: "Sonarr 4.0.15".into()
            }
        );
        let failed = Err(anyhow::anyhow!("couldn't reach http://one:8989"));
        assert_eq!(
            status_after(&failed),
            Status::Failed {
                problem: "Couldn't reach http://one:8989".into()
            }
        );
    }

    #[test]
    fn a_saved_connection_never_prints_its_key() {
        let printed = format!("{:?}", saved("http://one:8989"));
        assert!(printed.contains("http://one:8989"));
        assert!(!printed.contains("secret-key"));
    }
}
