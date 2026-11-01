//! Where the device key and refresh token live.
//!
//! The OS keychain (macOS Keychain, Secret Service on Linux) is preferred.
//! When it is unavailable, as on headless servers, secrets go to a 0600 JSON
//! file in the config directory instead. `CWW_SECRET_STORE=file` forces the
//! file store.
//!
//! The keychain is one per user, whatever the paths, so its entries name the
//! server they're for: signing in to a second server, a development one say,
//! never touches another server's key. A pairing saved before they did keeps
//! working: its entries are copied on first use. Nothing says which server
//! those were for, so they stay until a purge.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};

use crate::config::Config;
use crate::paths::{Paths, write_private_file};

#[cfg_attr(test, allow(dead_code))]
const KEYRING_SERVICE: &str = "com.chatwithwork.cww";

pub const DEVICE_KEY: &str = "device-key";
pub const REFRESH_TOKEN: &str = "refresh-token";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretStore {
    /// The keychain, under names of its own: only for `cww debug keychain`,
    /// and for reading what older versions saved.
    Keyring,
    /// The keychain, for one server, by its origin.
    ServerKeyring(String),
    OrganizationKeyring(String),
    File(PathBuf),
}

impl SecretStore {
    /// The store the config recorded at pairing, whatever the environment
    /// says now: where a purge finds what pairing wrote.
    pub fn recorded(config: &Config, paths: &Paths) -> Self {
        match config.secret_store.as_deref() {
            Some("file") => Self::File(paths.secrets_file()),
            _ => Self::Keyring.for_paired(config),
        }
    }

    /// The store recorded in the config, unless the environment overrides it.
    pub fn for_config(config: &Config, paths: &Paths) -> Self {
        let choice = std::env::var("CWW_SECRET_STORE")
            .ok()
            .or_else(|| config.secret_store.clone());
        match choice.as_deref() {
            Some("file") => Self::File(paths.secrets_file()),
            _ => Self::Keyring.for_paired(config),
        }
    }

    fn for_paired(self, config: &Config) -> Self {
        match &config.server {
            Some(server) => self.for_server(&server.url),
            None => self,
        }
    }

    /// This store, for the server at `origin`.
    pub fn for_server(self, origin: &str) -> Self {
        match self {
            Self::Keyring => Self::ServerKeyring(origin.trim_end_matches('/').to_string()),
            store => store,
        }
    }

    /// Whether this lives in the system's keychain, which the sandbox has
    /// to let the daemon reach.
    pub fn in_keychain(&self) -> bool {
        !matches!(self, Self::File(_))
    }

    /// Pick a store for a new pairing: the keychain if a round trip works,
    /// otherwise the file.
    pub fn choose(paths: &Paths) -> Self {
        if std::env::var("CWW_SECRET_STORE").as_deref() == Ok("file") {
            return Self::File(paths.secrets_file());
        }
        let probe = "probe";
        let works = Self::Keyring.set(probe, "ok").is_ok()
            && Self::Keyring.get(probe).ok().flatten().as_deref() == Some("ok");
        let _ = Self::Keyring.delete(probe);
        if works {
            Self::Keyring
        } else {
            Self::File(paths.secrets_file())
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Keyring | Self::ServerKeyring(_) | Self::OrganizationKeyring(_) => "keyring",
            Self::File(_) => "file",
        }
    }

    /// Each organization owns separate keychain entries or a separate file,
    /// so refreshing one grant cannot overwrite another organization's key.
    pub fn for_organization(kind: &str, paths: &Paths, id: &str) -> Result<Self> {
        anyhow::ensure!(
            id.len() == 43
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')),
            "Invalid organization sign-in identifier."
        );
        match kind {
            "keyring" => Ok(Self::OrganizationKeyring(id.to_owned())),
            "file" => Ok(Self::File(
                paths
                    .config_dir
                    .join("organizations")
                    .join(id)
                    .join("secrets.json"),
            )),
            _ => anyhow::bail!("Unknown organization secret store."),
        }
    }

