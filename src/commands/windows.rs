//! Commands on Windows: what a run gets there, and whether this computer can
//! sandbox one.
//!
//! A run is a Windows process container: MXC's ProcessContainer on the
//! BaseContainer that Windows 11 has from 24H2 with the August 2026 update.
//! Windows itself holds it to what the request grants, without changing the
//! permissions of anyone's files. MXC would fall back to an AppContainer that
//! does change them (it adds entries to the access lists of the folders it
//! grants, for the length of the run), so the request refuses that fallback,
//! and a computer without a BaseContainer runs no commands.
//!
//! A run reads what Windows lets every sandboxed app read (`C:\Windows`, and
//! `C:\Program Files` where an installer kept its permissions), the folders
//! on the system's `PATH` and the language toolchains installed for this
//! person, read-only; it writes only its folder and a home of its own.
//!
//! The command line runs in Windows PowerShell 5.1, which every Windows has.
//! The daemon writes it to `command.ps1` in the run's home, between a few
//! lines that make its output UTF-8 and its exit code the last command's,
//! and PowerShell reads it from there: a script block from a file's text,
//! so neither the command line's quoting nor the execution policy changes
//! it. PowerShell, Git and most programs load the Windows UI subsystem to
//! start, so a run may use it (`ui.disable: false`), held to itself: no
//! clipboard, no other program's windows, no synthetic input, no desktops,
//! no system settings, no input methods. They need a desktop session too,
//! so a daemon in session 0 (a service, an SSH session) runs no commands.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::plan::Plan;

/// What a run needs to know about this Windows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Host {
    /// `C:\Windows`.
    pub system_root: PathBuf,
    /// The person's profile folder, `C:\Users\<name>`.
    pub profile: Option<PathBuf>,
    /// Their `%LOCALAPPDATA%` and `%APPDATA%`.
    pub local_app_data: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    /// Where everyone's profile folders are, `C:\Users`.
    pub profiles: Option<PathBuf>,
    /// The folders on the system's `PATH` (the machine's, never the
    /// person's own), in order.
    pub machine_path: Vec<PathBuf>,
    /// The system's variables programs need to start, as the daemon has
    /// them: `ProgramFiles`, `PATHEXT`, `NUMBER_OF_PROCESSORS`...
    pub system_env: Vec<(String, String)>,
}

/// The system's variables a run gets from the daemon's environment. Never
/// the person's own: those can hold tokens.
const SYSTEM_VARIABLES: &[&str] = &[
    "ALLUSERSPROFILE",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
    "PROCESSOR_LEVEL",
    "PROCESSOR_REVISION",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "PUBLIC",
];

/// Where a toolchain installed for the person lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Base {
    /// `%USERPROFILE%`.
    Profile,
    /// `%LOCALAPPDATA%`.
    Local,
    /// `%APPDATA%`.
    Roaming,
}

/// A language toolchain installed for the person: the folder a run reads,
/// where its programs are inside it, and what tells them where they live.
struct Toolchain {
    base: Base,
    read: &'static str,
    bins: &'static [&'static str],
    env: Option<&'static str>,
}

/// Toolchains in the person's folders, used when they exist. Only what runs
/// programs: never the rest of the profile, and only the programs of
/// Cargo's home, which holds registry credentials. Toolchains installed for
/// everyone are on the system's `PATH`, which a run reads too.
const TOOLCHAINS: &[Toolchain] = &[
    Toolchain {
        base: Base::Profile,
        read: ".rustup",
        bins: &[],
        env: Some("RUSTUP_HOME"),
    },
    Toolchain {
        base: Base::Profile,
        read: ".cargo\\bin",
        bins: &[""],
        env: None,
    },
    Toolchain {
        base: Base::Local,
        read: "mise",
        bins: &["shims"],
        env: Some("MISE_DATA_DIR"),
    },
    Toolchain {
        base: Base::Profile,
        read: ".config\\mise",
        bins: &[],
        env: Some("MISE_CONFIG_DIR"),
    },
    // Python from python.org for this person: a folder per version, each
    // with its scripts. Their user installs (`pip install --user`) too.
    Toolchain {
        base: Base::Local,
        read: "Programs\\Python",
        bins: &["*", "*\\Scripts"],
        env: None,
    },
    Toolchain {
        base: Base::Roaming,
        read: "Python",
        bins: &["*\\Scripts"],
        env: None,
    },
    // Git for Windows installed for this person only.
    Toolchain {
        base: Base::Local,
        read: "Programs\\Git",
        bins: &["cmd"],
        env: None,
    },
    // npm's global programs, pnpm, Volta, and nvm-windows' Node.js versions
    // (its `nodejs` link on the system's `PATH` leads into them).
    Toolchain {
        base: Base::Roaming,
        read: "npm",
        bins: &[""],
        env: None,
    },
    Toolchain {
        base: Base::Local,
        read: "pnpm",
        bins: &[""],
        env: Some("PNPM_HOME"),
    },
    Toolchain {
        base: Base::Local,
        read: "Volta",
        bins: &["bin"],
        env: Some("VOLTA_HOME"),
    },
    Toolchain {
        base: Base::Roaming,
        read: "nvm",
        bins: &[],
        env: None,
    },
    Toolchain {
        base: Base::Profile,
        read: ".bun\\bin",
        bins: &[""],
        env: None,
    },
    Toolchain {
        base: Base::Profile,
        read: ".deno\\bin",
        bins: &[""],
        env: None,
    },
    Toolchain {
        base: Base::Profile,
        read: ".dotnet\\tools",
        bins: &[""],
        env: None,
    },
    // Scoop's shims and the programs they start, and WinGet's links for
    // this person and the packages they lead to.
    Toolchain {
        base: Base::Profile,
        read: "scoop\\shims",
        bins: &[""],
        env: None,
    },
    Toolchain {
        base: Base::Profile,
        read: "scoop\\apps",
        bins: &[],
        env: None,
    },
    Toolchain {
        base: Base::Local,
        read: "Microsoft\\WinGet\\Links",
        bins: &[""],
        env: None,
    },
    Toolchain {
        base: Base::Local,
        read: "Microsoft\\WinGet\\Packages",
        bins: &[],
        env: None,
    },
];

