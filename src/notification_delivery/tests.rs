use super::*;

struct Fixture {
    bridge: Bridge,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Receiver<Output>,
    opened: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let (native, commands) = mpsc::channel(128);
        let (output, events) = mpsc::channel(128);
        let opened = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&opened);
        let bridge = Bridge::new(
            Some(native),
            output,
            Arc::new(move |activation| sink.lock().unwrap().push(activation.browser_url())),
        );
        let mut fixture = Self {
            bridge,
            commands,
            events,
            opened,
        };
        fixture.connect("482139075");
        fixture
            .bridge
            .observation(Event::Permission {
                state: PermissionState::Granted,
            })
            .unwrap();
        fixture.drain();
        fixture
    }

    fn connect(&mut self, account: &str) {
        self.bridge
            .command(Input::Attach {
                generation: 1,
                server: "https://staging.example.com".into(),
                device_id: "7".into(),
            })
            .unwrap();
        self.bridge
            .server(ServerEvent::Ready {
                account: account.into(),
                device_id: 7,
                heartbeat_after: 30,
                visibility_ttl: 75,
            })
            .unwrap();
    }

    fn actions(&mut self) -> Vec<Value> {
        let mut actions = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            if let Output::Action { data, .. } = event {
                actions.push(data);
            }
        }
        actions
    }

    fn drain(&mut self) {
        while self.commands.try_recv().is_ok() {}
        self.actions();
    }

    fn show(&mut self, id: u64, chat: u64) -> (String, Option<u64>) {
        self.bridge.show(offer(id, chat)).unwrap();
        loop {
            match self.commands.try_recv().unwrap() {
                Command::Show { id, attempt, .. } => return (id, attempt),
                Command::Withdraw { .. } => {}
                other => panic!("unexpected command: {other:?}"),
            }
        }
    }
}

fn offer(id: u64, chat: u64) -> Offer {
    Offer {
        notification_id: format!("00000000-0000-4000-8000-{id:012x}"),
        delivery_token: format!("{id:048x}"),
        account: "482139075".into(),
        chat,
        kind: "answered".into(),
        title: "Example organization".into(),
        body: "Your answer is ready.".into(),
        thread: format!("chat-482139075-{chat}"),
        expires_at: (OffsetDateTime::now_utc() + time::Duration::seconds(14))
            .format(&Rfc3339)
            .unwrap(),
    }
}

#[test]
fn native_acceptance_is_the_only_receipt_and_a_duplicate_is_ignored() {
    let mut fixture = Fixture::new();
    let (id, attempt) = fixture.show(1, 16);
    assert!(fixture.actions().is_empty());
    fixture
        .bridge
        .observation(Event::Ready {
            protocol: protocol::VERSION,
        })
        .unwrap();
    assert!(fixture.actions().is_empty());
    fixture
        .bridge
        .observation(Event::Shown {
            id: id.clone(),
            attempt,
        })
        .unwrap();
    let actions = fixture.actions();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["status"], "shown");
    assert_eq!(actions[0]["delivery_token"], format!("{:048x}", 1));
    fixture
        .bridge
        .observation(Event::Shown { id, attempt })
        .unwrap();
    fixture.bridge.show(offer(1, 16)).unwrap();
    assert!(fixture.actions().is_empty());
    assert!(fixture.commands.try_recv().is_err());
}

#[test]
fn same_thread_replacement_rejects_already_buffered_callbacks() {
    let mut fixture = Fixture::new();
    let (id, first) = fixture.show(1, 16);
    let (replacement, second) = fixture.show(2, 16);
    assert_eq!(id, replacement);
    assert!(second > first);
    fixture.actions();
    for event in [
        Event::Shown {
            id: id.clone(),
            attempt: first,
        },
        Event::Closed {
            id: id.clone(),
            attempt: first,
        },
        Event::Activated {
            id: id.clone(),
            attempt: first,
            action: None,
        },
    ] {
        fixture.bridge.observation(event).unwrap();
    }
    assert!(fixture.actions().is_empty());
    assert!(fixture.opened.lock().unwrap().is_empty());
    fixture
        .bridge
        .observation(Event::Shown {
            id: id.clone(),
            attempt: second,
        })
        .unwrap();
    assert_eq!(
        fixture.actions()[0]["delivery_token"],
        format!("{:048x}", 2)
    );
    fixture
        .bridge
        .observation(Event::Activated {
            id: id.clone(),
            attempt: second,
            action: None,
        })
        .unwrap();
    fixture
        .bridge
        .observation(Event::Activated {
            id,
            attempt: second,
            action: None,
        })
        .unwrap();
    assert_eq!(
        *fixture.opened.lock().unwrap(),
        vec!["https://staging.example.com/482139075/chats/16"]
    );
}