    pub fn get(&self, name: &str) -> Result<Option<String>> {
        match self {
            Self::ServerKeyring(origin) => {
                let scoped = server_entry(origin, name);
                carried_over(
                    Self::Keyring.get(&scoped)?,
                    || Self::Keyring.get(name),
                    |value| Self::Keyring.set(&scoped, value),
                )
            }
            Self::OrganizationKeyring(id) => {
                Self::Keyring.get(&format!("organization:{id}:{name}"))
            }
            Self::Keyring => match entry(name)?.get_password() {
                Ok(v) => Ok(Some(v)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(anyhow!("reading {name} from the keychain: {e}")),
            },
            Self::File(path) => Ok(read_file(path)?.remove(name)),
        }
    }

    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        match self {
            Self::ServerKeyring(origin) => Self::Keyring.set(&server_entry(origin, name), value),
            Self::OrganizationKeyring(id) => {
                Self::Keyring.set(&format!("organization:{id}:{name}"), value)
            }
            Self::Keyring => entry(name)?
                .set_password(value)
                .map_err(|e| anyhow!("writing {name} to the keychain: {e}")),
            Self::File(path) => {
                let mut map = read_file(path)?;
                map.insert(name.to_string(), value.to_string());
                write_private_file(path, &serde_json::to_vec_pretty(&map)?)
            }
        }
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        match self {
            Self::ServerKeyring(origin) => Self::Keyring.delete(&server_entry(origin, name)),
            Self::OrganizationKeyring(id) => {
                Self::Keyring.delete(&format!("organization:{id}:{name}"))
            }
            Self::Keyring => match entry(name)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(anyhow!("deleting {name} from the keychain: {e}")),
            },
            Self::File(path) => {
                let mut map = read_file(path)?;
                if map.remove(name).is_some() {
                    write_private_file(path, &serde_json::to_vec_pretty(&map)?)?;
                }
                Ok(())
            }
        }
    }
}

fn server_entry(origin: &str, name: &str) -> String {
    format!("server:{origin}:{name}")
}

/// What a server's entry holds, or what an older version saved for this
/// computer, kept under the server's name from then on. The older entry
/// stays: another server's pairing may be the one reading it first.
fn carried_over(
    own: Option<String>,
    older: impl FnOnce() -> Result<Option<String>>,
    keep: impl FnOnce(&str) -> Result<()>,
) -> Result<Option<String>> {
    if own.is_some() {
        return Ok(own);
    }
    let older = older()?;
    if let Some(value) = &older {
        keep(value)?;
    }
    Ok(older)
}

fn entry(name: &str) -> Result<keyring::Entry> {
    // The keychain's entries are the person's, one set per user, whatever
    // the paths: a test that reached them would read or delete the real
    // pairing of whoever runs it. Tests use the file store.
    #[cfg(test)]
    {
        let _ = name;
        anyhow::bail!("tests never touch the system's keychain; use the file store");
    }
    #[cfg(not(test))]
    keyring::Entry::new(KEYRING_SERVICE, name).map_err(|e| anyhow!("opening the keychain: {e}"))
}

fn read_file(path: &PathBuf) -> Result<BTreeMap<String, String>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_server_has_entries_of_its_own() {
        let production = SecretStore::Keyring.for_server("https://chatwithwork.com/");
        let development = SecretStore::Keyring.for_server("http://localhost:3000");
        assert_ne!(production, development);
        assert_eq!(
            production,
            SecretStore::ServerKeyring("https://chatwithwork.com".into())
        );
        assert_eq!(
            server_entry("https://chatwithwork.com", DEVICE_KEY),
            "server:https://chatwithwork.com:device-key"
        );
        assert_ne!(
            server_entry("http://localhost:3000", REFRESH_TOKEN),
            server_entry("https://chatwithwork.com", REFRESH_TOKEN)
        );
    }

    #[test]
    fn a_file_or_an_organization_stays_as_it_is_for_a_server() {
        let file = SecretStore::File("/x".into());
        assert_eq!(file.clone().for_server("https://a.example"), file);
        let organization = SecretStore::OrganizationKeyring("id".into());
        assert_eq!(
            organization.clone().for_server("https://a.example"),
            organization
        );
    }

    #[test]
    fn a_pairing_saved_by_an_older_version_is_carried_over_once() {
        let kept = std::cell::RefCell::new(None);
        let keep = |value: &str| {
            *kept.borrow_mut() = Some(value.to_string());
            Ok(())
        };
        let value = carried_over(None, || Ok(Some("older".into())), keep).unwrap();
        assert_eq!(value.as_deref(), Some("older"));
        assert_eq!(kept.borrow().as_deref(), Some("older"));

        let value = carried_over(
            Some("own".into()),
            || panic!("the older entry was read"),
            |_| panic!("the older entry was copied"),
        )
        .unwrap();
        assert_eq!(value.as_deref(), Some("own"));

        let value = carried_over(None, || Ok(None), |_| panic!("nothing to keep")).unwrap();
        assert_eq!(value, None);
    }

    #[test]
    fn file_store_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SecretStore::File(tmp.path().join("secrets.json"));
        assert_eq!(store.get("a").unwrap(), None);
        store.set("a", "1").unwrap();
        store.set("b", "2").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("1"));
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
        assert_eq!(store.get("b").unwrap().as_deref(), Some("2"));
    }
}
