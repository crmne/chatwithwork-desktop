//! A selected chat sign-in is independent from the primary folder daemon.
use std::sync::{Arc, Condvar, Mutex, mpsc::Sender};
use std::time::Duration;

use super::app::{DaemonStatus, Msg};
use crate::auth::organizations::{SavedOrganization, list};
use crate::control::{BoundedClient, ControlRequest};
use crate::paths::Paths;

pub(super) struct Watcher(Arc<(Mutex<bool>, Condvar)>);

impl Watcher {
    pub fn spawn(paths: Paths, profile: SavedOrganization, tx: Sender<Msg>) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let stopping = Arc::clone(&stop);
        std::thread::spawn(move || {
            let (lock, changed) = &*stopping;
            let socket = match profile.runtime_paths(&paths) {
                Ok(paths) => paths.socket_path(),
                Err(error) => {
                    let _ = tx.send(Msg::OrganizationStatus(Err(error.to_string())));
                    return;
                }
            };
            let mut client = BoundedClient::new(socket, Duration::from_secs(5));
            let mut previous = None;
            loop {
                if *lock.lock().unwrap() {
                    break;
                }
                let status = read(&paths, &profile, &mut client)
                    .map(Box::new)
                    .map_err(|error| error.to_string());
                if previous.as_ref() != Some(&status) {
                    previous = Some(status.clone());
                    if tx.send(Msg::OrganizationStatus(status)).is_err() {
                        break;
                    }
                }
                let _ = changed
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(2), |stopped| {
                        !*stopped
                    })
                    .unwrap();
            }
        });
        Self(stop)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        let (lock, changed) = &*self.0;
        *lock.lock().unwrap() = true;
        changed.notify_one();
    }
}

fn read(
    paths: &Paths,
    profile: &SavedOrganization,
    client: &mut BoundedClient,
) -> anyhow::Result<DaemonStatus> {
    anyhow::ensure!(
        list(paths)?.iter().any(|saved| saved == profile),
        "This organization sign-in was removed. Sign in again with cww login --organization."
    );
    let status = client.request(&ControlRequest::Status)?;
    anyhow::ensure!(
        status["server"] == profile.server.url
            && status["device_id"] == profile.server.device_id
            && status["account"]["id"] == profile.account.id
            && status["chat_only"] == true,
        "This connection belongs to a different organization."
    );
    anyhow::ensure!(
        status["paired"] == true && status["connection"]["connection"] != "revoked",
        "This sign-in was revoked. Sign in again with cww login --organization."
    );
    Ok(serde_json::from_value(status)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn status_is_bound_to_the_saved_grant_and_forgetting_stops_requests() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::under(temp.path());
        let profile: SavedOrganization = serde_json::from_value(json!({"id":"a".repeat(43), "server":{"url":"https://example.com","device_id":"second"},
            "account":{"id":"123456789","name":"Synthetic organization","role":"member"},"secret_store":"file"})).unwrap();
        let file = paths.config_dir.join("organizations.json");
        crate::paths::write_private_file(
            &file,
            &serde_json::to_vec(&json!({"version":1,"organizations":[profile]})).unwrap(),
        )
        .unwrap();
        let runtime = profile.runtime_paths(&paths).unwrap();
        runtime.ensure().unwrap();
        let socket = runtime.socket_path();
        let mut listener = crate::control::bind(&socket).unwrap();
        let serving = tokio::spawn(async move {
            for account in ["123456789", "987654321"] {
                let mut stream = BufReader::new(listener.accept().await.unwrap());
                let mut request = String::new();
                stream.read_line(&mut request).await.unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&request).unwrap()["cmd"],
                    "status"
                );
                let response = json!({"ok":true,"paired":true,"chat_only":true,"server":"https://example.com","device_id":"second", "account":{"id":account},"connection":{"connection":"connected"}});
                let bytes = format!("{response}\n");
                stream.get_mut().write_all(bytes.as_bytes()).await.unwrap();
            }
        });
        tokio::task::spawn_blocking(move || {
            let mut client = BoundedClient::new(socket, Duration::from_secs(1));
            assert_eq!(
                read(&paths, &profile, &mut client)
                    .unwrap()
                    .device_id
                    .as_deref(),
                Some("second")
            );
            assert!(read(&paths, &profile, &mut client).is_err());
            crate::paths::write_private_file(&file, br#"{"version":1,"organizations":[]}"#)
                .unwrap();
            let error = read(&paths, &profile, &mut client).unwrap_err().to_string();
            assert!(error.contains("removed"));
        })
        .await
        .unwrap();
        serving.await.unwrap();
    }
}
