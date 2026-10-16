use super::*;
use std::sync::mpsc;

fn view(account: &str, chat: u64) -> View {
    View {
        account: account.into(),
        chat,
    }
}

fn send(view: &View, visible: bool) -> Step {
    Step::Send {
        view: view.clone(),
        visible,
    }
}

#[test]
fn clears_old_account_and_chat_before_setting_the_latest_view() {
    let now = Instant::now();
    let a = view("123456789", 42);
    let b = view("987654321", 42);
    let c = view("987654321", 43);
    let mut delivery = Delivery::default();
    assert_eq!(delivery.next(Some(&a), now), send(&a, true));
    delivery.sent(a.clone(), true, true, now);
    assert_eq!(delivery.next(Some(&b), now), send(&a, false));
    delivery.sent(a.clone(), false, true, now);
    // The desired view changed again while clearing. B is never activated.
    assert_eq!(delivery.next(Some(&c), now), send(&c, true));
    delivery.sent(c.clone(), true, true, now);
    assert_eq!(delivery.next(None, now), send(&c, false));
    delivery.sent(c, false, true, now);
    assert_eq!(delivery.next(None, now + RENEW), Step::Wait(None));
}

#[test]
fn unchanged_views_renew_at_thirty_seconds_and_blur_clears_immediately() {
    let now = Instant::now();
    let view = view("123456789", 42);
    let mut delivery = Delivery::default();
    delivery.sent(view.clone(), true, true, now);
    assert_eq!(
        delivery.next(Some(&view), now + Duration::from_secs(29)),
        Step::Wait(Some(Duration::from_secs(1)))
    );
    assert_eq!(delivery.next(Some(&view), now + RENEW), send(&view, true));
    delivery.sent(view.clone(), true, true, now + RENEW);
    assert_eq!(delivery.next(None, now + RENEW), send(&view, false));
}

#[test]
fn failed_visible_and_failed_clear_cannot_leave_an_old_account_behind() {
    let now = Instant::now();
    let old = view("123456789", 42);
    let new = view("987654321", 42);
    let mut delivery = Delivery::default();
    delivery.sent(old.clone(), true, false, now);
    assert_eq!(delivery.next(Some(&new), now), Step::Wait(Some(RETRY)));
    assert_eq!(delivery.next(Some(&new), now + RETRY), send(&old, false));
    delivery.sent(old.clone(), false, false, now + RETRY);
    assert_eq!(
        delivery.next(Some(&new), now + RETRY * 2),
        send(&old, false)
    );
    delivery.sent(old, false, true, now + RETRY * 2);
    assert_eq!(delivery.next(Some(&new), now + RETRY * 2), send(&new, true));
}

#[test]
fn failure_retries_current_view_without_waiting_for_the_renewal() {
    let now = Instant::now();
    let view = view("123456789", 42);
    let mut delivery = Delivery::default();
    delivery.sent(view.clone(), true, false, now);
    assert_eq!(delivery.next(Some(&view), now + RETRY), send(&view, true));
    delivery.sent(view.clone(), true, true, now + RETRY);
    assert_eq!(
        delivery.next(Some(&view), now + RETRY),
        Step::Wait(Some(RENEW))
    );
}

#[test]
fn slow_sender_coalesces_updates_and_drop_clears_without_blocking() {
    let (tx, rx) = mpsc::channel();
    let (finish, wait) = mpsc::channel();
    let presence = Presence::with_sender(move |request| {
        tx.send(request).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap()
    });
    let a = view("123456789", 42);
    let c = view("987654321", 99);
    presence.set(Some(a.clone()));
    let (session, account, chat, visible) = receive(&rx);
    assert_eq!((account, chat, visible), (a.account.clone(), 42, true));
    presence.set(Some(view("987654321", 42)));
    presence.set(Some(c.clone()));
    finish.send(Ok(())).unwrap();
    assert_eq!(receive(&rx), (session.clone(), a.account, 42, false));
    finish.send(Ok(())).unwrap();
    assert_eq!(receive(&rx), (session.clone(), c.account.clone(), 99, true));
    // Dropping must return even while the control request is still blocked.
    drop(presence);
    finish.send(Err("reply lost".into())).unwrap();
    assert_eq!(receive(&rx), (session, c.account, 99, false));
    finish.send(Err("daemon gone".into())).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn invalid_view_clears_and_different_windows_have_distinct_sessions() {
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    let first = Presence::with_sender(move |request| {
        tx.send(request).unwrap();
        Ok(())
    });
    let second = Presence::with_sender(move |request| {
        tx2.send(request).unwrap();
        Ok(())
    });
    first.set(Some(view("123456789", 42)));
    let first_session = receive(&rx).0;
    second.set(Some(view("123456789", 42)));
    let second_session = receive(&rx).0;
    assert_ne!(first_session, second_session);
    for session in [&first_session, &second_session] {
        assert!(session.len() <= 64);
        assert!(
            session
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        );
    }
    first.set(Some(view("untrusted/path", 42)));
    assert_eq!(receive(&rx), (first_session, "123456789".into(), 42, false));
    drop(first);
    drop(second);
    assert_eq!(
        receive(&rx),
        (second_session, "123456789".into(), 42, false)
    );
}

fn receive(rx: &mpsc::Receiver<ControlRequest>) -> (String, String, u64, bool) {
    let ControlRequest::NotificationView {
        session,
        account,
        chat,
        visible,
    } = rx.recv_timeout(Duration::from_secs(5)).unwrap()
    else {
        panic!("a presence request")
    };
    (session, account, chat, visible)
}