/// The paths, `PATH` entries and variables the toolchains that exist here
/// add.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub read: Vec<PathBuf>,
    pub path: Vec<PathBuf>,
    pub env: Vec<(String, String)>,
}

/// `%SystemRoot%`, `C:\Windows` unless it says otherwise.
pub fn system_root() -> PathBuf {
    ["SystemRoot", "windir"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

impl Host {
    /// This Windows, as the daemon sees it.
    pub fn current() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let system_root = system_root();
        let profile = var("USERPROFILE");
        let profiles = var("PUBLIC")
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(|| {
                profile
                    .as_ref()
                    .and_then(|p| p.parent().map(Path::to_path_buf))
            });
        let system_env = SYSTEM_VARIABLES
            .iter()
            .filter_map(|name| {
                let value = std::env::var(name).ok()?;
                (!value.is_empty()).then(|| (name.to_string(), value))
            })
            .collect();
        Self {
            system_root,
            local_app_data: var("LOCALAPPDATA"),
            app_data: var("APPDATA"),
            profile,
            profiles,
            machine_path: machine_path(),
            system_env,
        }
    }

    fn base(&self, base: Base) -> Option<&Path> {
        match base {
            Base::Profile => self.profile.as_deref(),
            Base::Local => self.local_app_data.as_deref(),
            Base::Roaming => self.app_data.as_deref(),
        }
    }

    fn system32(&self) -> PathBuf {
        within(&self.system_root, "System32")
    }

    /// Windows PowerShell 5.1, which every Windows has.
    pub fn powershell(&self) -> PathBuf {
        within(&self.system32(), r"WindowsPowerShell\v1.0\powershell.exe")
    }

    /// The toolchains installed for the person that exist here.
    pub fn toolchains(&self) -> Found {
        let mut found = Found::default();
        for toolchain in TOOLCHAINS {
            let Some(base) = self.base(toolchain.base) else {
                continue;
            };
            let dir = base.join(toolchain.read);
            if !dir.is_dir() {
                continue;
            }
            found.read.push(dir.clone());
            for bin in toolchain.bins {
                found
                    .path
                    .extend(expand(&dir, bin).into_iter().filter(|p| p.is_dir()));
            }
            if let Some(var) = toolchain.env {
                found.env.push((var.to_string(), display(&dir)));
            }
        }
        found
    }

    /// The folders of the system's `PATH` a run may use: folders that
    /// exist, never a whole drive, anyone's profile, the Store's app
    /// aliases (packaged apps can't start in a container) or a folder that
    /// holds `folder`, the run's own.
    pub fn system_path(&self, folder: &Path) -> Vec<PathBuf> {
        let mut path: Vec<PathBuf> = Vec::new();
        for dir in &self.machine_path {
            let whole_drive = dir.parent().is_none();
            let in_profiles = self
                .profiles
                .as_ref()
                .is_some_and(|profiles| starts_with(dir, profiles));
            let aliases = dir
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("WindowsApps"));
            if whole_drive
                || in_profiles
                || aliases
                || starts_with(folder, dir)
                || !dir.is_dir()
                || path.iter().any(|seen| same(seen, dir))
            {
                continue;
            }
            path.push(dir.clone());
        }
        path
    }
}

