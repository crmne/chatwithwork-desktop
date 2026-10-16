//! Saved chat sign-ins. Folder settings always keep the primary connection;
//! a selected organization gets its own permanently bound chat client.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use cww::auth::organizations::{SavedOrganization, list};
use cww::control::{BoundedClient, ControlRequest};

use crate::model::Status;
use crate::paths::Paths;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Choice {
    pub id: Option<String>,
    pub name: String,
    pub role: Option<String>,
    pub selected: bool,
}

#[derive(Clone, PartialEq)]
struct Loaded {
    profiles: Result<Vec<SavedOrganization>, String>,
    status: Option<(String, Result<Status, String>)>,
}

pub(super) struct State {
    pub profiles: Vec<SavedOrganization>,
    pub selected: Option<String>,
    pub error: Option<String>,
    primary_name: Option<(String, String, String)>,
    status: Option<Status>,
    known: bool,
    worker: Option<Worker>,
    generation: u64,
}

impl State {
    pub fn new(paths: &Paths, selected: Option<String>) -> Self {
        let result = list(&paths.daemon).map_err(|error| error.to_string());
        let error = result.as_ref().err().cloned();
        let profiles = result.unwrap_or_default();
        let selected = selected.filter(|id| profiles.iter().any(|p| &p.id == id));
        Self {
            profiles,
            selected,
            error,
            primary_name: None,
            status: None,
            known: false,
            worker: None,
            generation: 0,
        }
    }

    pub fn profile(&self) -> Option<&SavedOrganization> {
        self.profiles
            .iter()
            .find(|profile| Some(&profile.id) == self.selected.as_ref())
    }

    pub fn select(&mut self, id: Option<String>) -> bool {
        if id == self.selected
            || id
                .as_ref()
                .is_some_and(|id| !self.profiles.iter().any(|p| &p.id == id))
        {
            return false;
        }
        self.selected = id;
        self.status = None;
        self.known = false;
        self.refresh();
        true
    }

    pub fn refresh(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(worker) = &self.worker {
            worker.select(self.generation, self.selected.clone());
        }
    }

    pub fn socket(&self, paths: &Paths) -> PathBuf {
        self.profile()
            .and_then(|p| p.runtime_paths(&paths.daemon).ok())
            .map_or_else(|| paths.socket_path(), |p| p.socket_path())
    }

    pub fn status(&self, primary: Option<Status>) -> Option<Status> {
        if self.profile().is_some() {
            self.status.clone()
        } else {
            primary
        }
    }

    pub fn known(&self, primary: bool) -> bool {
        if self.profile().is_some() {
            self.known
        } else {
            primary
        }
    }

    pub fn remember_primary(&mut self, status: Option<&Status>, name: &str) {
        if self.selected.is_none()
            && !name.is_empty()
            && let Some(status) = status
            && let (Some(server), Some(device)) = (&status.server, &status.device_id)
        {
            self.primary_name = Some((server.clone(), device.clone(), name.into()));
        }
    }

    pub fn choices(&self, primary: Option<&Status>) -> Vec<Choice> {
        let mut choices = Vec::new();
        if let Some(primary) = primary.filter(|s| s.is_paired()) {
            let name = self
                .primary_name
                .as_ref()
                .filter(|(server, device, _)| {
                    primary.server.as_ref() == Some(server)
                        && primary.device_id.as_ref() == Some(device)
                })
                .map(|(_, _, name)| name.clone())
                .unwrap_or_else(|| "Connected organization".into());
            choices.push(Choice {
                id: None,
                name,
                role: None,
                selected: self.selected.is_none(),
            });
        }
        choices.extend(self.profiles.iter().map(|profile| Choice {
            id: Some(profile.id.clone()),
            name: profile.account.name.clone(),
            role: Some(profile.account.role.clone()),
            selected: self.selected.as_deref() == Some(&profile.id),
        }));
        choices
    }

    /// The worker polls without repainting an unchanged window. It owns one
    /// bounded client and publishes only changes; generations reject old work.
    pub fn poll(&mut self, paths: &Paths, ctx: &egui::Context) -> bool {
        let before = self.selected.clone();
        if let Some(received) = self.worker.as_ref().map(|worker| worker.events.try_recv()) {
            match received {
                Ok((generation, loaded)) => self.accept(generation, loaded),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.worker = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.worker.is_none() {
            match Worker::spawn(
                paths.daemon.clone(),
                ctx.clone(),
                self.generation,
                self.selected.clone(),
                Duration::from_secs(2),
            ) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.error = Some(format!("Could not check organization connections: {error}"));
                    ctx.request_repaint_after(Duration::from_secs(30));
                }
            }
        }
        self.selected != before
    }

    fn accept(&mut self, generation: u64, loaded: Loaded) {
        if generation != self.generation {
            return;
        }
        match loaded.profiles {
            Ok(profiles) => {
                self.profiles = profiles;
                self.error = None;
            }
            Err(error) => {
                self.profiles.clear();
                self.error = Some(error);
            }
        }
        if self.selected.is_some() && self.profile().is_none() {
            self.selected = None;
            self.status = None;
            self.known = false;
            self.refresh();
        }
        if let Some((id, result)) = loaded.status
            && self.selected.as_ref() == Some(&id)
        {
            self.known = true;
            match result {
                Ok(status) => self.status = Some(status),
                Err(error) => {
                    self.status = None;
                    self.error = Some(error);
                }
            }
        }
    }
}

struct Desired {
    generation: u64,
    selected: Option<String>,
    stop: bool,
}

struct Worker {
    desired: Arc<(Mutex<Desired>, Condvar)>,
    events: Receiver<(u64, Loaded)>,
}

