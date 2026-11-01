//! CUPS, through its own commands: `lpstat` and `lpoptions` to see printers,
//! `lp` to print. They're on every system that has CUPS, they honour the
//! person's settings and `CUPS_SERVER`, and they keep libcups out of this
//! binary. Every command runs in the C locale, since their output is read.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{Job, SystemPrinter};
use crate::commands::RunError;

/// `lpstat` can wait on printers found on the network.
const TIMEOUT: Duration = Duration::from_secs(5);
/// `lp` sends the whole file, up to 25 MiB, perhaps over a network: killed
/// partway, the job counts as one that may have printed. Within the 15
/// seconds the supervisor's channel gives a call.
const SUBMITTING: Duration = Duration::from_secs(12);
/// What listing all printers may take, within the 15 seconds the supervisor's
/// channel gives a call. Past it, printers are listed without what they can do.
const LISTING: Duration = Duration::from_secs(8);

pub(super) fn run(program: &str, args: &[&str]) -> Result<String, RunError> {
    run_within(program, args, TIMEOUT)
}

fn run_within(program: &str, args: &[&str], limit: Duration) -> Result<String, RunError> {
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RunError::new(
                    "unavailable",
                    format!("{program} isn't installed here, and printing needs CUPS's commands"),
                )
            } else {
                RunError::new("unavailable", format!("couldn't start {program}"))
            }
        })?;
    // Read as it comes: a command that fills a pipe nobody reads never ends.
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::new(
                    super::UNCONFIRMED,
                    format!("{program} took too long"),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return Err(RunError::new("unavailable", format!("{program} failed"))),
        }
    };
    let read = |pipe: Option<std::thread::JoinHandle<Vec<u8>>>| {
        pipe.and_then(|pipe| pipe.join().ok()).unwrap_or_default()
    };
    let (stdout, stderr) = (read(stdout), read(stderr));
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        let said = String::from_utf8_lossy(&stderr);
        let said: String = said.trim().chars().take(200).collect();
        Err(RunError::new("unavailable", said))
    }
}

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

pub fn printers() -> Result<Vec<SystemPrinter>, RunError> {
    let started = Instant::now();
    let names = run("lpstat", &["-e"])?;
    // With no default printer, `lpstat -d` says so and still exits 0; with
    // no printers at all, some versions fail.
    let default = run("lpstat", &["-d"]).unwrap_or_default();
    let default = parse_default(&default);
    let states = parse_states(&run("lpstat", &["-p"]).unwrap_or_default());
    Ok(names
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|queue| {
            let options = if started.elapsed() < LISTING {
                run("lpoptions", &["-p", queue, "-l"]).unwrap_or_default()
            } else {
                String::new()
            };
            let (duplex, color) = parse_options(&options);
            SystemPrinter {
                queue: queue.to_string(),
                description: None,
                default: default.as_deref() == Some(queue),
                state: states
                    .iter()
                    .find(|(name, _)| name == queue)
                    .map_or_else(|| "idle".to_string(), |(_, state)| state.clone()),
                duplex,
                color,
            }
        })
        .collect())
}

pub fn submit(job: &Job) -> Result<String, RunError> {
    let args = lp_args(job);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    parse_job(&run_within("lp", &args, SUBMITTING)?)
        .ok_or_else(|| RunError::new(super::UNCONFIRMED, "lp didn't say what became of the job"))
}

pub(super) fn lp_args(job: &Job) -> Vec<String> {
    let mut args = vec![
        "-d".to_string(),
        job.queue.clone(),
        "-n".to_string(),
        job.copies.to_string(),
        "-t".to_string(),
        job.title.clone(),
    ];
    match job.duplex {
        Some(true) => args.extend(["-o".into(), "sides=two-sided-long-edge".into()]),
        Some(false) => args.extend(["-o".into(), "sides=one-sided".into()]),
        None => {}
    }
    if let Some(pages) = &job.pages {
        args.extend(["-P".into(), pages.clone()]);
    }
    args.push("--".into());
    args.push(job.file.display().to_string());
    args
}

/// `system default destination: Brother`.
pub(super) fn parse_default(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("system default destination: "))
        .map(|name| name.trim().to_string())
}

/// `printer Brother is idle.  enabled since …`, `printer Brother now
/// printing Brother-12.  enabled since …`, `printer Brother disabled since …`.
pub(super) fn parse_states(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("printer ")?;
            let (name, rest) = rest.split_once(' ')?;
            let state = if rest.starts_with("disabled") {
                "stopped"
            } else if rest.starts_with("now printing") {
                "printing"
            } else {
                "idle"
            };
            Some((name.to_string(), state.to_string()))
        })
        .collect()
}

/// What `lpoptions -l` says it can do: two-sided printing, and colour.
pub(super) fn parse_options(text: &str) -> (bool, bool) {
    let choices = |keys: &[&str]| {
        text.lines().find_map(|line| {
            let (key, choices) = line.split_once(':')?;
            let key = key.split('/').next()?.trim();
            keys.iter()
                .any(|k| k.eq_ignore_ascii_case(key))
                .then(|| choices.trim().to_string())
        })
    };
    let duplex = choices(&["Duplex", "sides", "KMDuplex", "EFDuplex"]).is_some_and(|choices| {
        choices.split_whitespace().any(|choice| {
            let choice = choice.trim_start_matches('*').to_ascii_lowercase();
            choice.contains("duplex") || choice.contains("two-sided")
        })
    });
    let color = choices(&["ColorModel", "print-color-mode", "ColorMode"]).is_some_and(|choices| {
        choices.split_whitespace().any(|choice| {
            let choice = choice.trim_start_matches('*').to_ascii_lowercase();
            choice.contains("rgb") || choice.contains("cmyk") || choice == "color"
        })
    });
    (duplex, color)
}

/// `request id is Brother-12 (1 file(s))`.
pub(super) fn parse_job(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.split_once("request id is ")?.1;
        rest.split_whitespace().next().map(str::to_string)
    })
}
