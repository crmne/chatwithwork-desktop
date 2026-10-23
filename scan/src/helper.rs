//! The scan helper: a child process that holds the scanner backend, so a
//! driver that crashes or hangs takes only the helper with it.
//!
//! The app starts it (`cww-app --scan-helper`) and speaks JSON lines on its
//! stdin and stdout, one request at a time:
//!
//! | Request | Answers |
//! |---|---|
//! | `{"op":"devices"}` | `{"devices":[{"id","name","kind"}]}` |
//! | `{"op":"capabilities","device":…}` | `{"capabilities":{…}}` |
//! | `{"op":"scan","request":{…},"dir":…}` | a `{"page":{"path","resolution"}}` line per page as it arrives, then `{"end":"done"\|"no_paper"\|"cancelled"}` |
//! | `{"op":"cancel"}` | nothing: the scan running ends with `cancelled` |
//!
//! Any request may answer `{"error":{"code","message"}}` instead. Pages are
//! written as PNM files into `dir`, a private folder the app made for the
//! session, and never anywhere else.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Backend, Capabilities, Device, End, Request, ScanError};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Op {
    Devices,
    Capabilities { device: String },
    Scan { request: Request, dir: PathBuf },
    Cancel,
}

/// A page the helper wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageFile {
    pub path: PathBuf,
    pub resolution: u32,
}

fn error_line(e: &ScanError) -> Value {
    json!({ "error": { "code": e.code, "message": e.message } })
}

/// The helper's main loop, in the child process. Returns its exit code.
pub fn run(backend: impl FnOnce() -> Result<Box<dyn Backend>, ScanError>) -> i32 {
    let out = std::io::stdout();
    let mut out = BufWriter::new(out.lock());
    let mut say =
        move |value: Value| -> bool { writeln!(out, "{value}").and_then(|()| out.flush()).is_ok() };
    // Requests are read on their own thread, so `cancel` arrives while a
    // scan runs.
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Op>();
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Op>(&line) {
                    Ok(Op::Cancel) => cancel.store(true, Ordering::SeqCst),
                    Ok(op) => {
                        // Here, in order with a Cancel read after it: a
                        // Stop sent just after Start isn't lost.
                        if matches!(op, Op::Scan { .. }) {
                            cancel.store(false, Ordering::SeqCst);
                        }
                        if tx.send(op).is_err() {
                            break;
                        }
                    }
                    Err(_) => {}
                }
            }
        });
    }
    let mut backend = match backend() {
        Ok(backend) => Some(backend),
        Err(e) => {
            // Answer every request with why, rather than exiting at once,
            // so the page can say what to install.
            for _ in rx.iter() {
                if !say(error_line(&e)) {
                    return 1;
                }
            }
            None
        }
    };
    let Some(backend) = backend.as_mut() else {
        return 0;
    };
    for op in rx.iter() {
        let ok = match op {
            Op::Devices => say(match backend.devices() {
                Ok(devices) => json!({ "devices": devices }),
                Err(e) => error_line(&e),
            }),
            Op::Capabilities { device } => say(match backend.capabilities(&device) {
                Ok(caps) => json!({ "capabilities": caps }),
                Err(e) => error_line(&e),
            }),
            Op::Scan { request, dir } => {
                let mut n = 0;
                let mut alive = true;
                let result = backend.scan(&request, &cancel, &mut |scanned| {
                    n += 1;
                    let path = dir.join(format!("page-{n:04}.pnm"));
                    scanned
                        .image
                        .save_with_format(&path, image::ImageFormat::Pnm)
                        .map_err(|e| {
                            ScanError::new("disk", format!("Couldn't keep the page: {e}"))
                        })?;
                    alive =
                        say(json!({ "page": PageFile { path, resolution: scanned.resolution } }));
                    if alive {
                        Ok(())
                    } else {
                        Err(ScanError::new("gone", "The app went away."))
                    }
                });
                alive
                    && say(match result {
                        Ok(end) => json!({ "end": end }),
                        Err(e) => error_line(&e),
                    })
            }
            Op::Cancel => true,
        };
        if !ok {
            return 1;
        }
    }
    0
}

/// The app's side: a running helper.
pub struct Helper {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: BufReader<ChildStdout>,
}

