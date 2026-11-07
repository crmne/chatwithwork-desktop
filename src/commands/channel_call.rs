//! The calls a daemon makes to whatever runs its commands, and how they're
//! answered: the same here and in the supervisor (see `channel`).

use serde::{Deserialize, Serialize};

use super::{Request, RunError, Runs, valid_id};
use crate::config::Config;
use crate::paths::Paths;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "snake_case", deny_unknown_fields)]
pub enum Call {
    Start {
        request: Request,
    },
    Status {
        run: String,
    },
    Output {
        run: String,
        offset: u64,
        max: usize,
    },
    Finished {
        run: String,
    },
    Stop {
        run: String,
    },
    /// Folders changed: stop runs where commands aren't allowed anymore.
    Prune,
    /// The runs kept, for this computer's own windows.
    List,
    /// The device was revoked: stop every run.
    StopAll,
    /// The system's printers, and the pages left today.
    Printers,
    /// Print a file that arrived (`crate::printing`).
    Print {
        request: crate::printing::Request,
    },
    /// A bot's first run on a repository (`crate::coding::prepare`): start
    /// preparing it, or say it's prepared or what's missing.
    Prepare {
        ask: crate::coding::prepare::Ask,
    },
    /// How preparing it stands.
    Prepared {
        ask: crate::coding::prepare::Ask,
    },
    /// What this computer has for the coding agents, and whether the
    /// person's Git login reaches a repository, for the app's dialog only.
    CodingLook {
        repository: Option<String>,
    },
}

/// Answer one call, as the supervisor does.
pub fn answer(runs: &Runs, paths: &Paths, call: Call) -> Result<serde_json::Value, RunError> {
    let run_id = |run: &str| {
        if valid_id(run) {
            Ok(())
        } else {
            Err(RunError::new("not_found", "no such command"))
        }
    };
    let config = || {
        Config::load(paths).map_err(|_| RunError::new("unavailable", "couldn't read the settings"))
    };
    let value = match call {
        Call::Start { request } => to_value(runs.start(&config()?, &request)?),
        Call::Status { run } => {
            run_id(&run)?;
            to_value(runs.status(&run)?)
        }
        Call::Output { run, offset, max } => {
            run_id(&run)?;
            to_value(runs.output(&run, offset, max)?)
        }
        Call::Finished { run } => {
            run_id(&run)?;
            to_value(runs.finished(&run)?)
        }
        Call::Stop { run } => {
            run_id(&run)?;
            to_value(runs.stop(&run)?)
        }
        Call::Prune => {
            runs.stop_outside(&config()?);
            serde_json::Value::Null
        }
        Call::List => to_value(runs.list()),
        Call::StopAll => {
            runs.stop_all();
            serde_json::Value::Null
        }
        Call::Printers => serde_json::json!({
            "printers": crate::printing::system_printers()?,
            "pages_left": crate::printing::pages_left(paths, &config()?),
        }),
        Call::Print { request } => {
            let _turn = crate::printing::one_job_at_a_time();
            to_value(crate::printing::print(paths, &config()?, &request)?)
        }
        Call::Prepare { ask } => {
            to_value(runs.preparations.start(runs, &config()?, paths, &ask)?)
        }
        Call::Prepared { ask } => to_value(runs.preparations.status(&ask)),
        Call::CodingLook { repository } => to_value(crate::coding::prepare::look(
            &config()?,
            paths,
            repository.as_deref(),
        )),
    };
    Ok(value)
}

fn to_value(value: impl Serialize) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}
