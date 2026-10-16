//! Foreground chat presence shared by the native desktop and terminal.
//!
//! Keep one [`Presence`] per window. Call [`Presence::set`] with the account
//! ID from the displayed chat list and the displayed chat number, only while
//! that transcript is actually in the foreground. Use `None` on blur, other
//! screens, loading, or a pairing change. Merely following a chat is not
//! presence. Unchanged views renew every 30 seconds on a dedicated thread;
//! neither the UI thread nor a Tokio runtime is blocked by control requests.
//! Each control attempt has a five-second deadline for connect, write, and
//! response together, including Windows named pipes. The worker reuses one
//! current-thread async runtime; timed-out I/O is dropped, never abandoned on
//! another thread.
//!
//! Requests use one unique session and clear its old view before setting a
//! new one. Rapid changes coalesce to the latest desired view. Drop requests a
//! final clear without blocking the UI; the daemon's 75-second expiry covers
//! a crashed process or a control channel that can no longer accept it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::control::ControlRequest;

const RENEW: Duration = Duration::from_secs(30);
const RETRY: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// The displayed organization's external ID, not the daemon's account.
    pub account: String,
    pub chat: u64,
}

impl View {
    fn valid(&self) -> bool {
        (7..=20).contains(&self.account.len())
            && self.account.bytes().all(|byte| byte.is_ascii_digit())
            && self.chat > 0
    }
}

#[derive(Default)]
struct Desired {
    view: Option<View>,
    closing: bool,
}

/// A nonblocking foreground presence handle. All sends happen serially on its
/// worker thread. Dropping the handle stops renewals and requests a clear.
pub struct Presence {
    shared: Arc<(Mutex<Desired>, Condvar)>,
}

impl Presence {
    pub fn new(socket: PathBuf) -> Self {
        let mut sender = crate::control::BoundedClient::new(socket, Duration::from_secs(5));
        Self::with_sender(move |request| {
            sender
                .request(&request)
                .map(drop)
                .map_err(|error| error.to_string())
        })
    }

    /// Supply a blocking control sender, called only by this handle's worker.
    /// Success means the daemon accepted the request, not a notification was
    /// delivered. The sender must have a finite I/O timeout.
    pub fn with_sender(
        sender: impl FnMut(ControlRequest) -> Result<(), String> + Send + 'static,
    ) -> Self {
        let shared = Arc::new((Mutex::new(Desired::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        // Failure to start a worker must not prevent opening the app. In that
        // case no presence is reported, so normal notification fallback stays.
        let _ = thread::Builder::new()
            .name("notification-presence".into())
            .spawn(move || run(worker, sender, session()));
        Self { shared }
    }

    /// Set the latest view. Repeated calls with the same view are inexpensive
    /// and do not delay its periodic renewal. Invalid or absent views clear it.
    pub fn set(&self, view: Option<View>) {
        let view = view.filter(View::valid);
        let (state, changed) = &*self.shared;
        let mut desired = state.lock().unwrap();
        if desired.view != view {
            desired.view = view;
            changed.notify_one();
        }
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let (state, changed) = &*self.shared;
        let mut desired = state.lock().unwrap();
        desired.closing = true;
        desired.view = None;
        changed.notify_one();
    }
}

fn session() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "p{:x}-{time:x}-{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Send { view: View, visible: bool },
    Wait(Option<Duration>),
}

#[derive(Default)]
struct Delivery {
    // Even a failed send may have reached the daemon before its reply was
    // lost. Remember it until a clear succeeds before sending a different view.
    observed: Option<View>,
    renew_at: Option<Instant>,
    retry_at: Option<Instant>,
}

impl Delivery {
    fn next(&self, desired: Option<&View>, now: Instant) -> Step {
        if let Some(retry) = self.retry_at
            && retry > now
        {
            return Step::Wait(Some(retry - now));
        }
        if let Some(previous) = &self.observed
            && Some(previous) != desired
        {
            return Step::Send {
                view: previous.clone(),
                visible: false,
            };
        }
        let Some(view) = desired else {
            return Step::Wait(None);
        };
        if self.observed.as_ref() == Some(view)
            && let Some(renew) = self.renew_at
            && renew > now
        {
            return Step::Wait(Some(renew - now));
        }
        Step::Send {
            view: view.clone(),
            visible: true,
        }
    }

    fn sent(&mut self, view: View, visible: bool, success: bool, now: Instant) {
        if visible {
            self.observed = Some(view);
            self.renew_at = success.then_some(now + RENEW);
        } else if success {
            self.observed = None;
            self.renew_at = None;
        }
        self.retry_at = (!success).then_some(now + RETRY);
    }
}

fn run(
    shared: Arc<(Mutex<Desired>, Condvar)>,
    mut sender: impl FnMut(ControlRequest) -> Result<(), String>,
    session: String,
) {
    let (state, changed) = &*shared;
    let mut delivery = Delivery::default();
    loop {
        let desired = state.lock().unwrap();
        let closing = desired.closing;
        let step = if closing {
            match &delivery.observed {
                Some(view) => Step::Send {
                    view: view.clone(),
                    visible: false,
                },
                None => return,
            }
        } else {
            delivery.next(desired.view.as_ref(), Instant::now())
        };
        match step {
            Step::Send { view, visible } => {
                drop(desired);
                let success = sender(ControlRequest::NotificationView {
                    session: session.clone(),
                    account: view.account.clone(),
                    chat: view.chat,
                    visible,
                })
                .is_ok();
                delivery.sent(view, visible, success, Instant::now());
                if closing {
                    return;
                }
                // Re-read the latest desired state after every blocking call.
            }
            Step::Wait(Some(wait)) => {
                drop(changed.wait_timeout(desired, wait).unwrap());
            }
            Step::Wait(None) => {
                drop(changed.wait(desired).unwrap());
            }
        }
    }
}

#[cfg(test)]
mod tests;