#[test]
fn withdrawal_disconnect_and_wrong_tokens_cannot_acknowledge_or_open() {
    let mut fixture = Fixture::new();
    let (id, attempt) = fixture.show(1, 16);
    fixture
        .bridge
        .server(ServerEvent::Withdraw {
            notification_id: offer(1, 16).notification_id,
            delivery_token: "wrong".into(),
        })
        .unwrap();
    assert_eq!(fixture.bridge.active.len(), 1);
    fixture
        .bridge
        .server(ServerEvent::Withdraw {
            notification_id: offer(1, 16).notification_id,
            delivery_token: offer(1, 16).delivery_token,
        })
        .unwrap();
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        Command::Withdraw { .. }
    ));
    fixture
        .bridge
        .observation(Event::Shown {
            id: id.clone(),
            attempt,
        })
        .unwrap();
    fixture
        .bridge
        .observation(Event::Activated {
            id,
            attempt,
            action: None,
        })
        .unwrap();
    assert!(fixture.actions().is_empty());
    let (id, attempt) = fixture.show(2, 17);
    fixture
        .bridge
        .observation(Event::Shown {
            id: id.clone(),
            attempt,
        })
        .unwrap();
    fixture
        .bridge
        .command(Input::Detach { generation: 1 })
        .unwrap();
    fixture
        .bridge
        .observation(Event::Activated {
            id,
            attempt,
            action: None,
        })
        .unwrap();
    assert!(fixture.opened.lock().unwrap().is_empty());
    assert!(!fixture.bridge.status().connected);
}

#[test]
fn expiry_and_failure_emit_only_failure_and_never_replay() {
    let mut fixture = Fixture::new();
    let (id, attempt) = fixture.show(1, 16);
    fixture.bridge.active.get_mut(&id).unwrap().deadline = Instant::now() - Duration::from_secs(1);
    fixture
        .bridge
        .observation(Event::Shown { id, attempt })
        .unwrap();
    assert!(fixture.actions().is_empty());
    fixture.bridge.tick().unwrap();
    assert_eq!(fixture.actions()[0]["status"], "failed");
    fixture.drain();
    fixture.show(2, 17);
    fixture
        .bridge
        .observation(Event::Failed {
            id: None,
            attempt: None,
            message: "helper stopped".into(),
        })
        .unwrap();
    let actions = fixture.actions();
    assert_eq!(actions[0]["status"], "failed");
    assert!(!fixture.bridge.status().available);
    fixture.drain();
    fixture
        .bridge
        .observation(Event::Permission {
            state: PermissionState::Granted,
        })
        .unwrap();
    assert!(fixture.commands.try_recv().is_err());
    assert!(fixture.bridge.active.is_empty());
}

#[test]
fn helper_readiness_is_not_permission_and_only_local_request_prompts() {
    let mut fixture = Fixture::new();
    fixture.bridge.permission = Permission::Unavailable;
    fixture
        .bridge
        .observation(Event::Ready {
            protocol: protocol::VERSION,
        })
        .unwrap();
    assert!(!fixture.bridge.status().available);
    fixture.bridge.permission(false);
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        Command::Permission
    ));
    fixture
        .bridge
        .observation(Event::Permission {
            state: PermissionState::NotDetermined,
        })
        .unwrap();
    fixture.bridge.show(offer(1, 16)).unwrap();
    assert!(fixture.commands.try_recv().is_err());
    assert!(!fixture.bridge.status().available);
    fixture
        .bridge
        .command(Input::Permission { request: true })
        .unwrap();
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        Command::RequestPermission
    ));
    fixture
        .bridge
        .observation(Event::Permission {
            state: PermissionState::Denied,
        })
        .unwrap();
    assert!(!fixture.bridge.status().available);
}