/// What the helper answered, or that it died.
fn read(stdout: &mut BufReader<ChildStdout>) -> Result<Value, ScanError> {
    let mut line = String::new();
    match stdout.read_line(&mut line) {
        Ok(0) | Err(_) => Err(ScanError::new(
            "helper_stopped",
            "The scanner's driver stopped. Unplug the scanner, plug it in again, and look for scanners again.",
        )),
        Ok(_) => {
            let value: Value = serde_json::from_str(&line).map_err(|_| {
                ScanError::new(
                    "helper_stopped",
                    "The scanner's helper said something unexpected.",
                )
            })?;
            match value.get("error") {
                Some(error) => Err(ScanError {
                    code: leak_code(error["code"].as_str().unwrap_or("scanner_error")),
                    message: error["message"]
                        .as_str()
                        .unwrap_or("The scanner failed.")
                        .to_string(),
                }),
                None => Ok(value),
            }
        }
    }
}

/// Error codes are a small fixed set; keep them `'static` without leaking
/// anything a helper could make up.
fn leak_code(code: &str) -> &'static str {
    const KNOWN: [&str; 13] = [
        "no_sane",
        "unsupported",
        "not_found",
        "busy",
        "jammed",
        "cover_open",
        "access_denied",
        "invalid",
        "disk",
        "gone",
        "scanner_error",
        "helper_stopped",
        "no_paper",
    ];
    KNOWN
        .iter()
        .copied()
        .find(|k| *k == code)
        .unwrap_or("scanner_error")
}

impl Helper {
    /// Starts `command` as a helper.
    pub fn spawn(mut command: Command) -> Result<Self, ScanError> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                ScanError::new(
                    "helper_stopped",
                    format!("Couldn't start the scanner's helper: {e}"),
                )
            })?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped")));
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    fn send(&mut self, op: &Op) -> Result<(), ScanError> {
        let line = serde_json::to_string(op).expect("a request serializes");
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| ScanError::new("helper_stopped", "The scanner's helper stopped."))?;
        writeln!(stdin, "{line}")
            .and_then(|()| stdin.flush())
            .map_err(|_| ScanError::new("helper_stopped", "The scanner's helper stopped."))
    }

    pub fn devices(&mut self) -> Result<Vec<Device>, ScanError> {
        self.send(&Op::Devices)?;
        let value = read(&mut self.stdout)?;
        serde_json::from_value(value["devices"].clone())
            .map_err(|_| ScanError::new("scanner_error", "Couldn't list scanners."))
    }

    pub fn capabilities(&mut self, device: &str) -> Result<Capabilities, ScanError> {
        self.send(&Op::Capabilities {
            device: device.to_string(),
        })?;
        let value = read(&mut self.stdout)?;
        serde_json::from_value(value["capabilities"].clone())
            .map_err(|_| ScanError::new("scanner_error", "Couldn't read what the scanner can do."))
    }

    /// Scans into `dir`, handing each page to `page` as it arrives.
    pub fn scan(
        &mut self,
        request: &Request,
        dir: &Path,
        page: &mut dyn FnMut(PageFile),
    ) -> Result<End, ScanError> {
        self.send(&Op::Scan {
            request: request.clone(),
            dir: dir.to_path_buf(),
        })?;
        loop {
            let value = read(&mut self.stdout)?;
            if let Some(file) = value.get("page") {
                let file: PageFile = serde_json::from_value(file.clone())
                    .map_err(|_| ScanError::new("scanner_error", "A page went missing."))?;
                // Only files in the session's folder are taken.
                if file.path.parent() == Some(dir) {
                    page(file);
                }
            } else if let Some(end) = value.get("end") {
                return serde_json::from_value(end.clone())
                    .map_err(|_| ScanError::new("scanner_error", "The scan ended oddly."));
            }
        }
    }

    /// Stops the scan running, from another thread.
    pub fn canceller(&self) -> Canceller {
        Canceller(self.stdin.clone())
    }
}

/// Stops a helper's scan from another thread: the helper reads requests on
/// a thread of its own, so `cancel` reaches it while it scans.
#[derive(Clone)]
pub struct Canceller(Arc<Mutex<ChildStdin>>);

impl Canceller {
    pub fn cancel(&self) {
        let line = serde_json::to_string(&Op::Cancel).expect("serializes");
        if let Ok(mut stdin) = self.0.lock() {
            let _ = writeln!(stdin, "{line}").and_then(|()| stdin.flush());
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