/// `dir` joined with `bin`, where a `*` stands for each folder in it.
fn expand(dir: &Path, bin: &str) -> Vec<PathBuf> {
    match bin.split_once('*') {
        None if bin.is_empty() => vec![dir.to_path_buf()],
        None => vec![dir.join(bin)],
        Some((_, rest)) => {
            let rest = rest.trim_start_matches('\\');
            let Ok(entries) = std::fs::read_dir(dir) else {
                return Vec::new();
            };
            let mut found: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| {
                    if rest.is_empty() {
                        e.path()
                    } else {
                        e.path().join(rest)
                    }
                })
                .collect();
            // Newest version first: `Python313` before `Python312`.
            found.sort();
            found.reverse();
            found
        }
    }
}

/// `rest` inside `base`, with Windows' separator wherever this is built, so
/// a request reads the same in the tests on every platform.
fn within(base: &Path, rest: &str) -> PathBuf {
    PathBuf::from(format!("{}\\{rest}", display(base).trim_end_matches('\\')))
}

/// Whether `path` is `base` or inside it, as Windows compares names.
fn starts_with(path: &Path, base: &Path) -> bool {
    let path = display(path).to_lowercase().replace('/', "\\");
    let base = display(base).to_lowercase().replace('/', "\\");
    let base = base.trim_end_matches('\\');
    path == base || path.starts_with(&format!("{base}\\"))
}

fn same(a: &Path, b: &Path) -> bool {
    starts_with(a, b) && starts_with(b, a)
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The lines around the command: output in UTF-8, nothing waiting on a
/// progress bar, and the exit code of the last command (PowerShell itself
/// says only 0 or 1).
const PROLOGUE: &str = "$ErrorActionPreference = 'Continue'\r\n\
$ProgressPreference = 'SilentlyContinue'\r\n\
$__cwwUtf8 = [System.Text.UTF8Encoding]::new($false)\r\n\
[Console]::OutputEncoding = $__cwwUtf8\r\n\
$OutputEncoding = $__cwwUtf8\r\n\
$global:LASTEXITCODE = 0\r\n";

const EPILOGUE: &str = "$__cwwOk = $?\r\n\
if ($__cwwOk) { exit 0 }\r\n\
if ($global:LASTEXITCODE -is [int] -and $global:LASTEXITCODE -ne 0) { exit $global:LASTEXITCODE }\r\n\
exit 1\r\n";

/// The script a run's PowerShell reads: the command between the prologue
/// and the epilogue. The blank line ends a command whose last line goes on
/// (a trailing backtick) before the epilogue starts.
pub fn script(command: &str) -> String {
    format!("{PROLOGUE}{command}\r\n\r\n{EPILOGUE}")
}

/// Where a run's Windows PowerShell looks for modules: its own first, which
/// hold its everyday commands, then those installed for everyone.
fn module_path(host: &Host) -> String {
    let program_files = host
        .system_env
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("ProgramFiles"))
        .map_or_else(
            || PathBuf::from(r"C:\Program Files"),
            |(_, value)| PathBuf::from(value),
        );
    [
        within(&host.system32(), r"WindowsPowerShell\v1.0\Modules"),
        within(&program_files, r"WindowsPowerShell\Modules"),
    ]
    .iter()
    .map(|p| display(p))
    .collect::<Vec<_>>()
    .join(";")
}

/// The command line that runs `script` in Windows PowerShell 5.1.
pub fn command_line(host: &Host, script: &Path) -> String {
    let script = display(script).replace('\'', "''");
    format!(
        "\"{}\" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command \
         \"& ([ScriptBlock]::Create([IO.File]::ReadAllText('{script}')))\"",
        display(&host.powershell())
    )
}

impl Plan {
    /// The script this run's PowerShell reads, in its home.
    pub fn script_path(&self) -> PathBuf {
        self.home.join("command.ps1")
    }