#[test]
fn unknown_permission_attempts_delivery_but_failure_keeps_fallback() {
    let mut fixture = Fixture::new();
    fixture
        .bridge
        .observation(Event::Permission {
            state: PermissionState::Unknown,
        })
        .unwrap();
    assert_eq!(fixture.bridge.status().permission, Permission::Unknown);
    assert!(fixture.bridge.status().available);
    fixture.actions();
    let (id, attempt) = fixture.show(1, 16);
    assert!(fixture.actions().is_empty());
    fixture
        .bridge
        .observation(Event::Shown { id, attempt })
        .unwrap();
    assert_eq!(fixture.actions()[0]["status"], "shown");
    let (id, attempt) = fixture.show(2, 17);
    fixture
        .bridge
        .observation(Event::Failed {
            id: Some(id),
            attempt,
            message: "service unavailable".into(),
        })
        .unwrap();
    assert_eq!(fixture.actions()[0]["status"], "failed");
    assert!(!fixture.bridge.status().available);
}

#[cfg(unix)]
fn fake_helper() -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let path = dir.path().join("helper");
    let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("sh"))
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
        .unwrap();
    let script = r#"
[ "$1" = --notification-broker ] || exit 9
printf '%s\n' '{"event":"ready","protocol":2}'
while IFS= read -r line; do
  case "$line" in
*'"command":"permission"'*) printf '%s\n' '{"event":"permission","state":"unknown"}' ;;
*'"command":"show"'*)
  attempt=${line#*'"attempt":'}; attempt=${attempt%%,*}
  id=${line#*'"id":"'}; id=${id%%\"*}
  printf '{"event":"shown","id":"%s","attempt":%s}\n' "$id" "$attempt"
  ;;
*'"command":"exit"'*) exit 0 ;;
  esac
done
"#;
    std::fs::write(&path, format!("#!{}\n{script}", shell.display())).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    (dir, path)
}

#[cfg(unix)]
#[tokio::test]
async fn actual_supervisor_receipts_cross_private_pipes_without_a_desktop() {
    let (_dir, path) = fake_helper();
    let shutdown = CancellationToken::new();
    let mut parent = spawn_with_activation(
        Some(path),
        shutdown.child_token(),
        Arc::new(|_| panic!("no activation was sent")),
    );
    let (worker, parent_pipe) = tokio::io::duplex(8192);
    let (parent_input, parent_output) = tokio::io::split(parent_pipe);
    let relay_shutdown = shutdown.child_token();
    let stop_relay = relay_shutdown.clone();
    let relay = tokio::spawn(async move {
        serve_worker(parent_input, parent_output, &mut parent, relay_shutdown)
            .await
            .unwrap();
        parent
    });
    let (worker_input, worker_output) = tokio::io::split(worker);
    let endpoint = pipe::connect(worker_input, worker_output, shutdown.child_token());
    let client = Client::new(endpoint, shutdown.child_token());
    let server = ServerUrl::parse("https://staging.example.com").unwrap();
    let mut session = client.attach(&server, "7").unwrap();
    session.receive(json!({"type":"ready", "account":"482139075", "device_id":7, "heartbeat_after":30, "visibility_ttl":75})).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(action) = session.actions.recv().await {
            if action == json!({"action":"heartbeat", "available":true}) {
                return;
            }
        }
        panic!("delivery ended");
    })
    .await
    .unwrap();
    let offer = offer(1, 16);
    session.receive(json!({"type":"show", "notification_id":offer.notification_id, "delivery_token":offer.delivery_token, "account":offer.account, "chat":offer.chat, "kind":offer.kind, "title":offer.title, "body":offer.body, "thread":offer.thread, "expires_at":offer.expires_at})).unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(5), session.actions.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt["status"], "shown");
    assert_eq!(receipt["delivery_token"], format!("{:048x}", 1));
    drop(session);
    stop_relay.cancel();
    let parent = tokio::time::timeout(Duration::from_secs(5), relay)
        .await
        .unwrap()
        .unwrap();
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), parent.task)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn invalid_accounts_devices_payloads_and_expiry_never_reach_the_helper() {
    let mut fixture = Fixture::new();
    let mut wrong = offer(1, 16);
    wrong.account = "999999999".into();
    fixture.bridge.show(wrong).unwrap();
    let mut wrong = offer(2, 16);
    wrong.thread = "../../private".into();
    fixture.bridge.show(wrong).unwrap();
    let mut wrong = offer(3, 16);
    wrong.delivery_token = "secret".into();
    fixture.bridge.show(wrong).unwrap();
    let mut wrong = offer(4, 16);
    wrong.expires_at = "2000-01-01T00:00:00Z".into();
    fixture.bridge.show(wrong).unwrap();
    assert!(fixture.commands.try_recv().is_err());
    fixture
        .bridge
        .server(ServerEvent::Ready {
            account: "482139075".into(),
            device_id: 8,
            heartbeat_after: 30,
            visibility_ttl: 75,
        })
        .unwrap();
    assert!(!fixture.bridge.status().connected);
    assert!(valid_account("1234567"));
    assert!(!valid_account("123/4567"));
    assert!(!valid_account("1234567?"));
}

