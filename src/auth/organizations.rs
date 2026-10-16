//! Independent chat grants. Saving or forgetting one never modifies the
//! primary device pairing, its secrets, or its shared-folder configuration.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::client::{CHAT_SCOPE, Organization, ServerUrl, TokenResponse};
use super::key::DeviceKey;
use super::secrets::{DEVICE_KEY, REFRESH_TOKEN, SecretStore};
use super::{Credentials, lock_pairing, next_generation};
use crate::config::ServerConfig;
use crate::paths::{Paths, write_private_file};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedOrganization {
    /// Fresh public key of this sign-in, also a safe local namespace.
    pub id: String,
    pub server: ServerConfig,
    pub account: Organization,
    pub secret_store: String,
}

impl SavedOrganization {
    /// Private control and runtime directories for this one grant. The GUI
    /// keeps folder/preferences controls on the primary installation paths.
    pub fn runtime_paths(&self, primary: &Paths) -> Result<Paths> {
        self.validate(primary)?;
        Ok(Paths {
            config_dir: primary.config_dir.join("organizations").join(&self.id),
            data_dir: primary.data_dir.join("organizations").join(&self.id),
            state_dir: primary.state_dir.join("organizations").join(&self.id),
            runtime_dir: primary.runtime_dir.join("organizations").join(&self.id),
            home_trash: None,
        })
    }

    pub fn credentials(&self, paths: &Paths) -> Result<Credentials> {
        self.validate(paths)?;
        let store = self.store(paths)?;
        let stored = store
            .get(DEVICE_KEY)?
            .context("Sign in to this organization again.")?;
        let key = DeviceKey::from_stored(&stored)?;
        ensure!(
            key.public_key() == self.id,
            "This organization's saved key does not match."
        );
        Ok(Credentials {
            server: ServerUrl::parse(&self.server.url)?,
            device_id: self.server.device_id.clone(),
            key,
            store,
        })
    }

    fn store(&self, paths: &Paths) -> Result<SecretStore> {
        SecretStore::for_organization(&self.secret_store, paths, &self.id)
    }

    fn validate(&self, paths: &Paths) -> Result<()> {
        self.account.validate()?;
        self.store(paths)?;
        ensure!(
            ServerUrl::parse(&self.server.url)?.origin() == self.server.url,
            "Invalid saved server origin."
        );
        ensure!(
            !self.server.device_id.is_empty() && self.server.device_id.len() <= 128,
            "Invalid saved device identifier."
        );
        Ok(())
    }