    /// The environment a run gets on Windows: the system's variables,
    /// nothing of the daemon's or the person's own, a home and temporary
    /// folder of its own, and where the toolchains are.
    pub fn windows_environment(&self, host: &Host) -> Vec<(String, String)> {
        let found = host.toolchains();
        let own = display(&self.home);
        let mut path: Vec<PathBuf> = found.path.clone();
        path.extend(host.system_path(&self.folder));
        let system32 = host.system32();
        path.extend([
            system32.clone(),
            host.system_root.clone(),
            within(&system32, "Wbem"),
            within(&system32, r"WindowsPowerShell\v1.0"),
        ]);
        let mut unique: Vec<PathBuf> = Vec::new();
        for dir in path {
            if !unique.iter().any(|seen| same(seen, &dir)) {
                unique.push(dir);
            }
        }
        let path: Vec<String> = unique.iter().map(|p| display(p)).collect();
        let root = display(&host.system_root);
        let drive = root.get(..2).unwrap_or("C:").to_string();
        let (home_drive, home_path) = own.split_at(own.find('\\').unwrap_or(own.len()));
        let mut env: Vec<(String, String)> = host
            .system_env
            .iter()
            .filter(|(name, _)| !name.eq_ignore_ascii_case("PATHEXT"))
            .cloned()
            .collect();
        let pathext = host
            .system_env
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("PATHEXT"))
            .map_or_else(
                || ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC".to_string(),
                |(_, value)| value.clone(),
            );
        env.extend([
            ("PATH".to_string(), path.join(";")),
            ("PATHEXT".to_string(), pathext),
            ("SystemRoot".to_string(), root.clone()),
            ("windir".to_string(), root),
            ("SystemDrive".to_string(), drive),
            (
                "ComSpec".to_string(),
                display(&within(&system32, "cmd.exe")),
            ),
            ("USERPROFILE".to_string(), own.clone()),
            ("HOME".to_string(), own.clone()),
            ("HOMEDRIVE".to_string(), home_drive.to_string()),
            ("HOMEPATH".to_string(), home_path.to_string()),
            ("APPDATA".to_string(), format!("{own}\\AppData\\Roaming")),
            ("LOCALAPPDATA".to_string(), format!("{own}\\AppData\\Local")),
            ("TEMP".to_string(), format!("{own}\\tmp")),
            ("TMP".to_string(), format!("{own}\\tmp")),
            // Where Windows PowerShell finds most of its commands, which
            // live in modules (Write-Output is in one), and where it keeps
            // what it learned of them: in the run's home, like everything
            // else a run writes. Left to itself, a run on Windows on Arm
            // found none of them.
            ("PSModulePath".to_string(), module_path(host)),
            (
                "PSModuleAnalysisCachePath".to_string(),
                format!("{own}\\AppData\\Local\\ModuleAnalysisCache"),
            ),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("TERM".to_string(), "dumb".to_string()),
            // Python writes UTF-8 to a pipe, not the system's code page.
            ("PYTHONUTF8".to_string(), "1".to_string()),
            // npm and other tools ask for news of themselves on the network
            // before they exit, and without one they wait minutes for it.
            ("NO_UPDATE_NOTIFIER".to_string(), "1".to_string()),
            (
                "npm_config_update_notifier".to_string(),
                "false".to_string(),
            ),
            // mise reads a project's own `mise.toml` only once trusted.
            (
                "MISE_TRUSTED_CONFIG_PATHS".to_string(),
                display(&self.folder),
            ),
            (
                "MISE_CACHE_DIR".to_string(),
                format!("{own}\\AppData\\Local\\mise-cache"),
            ),
            (
                "MISE_STATE_DIR".to_string(),
                format!("{own}\\AppData\\Local\\mise-state"),
            ),
        ]);
        env.extend(found.env);
        env
    }

    /// The folders a run reads on Windows, besides what every sandboxed app
    /// reads: the toolchains, and the system's `PATH` outside `C:\Windows`.
    pub fn windows_readable(&self, host: &Host) -> Vec<String> {
        let mut read = host.toolchains().read;
        read.extend(
            host.system_path(&self.folder)
                .into_iter()
                .filter(|dir| !starts_with(dir, &host.system_root)),
        );
        let mut unique: Vec<PathBuf> = Vec::new();
        for dir in read {
            if starts_with(&dir, &self.folder) || starts_with(&dir, &self.home) {
                continue;
            }
            if !unique.iter().any(|seen| starts_with(&dir, seen)) {
                unique.retain(|seen| !starts_with(seen, &dir));
                unique.push(dir);
            }
        }
        unique.iter().map(|p| display(p)).collect()
    }

    /// The MXC request for this run in a Windows process container.
    pub fn windows_request(&self, host: &Host) -> Value {
        let env: Vec<String> = self
            .windows_environment(host)
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let mut resources = json!({
            "cpuTimeMs": self.limits.cpu_seconds.saturating_mul(1000),
            "priority": "low",
        });
        if self.limits.memory_mb > 0 {
            resources["memoryMb"] = self.limits.memory_mb.into();
        }
        if self.limits.processes > 0 {
            resources["maxProcesses"] = self.limits.processes.into();
        }
        json!({
            "version": "1.1.0-alpha",
            "containerId": format!("cww-run-{}", self.id),
            "containment": "processcontainer",
            "lifecycle": { "destroyOnExit": true, "preservePolicy": false },
            // Never the AppContainer that changes the permissions of the
            // folders it grants: no BaseContainer, no run.
            "fallback": { "allowDaclMutation": false },
            "process": {
                "commandLine": command_line(host, &self.script_path()),
                "timeout": 0,
                "cwd": display(&self.cwd),
                "env": env,
            },
            "filesystem": {
                "readwritePaths": [display(&self.folder), display(&self.home)],
                "readonlyPaths": self.windows_readable(host),
                "deniedPaths": [],
            },
            "network": { "egress": { "default": if self.network { "allow" } else { "deny" } } },
            "resources": resources,
            // The UI subsystem, which PowerShell needs to start, held to
            // the run's own windows.
            "ui": { "disable": false, "clipboard": "none", "injection": false },
            "processContainer": {
                "leastPrivilege": false,
                "capabilities": [],
                "ui": {
                    "isolation": "container",
                    "desktopSystemControl": false,
                    "systemSettings": "none",
                    "ime": false,
                },
            },
        })
    }
}