#[test]
fn thirty_third_thread_retires_oldest_without_waiting_for_closed() {
    let mut fixture = Fixture::new();
    let mut oldest = None;
    for index in 1..=32 {
        let (id, attempt) = fixture.show(index, index);
        if oldest.is_none() {
            oldest = Some((id.clone(), attempt));
        }
        fixture
            .bridge
            .observation(Event::Shown { id, attempt })
            .unwrap();
        fixture.actions();
    }
    fixture.bridge.show(offer(33, 33)).unwrap();
    let (old_id, old_attempt) = oldest.unwrap();
    assert!(
        matches!(fixture.commands.try_recv().unwrap(), Command::Withdraw { id } if id == old_id)
    );
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        Command::Show {
            attempt: Some(33),
            ..
        }
    ));
    assert_eq!(fixture.bridge.active.len(), 32);
    fixture
        .bridge
        .observation(Event::Activated {
            id: old_id,
            attempt: old_attempt,
            action: None,
        })
        .unwrap();
    assert!(fixture.opened.lock().unwrap().is_empty());
    assert!(fixture.actions().is_empty());
}

#[test]
fn foreground_views_are_account_scoped_aggregated_and_expire() {
    let mut fixture = Fixture::new();
    fixture
        .bridge
        .view("window1".into(), "999999999".into(), 16, true)
        .unwrap();
    assert!(fixture.actions().is_empty());
    fixture
        .bridge
        .view("window1".into(), "482139075".into(), 16, true)
        .unwrap();
    fixture
        .bridge
        .view("window2".into(), "482139075".into(), 16, true)
        .unwrap();
    fixture.actions();
    fixture
        .bridge
        .view("window1".into(), "482139075".into(), 16, false)
        .unwrap();
    assert!(fixture.actions().is_empty());
    fixture.bridge.views.get_mut("window2").unwrap().until =
        Instant::now() - Duration::from_secs(1);
    fixture.bridge.tick().unwrap();
    assert_eq!(
        fixture.actions(),
        vec![json!({"action":"view", "chat":16, "visible":false})]
    );
    fixture
        .bridge
        .view("window1".into(), "482139075".into(), 17, true)
        .unwrap();
    fixture.actions();
    fixture
        .bridge
        .view("window1".into(), "482139075".into(), 16, false)
        .unwrap();
    assert!(fixture.actions().is_empty());
    assert_eq!(fixture.bridge.views["window1"].chat, 17);
}