    fn forget_secrets(&self, paths: &Paths) -> Result<()> {
        let store = self.store(paths)?;
        store.delete(REFRESH_TOKEN)?;
        store.delete(DEVICE_KEY)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    organizations: Vec<SavedOrganization>,
}

fn file(paths: &Paths) -> std::path::PathBuf {
    paths.config_dir.join("organizations.json")
}

pub fn list(paths: &Paths) -> Result<Vec<SavedOrganization>> {
    let bytes = match std::fs::read(file(paths)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let saved: Saved = serde_json::from_slice(&bytes).context("Reading saved organizations")?;
    ensure!(
        saved.version == 1,
        "Unsupported saved organizations version."
    );
    for organization in &saved.organizations {
        organization.validate(paths)?;
    }
    let unique: std::collections::HashSet<_> = saved.organizations.iter().map(|p| &p.id).collect();
    ensure!(
        unique.len() == saved.organizations.len(),
        "Duplicate organization sign-ins."
    );
    Ok(saved.organizations)
}

fn write(paths: &Paths, organizations: Vec<SavedOrganization>) -> Result<()> {
    write_private_file(
        &file(paths),
        &serde_json::to_vec_pretty(&Saved {
            version: 1,
            organizations,
        })?,
    )
}

/// Called only under the pairing lock, after its generation and cancellation
/// checks. Old servers that silently return a folder grant are refused.
pub(super) fn save_grant(
    paths: &Paths,
    server: &ServerUrl,
    key: &DeviceKey,
    token: TokenResponse,
    store_kind: &str,
) -> Result<SavedOrganization> {
    ensure!(
        token
            .scope
            .as_deref()
            .is_some_and(|s| s.split_whitespace().eq([CHAT_SCOPE])),
        "This server does not support signing in without folder sharing."
    );
    let account = token
        .account
        .context("Update the server to sign in to another organization.")?;
    let profile = SavedOrganization {
        id: key.public_key(),
        server: ServerConfig {
            url: server.origin(),
            device_id: token.device_id.context("Missing device identifier")?,
        },
        account,
        secret_store: store_kind.to_owned(),
    };
    profile.validate(paths)?;
    let refresh = token
        .refresh_token
        .filter(|r| !r.is_empty())
        .context("Missing refresh credential")?;
    let mut saved = list(paths)?;
    let replaced: Vec<_> = saved
        .iter()
        .filter(|p| p.server.url == profile.server.url && p.account.id == profile.account.id)
        .cloned()
        .collect();
    saved.retain(|p| p.server.url != profile.server.url || p.account.id != profile.account.id);
    saved.push(profile.clone());
    let store = profile.store(paths)?;
    store.set(DEVICE_KEY, &key.to_stored())?;
    if let Err(error) = store
        .set(REFRESH_TOKEN, &refresh)
        .and_then(|()| write(paths, saved))
    {
        let _ = profile.forget_secrets(paths);
        return Err(error);
    }
    for old in replaced {
        if old.id != profile.id {
            let _ = old.forget_secrets(paths);
        }
    }
    Ok(profile)
}

/// Forget a local chat grant. Any pending login is invalidated under the same
/// lock, so approving an older browser page cannot restore it afterwards.
pub fn forget(paths: &Paths, id: &str) -> Result<bool> {
    let lock = lock_pairing(paths)?;
    next_generation(paths, &lock)?;
    let mut saved = list(paths)?;
    let Some(index) = saved.iter().position(|p| p.id == id) else {
        return Ok(false);
    };
    let profile = saved.remove(index);
    // Retire the runtime even if the OS keychain refuses secret cleanup.
    write(paths, saved)?;
    profile.forget_secrets(paths)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn token(account: &str) -> TokenResponse {
        serde_json::from_value(serde_json::json!({
            "access_token": "access", "token_type": "DPoP", "expires_in": 600,
            "refresh_token": format!("refresh-{account}"), "device_id": "42",
            "scope": CHAT_SCOPE, "account": {"id": account, "name": "Demo team", "role": "member"}
        }))
        .unwrap()
    }

    #[test]
    fn organizations_keep_separate_keys_without_changing_the_folder_pairing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        let config = Config {
            server: Some(ServerConfig {
                url: "https://example.com".into(),
                device_id: "original".into(),
            }),
            ..Default::default()
        };
        config.save(&paths).unwrap();
        let primary = SecretStore::File(paths.secrets_file());
        primary.set(DEVICE_KEY, "primary").unwrap();
        primary.set(REFRESH_TOKEN, "original-refresh").unwrap();
        let origin = ServerUrl::parse("https://example.com").unwrap();
        let first = save_grant(
            &paths,
            &origin,
            &DeviceKey::generate().unwrap(),
            token("123456789"),
            "file",
        )
        .unwrap();
        let second = save_grant(
            &paths,
            &origin,
            &DeviceKey::generate().unwrap(),
            token("987654321"),
            "file",
        )
        .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(Config::load(&paths).unwrap(), config);
        assert_eq!(primary.get(DEVICE_KEY).unwrap().as_deref(), Some("primary"));
        assert_eq!(
            primary.get(REFRESH_TOKEN).unwrap().as_deref(),
            Some("original-refresh")
        );
        assert_eq!(list(&paths).unwrap(), [first.clone(), second.clone()]);
        assert_eq!(
            first
                .credentials(&paths)
                .unwrap()
                .store
                .get(REFRESH_TOKEN)
                .unwrap()
                .as_deref(),
            Some("refresh-123456789")
        );
        let json = std::fs::read_to_string(file(&paths)).unwrap();
        assert!(!json.contains("refresh-") && !json.contains("access_token"));
        assert!(forget(&paths, &first.id).unwrap());
        assert!(first.credentials(&paths).is_err());
        assert!(second.credentials(&paths).is_ok());
        assert_eq!(list(&paths).unwrap(), [second]);
        assert!(!forget(&paths, "missing").unwrap());
        assert_eq!(Config::load(&paths).unwrap(), config);
    }

    #[test]
    fn forgetting_retires_the_runtime_even_if_secret_cleanup_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        let origin = ServerUrl::parse("https://example.com").unwrap();
        let profile = save_grant(
            &paths,
            &origin,
            &DeviceKey::generate().unwrap(),
            token("123456789"),
            "file",
        )
        .unwrap();
        let SecretStore::File(secret) = profile.store(&paths).unwrap() else {
            panic!("file store");
        };
        std::fs::remove_file(&secret).unwrap();
        std::fs::create_dir(&secret).unwrap();
        assert!(forget(&paths, &profile.id).is_err());
        assert!(list(&paths).unwrap().is_empty());
    }

    #[test]
    fn old_server_grants_invalid_accounts_and_mixed_keys_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::under(tmp.path());
        let origin = ServerUrl::parse("https://example.com").unwrap();
        let key = DeviceKey::generate().unwrap();
        let mut old = token("123456789");
        old.scope = Some("local_agent:serve local_agent:chat".into());
        assert!(save_grant(&paths, &origin, &key, old, "file").is_err());
        let mut invalid = token("../escape");
        assert!(save_grant(&paths, &origin, &key, invalid.clone(), "file").is_err());
        invalid.account = None;
        assert!(save_grant(&paths, &origin, &key, invalid, "file").is_err());
        assert!(list(&paths).unwrap().is_empty());
        let profile = save_grant(&paths, &origin, &key, token("123456789"), "file").unwrap();
        profile
            .store(&paths)
            .unwrap()
            .set(DEVICE_KEY, &DeviceKey::generate().unwrap().to_stored())
            .unwrap();
        assert!(profile.credentials(&paths).is_err());
        assert!(SecretStore::for_organization("file", &paths, "../escape").is_err());
        std::fs::write(file(&paths), b"{broken").unwrap();
        assert!(list(&paths).is_err());
    }
}
