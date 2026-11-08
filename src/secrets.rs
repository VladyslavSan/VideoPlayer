//! Secrets live in the operating system's credential store (Windows
//! Credential Manager, macOS Keychain, Secret Service on Linux), which
//! encrypts them for the signed-in user. The library database never holds them.

use anyhow::{Context as _, Result};
use keyring::{Entry, Error};

const SERVICE: &str = "VideoPlayer";

fn entry(account: &str) -> Result<Entry> {
    Entry::new(SERVICE, account).context("opening the system credential store")
}

pub fn save(account: &str, secret: &str) -> Result<()> {
    entry(account)?
        .set_password(secret)
        .context("saving to the system credential store")
}

/// `None` if nothing is stored under `account`.
pub fn load(account: &str) -> Result<Option<String>> {
    match entry(account)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("reading the system credential store"),
    }
}

/// Removing something that isn't there is not an error.
pub fn delete(account: &str) -> Result<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("deleting from the system credential store"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the real credential store, under a throwaway account name.
    #[test]
    #[cfg_attr(not(windows), ignore)]
    fn round_trip() -> Result<()> {
        let account = format!("test:{}", std::process::id());
        assert_eq!(load(&account)?, None);
        save(&account, "first")?;
        save(&account, "second")?;
        assert_eq!(load(&account)?.as_deref(), Some("second"));
        delete(&account)?;
        assert_eq!(load(&account)?, None);
        delete(&account)?;
        Ok(())
    }
}