/// What sandboxed apps may do on a drive's root once it's prepared: read its
/// attributes, extended attributes and permissions, never list or read
/// what's in it (`FILE_READ_ATTRIBUTES | FILE_READ_EA | READ_CONTROL |
/// SYNCHRONIZE`). The same entries as MXC's `wxc-host-prep
/// prepare-system-drive`, which `icacls` writes as `(RA,REA,RC,S)`.
///
/// Windows gives sandboxed apps nothing on a drive's root, and some programs
/// look at it before anything else: Node.js finds where a script really is
/// from the root down, so `node script.js` and npm fail with `EPERM: lstat
/// 'C:\'`, and `cmd`'s `dir` and `> NUL` fail too. A process container can
/// be given a folder only with everything in it, so the only way to let
/// them see the root alone is these two entries, which an administrator
/// adds once (`cww roots prepare-commands`, or the app's folder settings).
pub const DRIVE_ROOT_MASK: u32 = 0x0012_0088;

/// ALL APPLICATION PACKAGES and ALL RESTRICTED APPLICATION PACKAGES, the
/// groups every sandboxed app on Windows belongs to.
pub const SANDBOXED_APPS: [&str; 2] = ["S-1-15-2-1", "S-1-15-2-2"];

/// The drive roots commands here may need: the system drive's (where
/// `system_root` is, and the programs), and those of `folders`, in order,
/// each once.
pub fn drive_roots(system_root: &Path, folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for path in std::iter::once(system_root).chain(folders.iter().map(PathBuf::as_path)) {
        let text = display(path);
        let bytes = text.as_bytes();
        if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
            continue;
        }
        let root = PathBuf::from(format!("{}:\\", (bytes[0] as char).to_ascii_uppercase()));
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// Whether sandboxed apps may read the attributes of the drive whose root
/// is `root` ([`DRIVE_ROOT_MASK`]). Reading a drive's permissions needs no
/// administrator.
#[cfg(windows)]
pub fn drive_prepared(root: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, READ_CONTROL};
    const INHERIT_ONLY_ACE: u8 = 0x08;
    let Ok(file) = std::fs::OpenOptions::new()
        .access_mode(READ_CONTROL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(root)
    else {
        return false;
    };
    let Ok(security) = crate::win::file_security(&file) else {
        return false;
    };
    // A null DACL lets everyone do everything.
    let Some(grants) = security.grants else {
        return true;
    };
    SANDBOXED_APPS.iter().all(|sid| {
        grants.iter().any(|grant| {
            grant.sid == *sid
                && grant.flags & INHERIT_ONLY_ACE == 0
                && grant.mask & DRIVE_ROOT_MASK == DRIVE_ROOT_MASK
        })
    })
}

/// How people read `roots`: "drive C:", "drives C: and D:".
pub fn drive_names(roots: &[PathBuf]) -> String {
    let names: Vec<String> = roots
        .iter()
        .map(|root| display(root).trim_end_matches('\\').to_string())
        .collect();
    match names.as_slice() {
        [one] => format!("drive {one}"),
        [rest @ .., last] => format!("drives {} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// The `icacls` arguments that prepare `root`.
pub fn prepare_arguments(root: &Path) -> String {
    let grants: Vec<String> = SANDBOXED_APPS
        .iter()
        .map(|sid| format!("*{sid}:(RA,REA,RC,S)"))
        .collect();
    // A drive root isn't quoted: `"C:\"` would end in an escaped quote.
    format!("{} /grant {}", display(root), grants.join(" "))
}

/// Let sandboxed apps read the attributes of each of `roots` that doesn't
/// let them yet, with `icacls` as an administrator: Windows asks for one,
/// over `owner` (a window) if given. Returns the drives prepared.
#[cfg(windows)]
pub fn prepare_drives(roots: &[PathBuf], owner: Option<isize>) -> Result<Vec<PathBuf>, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    let host = Host::current();
    let icacls = within(&host.system32(), "icacls.exe");
    let mut prepared = Vec::new();
    for root in roots.iter().filter(|root| !drive_prepared(root)) {
        let verb = crate::win::wide("runas");
        let file = crate::win::wide(&display(&icacls));
        let parameters = crate::win::wide(&prepare_arguments(root));
        // SAFETY: zeroed is a valid SHELLEXECUTEINFOW (null pointers and
        // handles); the strings outlive the call.
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.hwnd = owner.map_or(std::ptr::null_mut(), |hwnd| hwnd as _);
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = parameters.as_ptr();
        // SW_HIDE: icacls is a console program with nothing to show.
        info.nShow = 0;
        // SAFETY: `info` is initialized as the function requires.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let error = unsafe { GetLastError() };
            return Err(if error == ERROR_CANCELLED {
                "Windows wasn't allowed to change it: an administrator has to say yes".to_string()
            } else {
                format!("couldn't start icacls as an administrator (error {error})")
            });
        }
        let mut code = 1u32;
        if !info.hProcess.is_null() {
            // SAFETY: the process handle SEE_MASK_NOCLOSEPROCESS gave us.
            unsafe {
                WaitForSingleObject(info.hProcess, INFINITE);
                GetExitCodeProcess(info.hProcess, &mut code);
                CloseHandle(info.hProcess);
            }
        }
        if code != 0 || !drive_prepared(root) {
            return Err(format!(
                "icacls couldn't change {} (exit {code})",
                display(root)
            ));
        }
        prepared.push(root.clone());
    }
    Ok(prepared)
}

/// The folders on the machine's `PATH`, from the registry, with their
/// variables expanded.
#[cfg(windows)]
fn machine_path() -> Vec<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let key = crate::win::wide(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment");
    let name = crate::win::wide("Path");
    let mut size: u32 = 0;
    // SAFETY: NUL-terminated names; a null buffer asks only for the size.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != 0 || size == 0 {
        return Vec::new();
    }
    // Expanding can need more room than the size asked first.
    let mut buffer: Vec<u16> = vec![0; (size as usize / 2) + 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: the buffer holds `size` bytes, and outlives the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return Vec::new();
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    split_path(&String::from_utf16_lossy(&buffer[..len]))
}

#[cfg(not(windows))]
fn machine_path() -> Vec<PathBuf> {
    Vec::new()
}

/// A `PATH` value's folders: absolute ones, without quotes or a trailing
/// separator.
pub fn split_path(value: &str) -> Vec<PathBuf> {
    value
        .split(';')
        .map(|part| part.trim().trim_matches('"').trim())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let trimmed = part.trim_end_matches('\\');
            // `C:` alone means the drive's current folder: keep the root.
            if trimmed.len() == 2 && trimmed.ends_with(':') {
                PathBuf::from(format!("{trimmed}\\"))
            } else {
                PathBuf::from(trimmed)
            }
        })
        .filter(|path| {
            let text = display(path);
            text.len() >= 3 && text.as_bytes()[1] == b':' && text.as_bytes()[2] == b'\\'
                || text.starts_with("\\\\")
        })
        .collect()
}