#[test]
fn reset_keeps_attempts_increasing_but_drops_every_old_route() {
    let mut fixture = Fixture::new();
    let (id, attempt) = fixture.show(1, 16);
    fixture.bridge.command(Input::Reset).unwrap();
    fixture.connect("482139075");
    fixture.drain();
    let (_, next) = fixture.show(2, 17);
    assert!(next > attempt);
    fixture
        .bridge
        .observation(Event::Shown { id, attempt })
        .unwrap();
    assert!(fixture.actions().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn reset_discards_a_full_native_queue_before_allowing_new_work() {
    let (_dir, path) = fake_helper();
    let mut native = Native::new(Some(path), CancellationToken::new());
    let commands = native.restart().await.unwrap();
    let mut fixture = Fixture::new();
    fixture.bridge.native = Some(commands.clone());
    fixture.bridge.restart_native = false;
    // No scheduler yield: all 32 shows are still queued in the old
    // supervisor, so every best-effort withdrawal would fail admission.
    for index in 1..=32 {
        fixture.bridge.show(offer(index, index)).unwrap();
    }
    assert_eq!(commands.capacity(), 0);
    fixture.bridge.command(Input::Reset).unwrap();
    assert!(fixture.bridge.restart_native);
    assert!(fixture.bridge.active.is_empty());
    assert!(
        fixture.events.try_recv().is_err(),
        "reset cannot acknowledge before reaping native work"
    );
    fixture.bridge.native = native.restart().await;
    assert!(commands.is_closed());
    fixture.bridge.restart_native = false;
    fixture.connect("482139075");
    fixture.bridge.native(Command::Permission);
    for _ in 0..2 {
        let event = tokio::time::timeout(
            Duration::from_secs(5),
            native.events.as_mut().unwrap().recv(),
        )
        .await
        .unwrap()
        .unwrap();
        fixture.bridge.observation(event).unwrap();
    }
    assert!(fixture.bridge.status().available);
    fixture.actions();
    fixture.bridge.show(offer(33, 33)).unwrap();
    let event = tokio::time::timeout(
        Duration::from_secs(5),
        native.events.as_mut().unwrap().recv(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        event,
        Event::Shown {
            attempt: Some(33),
            ..
        }
    ));
    fixture.bridge.observation(event).unwrap();
    assert_eq!(
        fixture.actions()[0]["delivery_token"],
        format!("{:048x}", 33)
    );
    native.stop().await;
}

#[tokio::test]
async fn command_pressure_reserves_connection_boundaries_and_recovers() {
    let (commands, mut incoming) = mpsc::channel(CAPACITY);
    let (_output, events) = mpsc::channel(CAPACITY);
    let shutdown = CancellationToken::new();
    let stop = shutdown.child_token();
    let task_stop = stop.clone();
    let endpoint = Endpoint {
        commands,
        events,
        stop,
        task: tokio::spawn(async move { task_stop.cancelled().await }),
    };
    let client = Client::new(endpoint, shutdown.clone());
    for _ in 0..(CAPACITY - 8) {
        client.permission(false).unwrap();
    }
    assert!(client.permission(false).is_err());
    let server = ServerUrl::parse("https://staging.example.com").unwrap();
    let session = client.attach(&server, "7").unwrap();
    drop(session);
    let mut saw_detach = false;
    while let Ok(command) = incoming.try_recv() {
        saw_detach |= matches!(command, Input::Detach { .. });
    }
    assert!(saw_detach);
    client.permission(false).unwrap();
    let session = client.attach(&server, "7").unwrap();
    assert_eq!(session.generation, 2);
    shutdown.cancel();
}

#[tokio::test]
async fn slow_event_consumer_reconnects_without_killing_the_delivery_client() {
    let shutdown = CancellationToken::new();
    let mut endpoint = spawn(None, shutdown.clone());
    endpoint
        .commands
        .send(Input::Attach {
            generation: 1,
            server: "https://staging.example.com".into(),
            device_id: "7".into(),
        })
        .await
        .unwrap();
    endpoint.commands.send(Input::Server { generation:1, event:json!({"type":"ready", "account":"482139075", "device_id":7, "heartbeat_after":30, "visibility_ttl":75}) }).await.unwrap();
    let producer = endpoint.commands.clone();
    let flood = tokio::spawn(async move {
        for _ in 0..200 {
            producer
                .send(Input::View {
                    session: "window".into(),
                    account: "482139075".into(),
                    chat: 16,
                    visible: true,
                })
                .await
                .unwrap();
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while endpoint.events.len() < CAPACITY {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = endpoint.events.recv().await {
            if matches!(event, Output::Disconnected { generation: 1 }) {
                return;
            }
        }
        panic!("delivery endpoint stopped");
    })
    .await
    .unwrap();
    flood.await.unwrap();
    endpoint
        .commands
        .send(Input::Attach {
            generation: 2,
            server: "https://staging.example.com".into(),
            device_id: "7".into(),
        })
        .await
        .unwrap();
    endpoint.commands.send(Input::Server { generation:2, event:json!({"type":"ready", "account":"482139075", "device_id":7, "heartbeat_after":30, "visibility_ttl":75}) }).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = endpoint.events.recv().await {
            if let Output::Action {
                generation: 2,
                data,
            } = event
            {
                assert_eq!(data["action"], "heartbeat");
                return;
            }
        }
        panic!("delivery did not recover");
    })
    .await
    .unwrap();
    shutdown.cancel();
    endpoint.task.await.unwrap();
}