impl Worker {
    fn spawn(
        paths: cww::paths::Paths,
        ctx: egui::Context,
        generation: u64,
        selected: Option<String>,
        interval: Duration,
    ) -> std::io::Result<Self> {
        let desired = Arc::new((
            Mutex::new(Desired {
                generation,
                selected,
                stop: false,
            }),
            Condvar::new(),
        ));
        let control = Arc::clone(&desired);
        let (tx, events) = sync_channel(1);
        std::thread::Builder::new()
            .name("cww-organizations".into())
            .spawn(move || {
                let (lock, changed) = &*control;
                let mut last = None;
                let mut client: Option<(PathBuf, BoundedClient)> = None;
                loop {
                    let state = lock.lock().unwrap();
                    if state.stop {
                        break;
                    }
                    let (generation, selected) = (state.generation, state.selected.clone());
                    drop(state);
                    let profiles = list(&paths).map_err(|error| error.to_string());
                    let profile = profiles.as_ref().ok().and_then(|profiles| {
                        profiles
                            .iter()
                            .find(|profile| Some(&profile.id) == selected.as_ref())
                    });
                    let status = profile.map(|profile| {
                        let result = (|| {
                            let socket = profile.runtime_paths(&paths)?.socket_path();
                            if client
                                .as_ref()
                                .is_none_or(|(previous, _)| *previous != socket)
                            {
                                client = Some((
                                    socket.clone(),
                                    BoundedClient::new(socket, Duration::from_secs(5)),
                                ));
                            }
                            let value = client
                                .as_mut()
                                .unwrap()
                                .1
                                .request(&ControlRequest::Status)?;
                            validated_status(profile, value)
                        })()
                        .map_err(|error| error.to_string());
                        (profile.id.clone(), result)
                    });
                    if selected.is_none() {
                        client = None;
                    }
                    let result = (generation, Loaded { profiles, status });
                    if last.as_ref() != Some(&result) {
                        match tx.try_send(result.clone()) {
                            Ok(()) => {
                                last = Some(result);
                                ctx.request_repaint();
                            }
                            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                            Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                        }
                    }
                    let state = lock.lock().unwrap();
                    let _ = changed
                        .wait_timeout_while(state, interval, |state| {
                            !state.stop && state.generation == generation
                        })
                        .unwrap();
                }
            })?;
        Ok(Self { desired, events })
    }

    fn select(&self, generation: u64, selected: Option<String>) {
        let (lock, changed) = &*self.desired;
        let mut state = lock.lock().unwrap();
        state.generation = generation;
        state.selected = selected;
        changed.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, changed) = &*self.desired;
        lock.lock().unwrap().stop = true;
        changed.notify_one();
    }
}

fn validated_status(
    profile: &SavedOrganization,
    value: serde_json::Value,
) -> anyhow::Result<Status> {
    anyhow::ensure!(
        value["server"] == profile.server.url
            && value["device_id"] == profile.server.device_id
            && value["account"]["id"] == profile.account.id
            && value["chat_only"] == true,
        "This connection belongs to a different organization."
    );
    Ok(serde_json::from_value(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile() -> SavedOrganization {
        serde_json::from_value(json!({"id":"a".repeat(43), "server":{"url":"https://example.com","device_id":"device-a"},
            "account":{"id":"123456789","name":"First organization","role":"member"},"secret_store":"file"})).unwrap()
    }

    fn state() -> (tempfile::TempDir, Paths, State) {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::under(temp.path());
        let profile = profile();
        cww::paths::write_private_file(
            &paths.daemon.config_dir.join("organizations.json"),
            &serde_json::to_vec(&json!({"version":1,"organizations":[profile]})).unwrap(),
        )
        .unwrap();
        let state = State::new(&paths, Some(profile.id));
        (temp, paths, state)
    }

    #[test]
    fn status_must_match_the_selected_grant_before_chat_can_start() {
        let (_temp, _paths, state) = state();
        assert!(state.status(Some(Status::default())).is_none());
        let profile = profile();
        let valid = json!({"server":profile.server.url,"device_id":profile.server.device_id,"account":profile.account,"chat_only":true});
        assert!(validated_status(&profile, valid.clone()).is_ok());
        for (field, value) in [
            ("server", json!("https://other.example")),
            ("device_id", json!("device-b")),
            ("account", json!({"id":"987654321"})),
            ("chat_only", json!(false)),
        ] {
            let mut wrong = valid.clone();
            wrong[field] = value;
            assert!(validated_status(&profile, wrong).is_err(), "{field}");
        }
    }

    #[test]
    fn retired_and_stale_grants_cannot_restore_a_chat_connection() {
        let (_temp, _paths, mut state) = state();
        state.accept(
            state.generation,
            Loaded {
                profiles: Ok(vec![]),
                status: Some((profile().id, Ok(Status::default()))),
            },
        );
        assert!(state.selected.is_none());
        assert!(state.status(None).is_none());
        state.accept(
            state.generation.wrapping_sub(1),
            Loaded {
                profiles: Ok(vec![profile()]),
                status: None,
            },
        );
        assert!(state.profiles.is_empty());
    }

    #[test]
    fn worker_only_publishes_changes_and_refresh_wakes_it() {
        let (_temp, paths, _state) = state();
        let worker = Worker::spawn(
            paths.daemon,
            egui::Context::default(),
            0,
            None,
            Duration::from_millis(10),
        )
        .unwrap();
        assert_eq!(
            worker
                .events
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .0,
            0
        );
        assert!(
            worker
                .events
                .recv_timeout(Duration::from_millis(60))
                .is_err()
        );
        worker.select(1, None);
        assert_eq!(
            worker
                .events
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .0,
            1
        );
    }
}