/// Whether this computer can sandbox commands: Windows with a
/// BaseContainer, and a daemon in a desktop session.
#[cfg(windows)]
pub fn sandbox() -> Result<super::Sandbox, super::RunError> {
    supported()?;
    if !in_desktop_session() {
        return Err(super::RunError::new(
            "unavailable",
            "commands run only while Chat with Work Desktop runs in your desktop session: \
             Windows PowerShell can't start in a sandbox from a service or an SSH session",
        ));
    }
    Ok(super::Sandbox::ProcessContainer)
}

/// Whether this Windows has the process containers commands run in,
/// worked out once.
#[cfg(windows)]
pub fn supported() -> Result<(), super::RunError> {
    static ANSWER: std::sync::OnceLock<Result<(), super::RunError>> = std::sync::OnceLock::new();
    ANSWER
        .get_or_init(|| match mxc_sdk::v1::probe(None) {
            Ok(probe) if probe.tier == Some("base-container") => Ok(()),
            Ok(probe) => {
                tracing::info!(tier = probe.tier, "no BaseContainer for commands");
                Err(super::RunError::new(
                    "unavailable",
                    "commands need Windows 11 24H2 or later, updated since August 2026, whose \
                     sandbox doesn't change the permissions of your files",
                ))
            }
            Err(e) => Err(super::RunError::new(
                "unavailable",
                format!("commands can't run on this computer: {}", e.message),
            )),
        })
        .clone()
}

