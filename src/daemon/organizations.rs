//! Independent chat runtimes in the unsandboxed service parent. Each has its
//! own control socket, tokens, subscriptions and notification endpoint. Only
//! the primary confined worker owns filesystem tools and shared folders.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::auth::organizations::{SavedOrganization, list};
use crate::paths::Paths;

struct Running {
    profile: SavedOrganization,
    proxy: Option<String>,
    stop: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

pub(super) fn spawn(
    primary: Paths,
    helper: Option<PathBuf>,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut running: BTreeMap<String, Running> = BTreeMap::new();
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut invalid = false;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = tick.tick() => {}
            }
            let paths = primary.clone();
            let profiles = tokio::task::spawn_blocking(move || {
                Ok::<_, anyhow::Error>((list(&paths)?, crate::config::Config::load(&paths)?.proxy))
            })
            .await;
            // A removed or unreadable grant must never leave an old runtime
            // accepting commands indefinitely. Retry after the next tick.
            let (desired, proxy): (BTreeMap<_, _>, _) = match profiles {
                Ok(Ok((profiles, proxy))) => {
                    invalid = false;
                    (
                        profiles.into_iter().map(|p| (p.id.clone(), p)).collect(),
                        proxy,
                    )
                }
                _ => {
                    if !invalid {
                        tracing::warn!("saved organization sign-ins could not be loaded");
                    }
                    invalid = true;
                    (BTreeMap::new(), None)
                }
            };
            let retired: Vec<_> = running
                .iter()
                .filter(|(id, entry)| {
                    desired.get(*id) != Some(&entry.profile)
                        || proxy != entry.proxy
                        || entry.task.is_finished()
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in retired {
                if let Some(entry) = running.remove(&id) {
                    stop(entry).await;
                }
            }
            for (id, profile) in desired {
                if running.contains_key(&id) {
                    continue;
                }
                let stop = shutdown.child_token();
                let context = super::OrganizationContext {
                    primary: primary.clone(),
                    profile: profile.clone(),
                    proxy: proxy.clone(),
                };
                let helper = helper.clone();
                let task_stop = stop.clone();
                let task = tokio::spawn(async move {
                    let result = async {
                        let paths = context.profile.runtime_paths(&context.primary)?;
                        let notifications =
                            crate::notification_delivery::spawn(helper, task_stop.child_token());
                        super::run_with_notifications(
                            paths,
                            task_stop.clone(),
                            false,
                            notifications,
                            Some(context),
                            crate::commands::Runner::Off(
                                "an organization's chats run no commands".into(),
                            ),
                        )
                        .await
                    }
                    .await;
                    task_stop.cancel();
                    result
                });
                running.insert(
                    id,
                    Running {
                        profile,
                        proxy: proxy.clone(),
                        stop,
                        task,
                    },
                );
            }
        }
        for (_, entry) in running {
            stop(entry).await;
        }
    })
}

async fn stop(mut entry: Running) {
    entry.stop.cancel();
    match tokio::time::timeout(Duration::from_secs(5), &mut entry.task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(_) => tracing::warn!("an organization chat connection stopped"),
        Err(_) => {
            entry.task.abort();
            let _ = entry.task.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::client::Organization;
    use crate::auth::key::DeviceKey;
    use crate::auth::secrets::{DEVICE_KEY, REFRESH_TOKEN, SecretStore};
    use crate::config::ServerConfig;
    use crate::control::{self, ControlRequest};
    use crate::paths::write_private_file;
    use serde_json::json;

    async fn is_running(paths: &Paths) -> bool {
        let socket = paths.socket_path();
        tokio::task::spawn_blocking(move || {
            control::request(&socket, ControlRequest::Hello)
                .ok()
                .flatten()
                .is_some()
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn saved_grants_run_without_a_ui_and_forgetting_one_closes_its_socket() {
        let temporary = tempfile::tempdir().unwrap();
        let primary = Paths::under(temporary.path());
        let key = DeviceKey::generate().unwrap();
        let profile = SavedOrganization {
            id: key.public_key(),
            server: ServerConfig {
                url: "http://127.0.0.1:9".into(),
                device_id: "42".into(),
            },
            account: Organization {
                id: "123456789".into(),
                name: "Example".into(),
                role: "member".into(),
            },
            secret_store: "file".into(),
        };
        let store = SecretStore::for_organization("file", &primary, &profile.id).unwrap();
        store.set(DEVICE_KEY, &key.to_stored()).unwrap();
        store.set(REFRESH_TOKEN, "synthetic-refresh").unwrap();
        write_private_file(
            &primary.config_dir.join("organizations.json"),
            &serde_json::to_vec(&json!({ "version": 1, "organizations": [profile] })).unwrap(),
        )
        .unwrap();
        let paths = profile.runtime_paths(&primary).unwrap();
        let shutdown = CancellationToken::new();
        let task = spawn(primary.clone(), None, shutdown.clone());
        tokio::time::timeout(Duration::from_secs(5), async {
            while !is_running(&paths).await {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        assert!(!primary.config_file().exists());
        assert!(!primary.secrets_file().exists());
        assert!(crate::auth::organizations::forget(&primary, &profile.id).unwrap());
        tokio::time::timeout(Duration::from_secs(5), async {
            while is_running(&paths).await {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }
}