/// Whether this process runs in a desktop session, not session 0 (services,
/// and SSH sessions), whose window station sandboxed programs can't use.
#[cfg(windows)]
fn in_desktop_session() -> bool {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    let mut session: u32 = 0;
    // SAFETY: a valid out pointer for the current process's ID.
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    ok != 0 && session != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Windows whose profiles are in `base\Users`, the person's `me`.
    fn host(base: &Path) -> Host {
        let profile = base.join("Users").join("me");
        Host {
            system_root: PathBuf::from(r"C:\Windows"),
            local_app_data: Some(profile.join("AppData").join("Local")),
            app_data: Some(profile.join("AppData").join("Roaming")),
            profile: Some(profile),
            profiles: Some(base.join("Users")),
            machine_path: Vec::new(),
            system_env: vec![
                ("ProgramFiles".into(), r"C:\Program Files".into()),
                ("PATHEXT".into(), ".COM;.EXE;.BAT;.CMD".into()),
            ],
        }
    }

    #[test]
    fn a_path_value_keeps_its_absolute_folders() {
        assert_eq!(
            split_path(
                r#"C:\Windows\system32;;"C:\Program Files\Git\cmd";relative;C:\tools\;D:;\\server\share\bin"#
            ),
            [
                PathBuf::from(r"C:\Windows\system32"),
                PathBuf::from(r"C:\Program Files\Git\cmd"),
                PathBuf::from(r"C:\tools"),
                PathBuf::from(r"D:\"),
                PathBuf::from(r"\\server\share\bin"),
            ]
        );
    }

    #[test]
    fn names_compare_as_windows_compares_them() {
        assert!(starts_with(
            Path::new(r"C:\Users\Ann\proj"),
            Path::new(r"c:\users\ann")
        ));
        assert!(starts_with(
            Path::new(r"C:\Users\Ann"),
            Path::new(r"C:\Users\Ann\")
        ));
        assert!(!starts_with(
            Path::new(r"C:\Users\Anna"),
            Path::new(r"C:\Users\Ann")
        ));
    }

    #[test]
    fn the_script_keeps_the_last_commands_exit_code_and_writes_utf8() {
        let script = script("npm test");
        assert!(script.starts_with("$ErrorActionPreference = 'Continue'"));
        assert!(script.contains("[Console]::OutputEncoding = $__cwwUtf8"));
        let command = script.find("npm test").unwrap();
        let epilogue = script.find("$__cwwOk = $?").unwrap();
        assert!(command < epilogue);
        assert!(script[command..epilogue].contains("\r\n\r\n"));
        assert!(script.contains("exit $global:LASTEXITCODE"));
    }

    fn plan(folder: &Path, home: &Path, network: bool) -> Plan {
        Plan {
            id: "0123456789ab".into(),
            command: "npm test".into(),
            folder: folder.to_path_buf(),
            cwd: folder.join("src"),
            network,
            // As Windows gets them; a Mac's leave out memory and processes.
            limits: super::super::Limits {
                cpu_seconds: 600,
                memory_mb: 4096,
                timeout_seconds: 1800,
                processes: 256,
                output_chars: 20_000,
            },
            home: home.to_path_buf(),
        }
    }

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    /// The request a Windows run gets: a process container that never falls
    /// back to changing permissions, its folder and home to write, the
    /// system's PATH and the person's toolchains to read, no network, the
    /// UI subsystem held to itself, and limits on CPU, memory and processes.
    #[test]
    fn a_windows_run_writes_only_its_folder_and_reads_its_toolchains() {
        let base = tempfile::tempdir().unwrap();
        let mut host = host(base.path());
        let profile = host.profile.clone().unwrap();
        std::fs::create_dir_all(profile.join(".cargo\\bin")).unwrap();
        std::fs::create_dir_all(profile.join(".cargo\\registry")).unwrap();
        let tools = base.path().join("tools");
        let in_profiles = base.path().join("Users").join("other").join("bin");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::create_dir_all(&in_profiles).unwrap();
        let folder = base.path().join("work").join("app");
        std::fs::create_dir_all(folder.join("src")).unwrap();
        host.machine_path = vec![
            tools.clone(),
            in_profiles.clone(),
            base.path().join("work"),
            base.path().join("missing"),
        ];
        let home = base.path().join("runs").join("0123456789ab");
        let json = plan(&folder, &home, false).windows_request(&host);

        assert_eq!(json["containment"], "processcontainer");
        assert_eq!(json["fallback"]["allowDaclMutation"], false);
        assert_eq!(json["network"]["egress"]["default"], "deny");
        assert_eq!(json["ui"]["disable"], false);
        assert_eq!(json["ui"]["clipboard"], "none");
        assert_eq!(json["ui"]["injection"], false);
        assert_eq!(json["processContainer"]["ui"]["isolation"], "container");
        assert_eq!(json["processContainer"]["ui"]["systemSettings"], "none");
        assert_eq!(json["processContainer"]["leastPrivilege"], false);
        assert_eq!(
            strings(&json["filesystem"]["readwritePaths"]),
            [display(&folder), display(&home)]
        );
        let readable = strings(&json["filesystem"]["readonlyPaths"]);
        assert!(readable.contains(&display(&tools)), "{readable:?}");
        assert!(
            readable.contains(&display(&profile.join(".cargo\\bin"))),
            "{readable:?}"
        );
        assert!(
            !readable.iter().any(|p| p.contains("registry")),
            "never the rest of Cargo's home: {readable:?}"
        );
        assert!(
            !readable.contains(&display(&in_profiles)),
            "nothing in anyone's profile from the system's PATH: {readable:?}"
        );
        assert!(
            !readable.contains(&display(&base.path().join("work"))),
            "never a folder that holds the run's own: {readable:?}"
        );
        assert!(strings(&json["filesystem"]["deniedPaths"]).is_empty());
        assert_eq!(json["resources"]["cpuTimeMs"], 600_000);
        assert_eq!(json["resources"]["memoryMb"], 4096);
        assert_eq!(json["resources"]["maxProcesses"], 256);
        assert_eq!(json["resources"]["priority"], "low");
        assert_eq!(json["process"]["cwd"], display(&folder.join("src")));
        let line = json["process"]["commandLine"].as_str().unwrap();
        assert!(line.contains("powershell.exe"), "{line}");
        assert!(line.contains(&display(&home.join("command.ps1"))), "{line}");

        let env = strings(&json["process"]["env"]);
        let var = |name: &str| {
            env.iter()
                .find_map(|e| e.strip_prefix(&format!("{name}=")))
                .map(String::from)
        };
        assert_eq!(var("SystemRoot").as_deref(), Some(r"C:\Windows"));
        assert_eq!(
            var("ComSpec").as_deref(),
            Some(r"C:\Windows\System32\cmd.exe")
        );
        assert_eq!(var("USERPROFILE"), Some(display(&home)));
        assert_eq!(var("HOME"), Some(display(&home)));
        assert_eq!(var("TEMP"), Some(format!("{}\\tmp", display(&home))));
        assert_eq!(
            var("LOCALAPPDATA"),
            Some(format!("{}\\AppData\\Local", display(&home)))
        );
        assert_eq!(var("ProgramFiles").as_deref(), Some(r"C:\Program Files"));
        assert_eq!(var("PATHEXT").as_deref(), Some(".COM;.EXE;.BAT;.CMD"));
        assert_eq!(var("npm_config_update_notifier").as_deref(), Some("false"));
        assert!(
            var("PSModuleAnalysisCachePath")
                .unwrap()
                .starts_with(&var("LOCALAPPDATA").unwrap())
        );
        assert_eq!(
            var("PSModulePath").as_deref(),
            Some(
                r"C:\Windows\System32\WindowsPowerShell\v1.0\Modules;C:\Program Files\WindowsPowerShell\Modules"
            )
        );
        let path = var("PATH").unwrap();
        let path: Vec<&str> = path.split(';').collect();
        let at = |dir: &str| path.iter().position(|p| *p == dir);
        let cargo = display(&profile.join(".cargo\\bin"));
        assert!(at(&cargo) < at(&display(&tools)), "{path:?}");
        assert!(
            at(&display(&tools)) < at(r"C:\Windows\System32"),
            "{path:?}"
        );
        assert!(at(r"C:\Windows\System32\WindowsPowerShell\v1.0").is_some());
        assert!(at(&display(&in_profiles)).is_none(), "{path:?}");
        assert!(
            env.iter().all(|e| !e.starts_with("CWW_")),
            "nothing of the daemon's: {env:?}"
        );

        let online = plan(&folder, &home, true).windows_request(&host);
        assert_eq!(online["network"]["egress"]["default"], "allow");
    }

    #[test]
    fn drives_are_the_systems_and_the_folders_each_once() {
        assert_eq!(
            drive_roots(
                Path::new(r"C:\Windows"),
                &[
                    PathBuf::from(r"d:\work\app"),
                    PathBuf::from(r"C:\Users\me\proj"),
                    PathBuf::from(r"D:\other"),
                    PathBuf::from(r"\\server\share\x"),
                ]
            ),
            [PathBuf::from(r"C:\"), PathBuf::from(r"D:\")]
        );
        assert_eq!(drive_names(&[PathBuf::from(r"C:\")]), "drive C:");
        assert_eq!(
            drive_names(&[
                PathBuf::from(r"C:\"),
                PathBuf::from(r"D:\"),
                PathBuf::from(r"E:\")
            ]),
            "drives C:, D: and E:"
        );
        assert_eq!(
            prepare_arguments(Path::new(r"C:\")),
            r"C:\ /grant *S-1-15-2-1:(RA,REA,RC,S) *S-1-15-2-2:(RA,REA,RC,S)"
        );
    }

    #[test]
    fn powershell_reads_the_script_whatever_its_path_holds() {
        let host = host(Path::new(r"C:\Users"));
        let line = command_line(
            &host,
            Path::new(r"C:\Users\O'Brien\AppData\Local\cww\data\runs\0123456789ab\command.ps1"),
        );
        assert!(line.starts_with(r#""C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive"#), "{line}");
        assert!(line.contains(r"ReadAllText('C:\Users\O''Brien\AppData\Local\cww\data\runs\0123456789ab\command.ps1')"), "{line}");
        assert!(!line.contains("-EncodedCommand"));
    }
}
