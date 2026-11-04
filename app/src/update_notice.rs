//! What a copy that can't replace itself says when a newer release is out.
//!
//! A package manager, a Flatpak, the AppImage (until a release signs it) or
//! the one-line installer updates these copies, never the app (update.rs).
//! They still hear about each release whose signed `checksums.txt`
//! verifies, on the same daily check: General shows one notice per version
//! with how this copy was installed, the exact command that updates it there
//! (copied with a button, never run here), the file to download when there
//! is one, and the release notes. The tray offers the same notice until
//! it's dismissed. fastframe-update finds the package manager, the package
//! and its command (`Updater::managed_update`); this file words them, and
//! knows what only Chat with Work Desktop's releases are: the `.deb`, the
//! release's own `.rpm` and the Flatpak come as files, with no repository
//! to upgrade from.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use fastframe_update::{ManagedUpdate, Manager, Unsupported};
use serde::Serialize;

/// Where the release files are (update.rs passes its own).
#[cfg(test)]
const DOWNLOADS: &str = "https://github.com/crmne/chatwithwork-desktop/releases/download";

/// The README's one-line installer, for a copy it put there.
pub const INSTALLER: &str = "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/crmne/chatwithwork-desktop/releases/latest/download/cww-installer.sh | sh";

/// What the notice says for this copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    /// How this copy was installed and how it updates, in a sentence or two.
    pub how: String,
    /// The command that updates it, when the computer tells which one.
    pub command: Option<String>,
    /// The file to download first, when the update needs one.
    pub download: Option<String>,
}

/// fastframe-update's answer, kept as the parts the notice reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Managed {
    pub manager: Manager,
    pub command: Option<String>,
}

impl From<ManagedUpdate> for Managed {
    fn from(update: ManagedUpdate) -> Self {
        Self {
            manager: update.manager,
            command: update.command,
        }
    }
}

/// How this copy was installed, as far as the computer tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// A package manager with a repository: pacman and the AUR, COPR,
    /// openSUSE's Build Service, Homebrew, Nix, Snap, cargo.
    Managed(Managed),
    /// The release's `.deb`: there's no APT repository to upgrade from.
    Deb,
    /// The release's own `.rpm`, installed from the file.
    Rpm,
    /// The release's Flatpak bundle: no remote publishes it yet.
    FlatpakBundle,
    /// The AppImage at this path, whose release doesn't sign it yet or
    /// which the user can't replace.
    AppImage(PathBuf),
    /// Linux, neither packaged nor marked portable: the one-line installer
    /// or the app archive.
    Installer,
    /// The Mac's `.pkg`, by its receipt.
    Pkg,
    /// Anything else, with fastframe-update's reason.
    Other(Unsupported),
}

/// What detection asks this computer beyond fastframe-update. Tests answer
/// with a fake.
pub trait Probe {
    fn var(&self, name: &str) -> Option<String>;
    /// The trimmed output of `program args`, when it ran and succeeded.
    fn run(&self, program: &str, args: &[&OsStr]) -> Option<String>;
}

/// The real computer.
pub struct System;

impl Probe for System {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }

    fn run(&self, program: &str, args: &[&OsStr]) -> Option<String> {
        use std::io::Read;
        let mut child = std::process::Command::new(program)
            .args(args)
            .env("LC_ALL", "C")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        // A package database that is locked gives no answer, not a stuck
        // update check.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().ok()? {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let mut output = String::new();
        child
            .stdout
            .take()?
            .take(4096)
            .read_to_string(&mut output)
            .ok()?;
        let output = output.trim().to_owned();
        (status.success() && !output.is_empty()).then_some(output)
    }
}

/// How the running copy was installed, given why it can't update itself
/// and fastframe-update's `managed_update`.
pub fn detect(
    reason: &Unsupported,
    pkg_receipt: bool,
    managed: Option<ManagedUpdate>,
) -> Installed {
    #[cfg(feature = "demo")]
    if let Some(installed) = demo() {
        return installed;
    }
    let executable = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .unwrap_or_default();
    installed(
        reason,
        &executable,
        pkg_receipt,
        managed.map(Managed::from),
        &System,
    )
}

/// [`detect`] for this executable on a given computer.
pub fn installed(
    reason: &Unsupported,
    executable: &Path,
    pkg_receipt: bool,
    managed: Option<Managed>,
    probe: &dyn Probe,
) -> Installed {
    if pkg_receipt {
        return Installed::Pkg;
    }
    // $APPDIR holding the executable says it runs from this AppImage, not
    // from a program one started.
    if let (Some(appimage), Some(appdir)) = (probe.var("APPIMAGE"), probe.var("APPDIR")) {
        let appdir = Path::new(&appdir);
        let appdir = appdir.canonicalize().unwrap_or_else(|_| appdir.to_owned());
        if Path::new(&appimage).is_absolute() && executable.starts_with(&appdir) {
            return Installed::AppImage(appimage.into());
        }
    }
    match managed {
        Some(Managed {
            manager: Manager::Apt,
            ..
        }) => Installed::Deb,
        Some(Managed {
            manager: Manager::Flatpak,
            ..
        }) => Installed::FlatpakBundle,
        Some(
            managed @ Managed {
                manager: Manager::Dnf | Manager::Zypper,
                ..
            },
        ) => {
            // COPR and the Build Service stamp their own vendor; the
            // release's .rpm says Plenty.
            let vendor = probe
                .run(
                    "rpm",
                    &[
                        "-qf".as_ref(),
                        "--qf".as_ref(),
                        "%{VENDOR}".as_ref(),
                        executable.as_os_str(),
                    ],
                )
                .unwrap_or_default()
                .to_lowercase();
            if vendor == "plenty" {
                Installed::Rpm
            } else if vendor.is_empty() {
                // rpm didn't say: not known to be a repository install.
                Installed::Other(reason.clone())
            } else {
                Installed::Managed(managed)
            }
        }
        Some(managed) => Installed::Managed(managed),
        None if matches!(reason, Unsupported::NotPortable) && cfg!(target_os = "linux") => {
            Installed::Installer
        }
        None => Installed::Other(reason.clone()),
    }
}

/// The notice for `installed` and the release `version` (already checked to
/// be plain `X.Y.Z`), on a processor `arch` (`std::env::consts::ARCH`),
/// with downloads going to `downloads`.
#[cfg(test)]
pub fn notice(
    installed: &Installed,
    version: &str,
    arch: &str,
    downloads: Option<&Path>,
) -> Notice {
    notice_from(
        installed,
        version,
        arch,
        downloads,
        &format!("{DOWNLOADS}/v{version}"),
    )
}

/// [`notice`] with the release's files at `files` (a local test feed's
/// address, say).
pub fn notice_from(
    installed: &Installed,
    version: &str,
    arch: &str,
    downloads: Option<&Path>,
    files: &str,
) -> Notice {
    let file = |name: String| format!("{files}/{name}");
    let saved = |name: &str| match downloads {
        Some(folder) => shell_path(&folder.join(name), crate::paths::home_dir().as_deref()),
        None => format!("./{name}"),
    };
    let deb_arch = if arch == "aarch64" { "arm64" } else { "amd64" };
    let (how, command, download): (&str, Option<String>, Option<String>) = match installed {
        Installed::Managed(managed) => {
            let how = match (managed.manager, managed.command.is_some()) {
                (Manager::Aur, _) => "Installed from the AUR. Update it with your AUR helper.",
                (Manager::Pacman, _) => "Installed with pacman. Update it with pacman.",
                (Manager::Dnf, _) => "Installed from COPR. Update it with dnf.",
                (Manager::Zypper, _) => {
                    "Installed from openSUSE's Build Service. Update it with zypper."
                }
                (Manager::Homebrew | Manager::HomebrewCask, _) => {
                    "Installed with Homebrew. Update it with brew."
                }
                (Manager::Nix, true) => "Installed with nix profile. Update it with Nix.",
                (Manager::Nix, false) => {
                    "Installed with Nix. Update it the way you installed it, such as nixos-rebuild or Home Manager."
                }
                (Manager::Snap, _) => "Installed with Snap. Update it with snap.",
                (Manager::Cargo, _) => "Built with cargo. Install the new version the same way.",
                _ => "Installed with a package manager. Update it there.",
            };
            (how, managed.command.clone(), None)
        }
        Installed::Deb => {
            let name = format!("chatwithwork-desktop_{version}_{deb_arch}.deb");
            (
                "Installed from a .deb package. Download the new one, then install it.",
                Some(format!("sudo apt install {}", saved(&name))),
                Some(file(name)),
            )
        }
        Installed::Rpm => (
            "Installed from an .rpm package. Install the new one with dnf.",
            Some(format!(
                "sudo dnf install {}",
                file(format!("chatwithwork-desktop-{version}-1.{arch}.rpm"))
            )),
            None,
        ),
        Installed::FlatpakBundle => {
            let name = format!("chatwithwork-desktop-{version}-{arch}.flatpak");
            (
                "Installed from a Flatpak bundle. Download the new one, then install it.",
                Some(format!("flatpak install --user {}", saved(&name))),
                Some(file(name)),
            )
        }
        Installed::AppImage(path) => (
            "An AppImage. Update it with AppImageUpdate or Gear Lever, or download the new one.",
            Some(format!(
                "appimageupdatetool {}",
                shell_path(path, crate::paths::home_dir().as_deref())
            )),
            Some(file(format!(
                "chatwithwork-desktop-{version}-{arch}.AppImage"
            ))),
        ),
        Installed::Installer => (
            "Installed with the one-line installer or the app archive. Run the installer again.",
            Some(INSTALLER.into()),
            None,
        ),
        Installed::Pkg => (
            "Installed with the .pkg installer. Download the new one and open it.",
            None,
            Some(file(format!("cww-v{version}-macos-universal.pkg"))),
        ),
        Installed::Other(Unsupported::SystemDirectory) => (
            "Installed system-wide. Update it with the package manager that installed it.",
            None,
            None,
        ),
        Installed::Other(reason) => {
            return Notice {
                how: crate::update::advice(reason),
                command: None,
                download: None,
            };
        }
    };
    Notice {
        how: how.into(),
        command,
        download,
    }
}

/// `path` as a shell reads it: `~/...` under the home folder when nothing
/// in it needs quoting, else single-quoted.
fn shell_path(path: &Path, home: Option<&Path>) -> String {
    let text = path.to_string_lossy();
    let plain = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.+@,:".contains(&b))
    };
    if let Some(rest) = home.and_then(|home| path.strip_prefix(home).ok())
        && let Some(rest) = rest.to_str()
        && plain(rest)
    {
        return format!("~/{rest}");
    }
    if plain(&text) {
        return text.into_owned();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// `CWW_DEMO_INSTALLED` (`aur`, `deb`, `copr`, `obs`, `rpm`, `homebrew`,
/// `nix`, `flatpak`, `appimage`, `installer`, `pkg`) stands in for
/// detection in --demo builds, for pictures of each notice.
#[cfg(feature = "demo")]
fn demo() -> Option<Installed> {
    let managed = |manager, command: &str| {
        Installed::Managed(Managed {
            manager,
            command: Some(command.into()),
        })
    };
    Some(match std::env::var("CWW_DEMO_INSTALLED").ok()?.as_str() {
        "aur" => managed(Manager::Aur, "yay -Syu chatwithwork-desktop-bin"),
        "deb" => Installed::Deb,
        "copr" => managed(
            Manager::Dnf,
            "sudo dnf upgrade --refresh chatwithwork-desktop",
        ),
        "obs" => managed(
            Manager::Zypper,
            "sudo zypper refresh && sudo zypper update chatwithwork-desktop",
        ),
        "rpm" => Installed::Rpm,
        "homebrew" => managed(Manager::Homebrew, "brew upgrade cww"),
        "nix" => managed(Manager::Nix, "nix profile upgrade chatwithwork-desktop"),
        "flatpak" => Installed::FlatpakBundle,
        "appimage" => Installed::AppImage(
            dirs::home_dir()?.join("Applications/chatwithwork-desktop-x86_64.AppImage"),
        ),
        "installer" => Installed::Installer,
        "pkg" => Installed::Pkg,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake {
        vars: HashMap<&'static str, String>,
        vendor: Option<String>,
    }

    impl Probe for Fake {
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }
        fn run(&self, program: &str, _: &[&OsStr]) -> Option<String> {
            (program == "rpm").then(|| self.vendor.clone()).flatten()
        }
    }

    fn managed(manager: Manager, command: &str) -> Option<Managed> {
        Some(Managed {
            manager,
            command: Some(command.into()),
        })
    }

    fn at(reason: Unsupported, managed: Option<Managed>, fake: &Fake) -> Installed {
        installed(&reason, Path::new("/usr/bin/cww-app"), false, managed, fake)
    }

    const PACKAGE: Unsupported =
        Unsupported::SystemPackage(fastframe_update::PackageManager::Pacman);

    #[test]
    fn fastframes_command_is_shown_for_repositories() {
        let fake = Fake::default();
        let aur = at(
            PACKAGE,
            managed(Manager::Aur, "yay -Syu chatwithwork-desktop-bin"),
            &fake,
        );
        let notice = notice(&aur, "0.12.0", "x86_64", None);
        assert_eq!(
            notice.command.as_deref(),
            Some("yay -Syu chatwithwork-desktop-bin")
        );
        assert!(notice.how.contains("AUR"));
        assert!(notice.download.is_none());
        let brew = at(
            Unsupported::Homebrew,
            managed(Manager::Homebrew, "brew upgrade cww"),
            &fake,
        );
        assert_eq!(
            super::notice(&brew, "0.12.0", "x86_64", None)
                .command
                .as_deref(),
            Some("brew upgrade cww")
        );
        let nix = at(
            Unsupported::Nix,
            Some(Managed {
                manager: Manager::Nix,
                command: None,
            }),
            &fake,
        );
        let nix = super::notice(&nix, "0.12.0", "x86_64", None);
        assert!(nix.command.is_none());
        assert!(nix.how.contains("nixos-rebuild"));
    }

    #[test]
    fn an_rpm_that_says_no_vendor_gets_no_repository_command() {
        let dnf = Unsupported::SystemPackage(fastframe_update::PackageManager::Dnf);
        let silent = Fake::default();
        let command = "sudo dnf upgrade --refresh chatwithwork-desktop";
        let installed = at(dnf.clone(), managed(Manager::Dnf, command), &silent);
        assert_eq!(installed, Installed::Other(dnf));
        assert!(
            notice(&installed, "0.12.0", "x86_64", None)
                .command
                .is_none()
        );
    }

    #[test]
    fn a_local_feed_names_its_own_files() {
        let notice = notice_from(
            &Installed::Deb,
            "0.12.0",
            "x86_64",
            None,
            "http://127.0.0.1:8799",
        );
        assert_eq!(
            notice.download.as_deref(),
            Some("http://127.0.0.1:8799/chatwithwork-desktop_0.12.0_amd64.deb")
        );
    }

    #[test]
    fn rpm_vendors_tell_copr_and_obs_from_the_release_file() {
        let dnf = Unsupported::SystemPackage(fastframe_update::PackageManager::Dnf);
        let copr = Fake {
            vendor: Some("Fedora Copr - user paolino".into()),
            ..Fake::default()
        };
        let command = "sudo dnf upgrade --refresh chatwithwork-desktop";
        assert_eq!(
            at(dnf.clone(), managed(Manager::Dnf, command), &copr),
            Installed::Managed(managed(Manager::Dnf, command).unwrap())
        );
        let file = Fake {
            vendor: Some("Plenty".into()),
            ..Fake::default()
        };
        let rpm = at(dnf, managed(Manager::Dnf, command), &file);
        assert_eq!(rpm, Installed::Rpm);
        assert_eq!(
            notice(&rpm, "0.12.0", "aarch64", None).command.as_deref(),
            Some(
                "sudo dnf install https://github.com/crmne/chatwithwork-desktop/releases/download/v0.12.0/chatwithwork-desktop-0.12.0-1.aarch64.rpm"
            )
        );
    }

    // Linux and Unix paths: these installs don't exist on Windows.
    #[cfg(unix)]
    #[test]
    fn a_deb_and_a_flatpak_bundle_are_downloaded_then_installed() {
        let fake = Fake::default();
        let deb = at(
            Unsupported::SystemPackage(fastframe_update::PackageManager::Apt),
            managed(
                Manager::Apt,
                "sudo apt update && sudo apt install --only-upgrade chatwithwork-desktop",
            ),
            &fake,
        );
        assert_eq!(deb, Installed::Deb);
        let notice = notice(&deb, "0.12.0", "x86_64", Some(Path::new("/srv/dl")));
        assert_eq!(
            notice.download.as_deref(),
            Some(
                "https://github.com/crmne/chatwithwork-desktop/releases/download/v0.12.0/chatwithwork-desktop_0.12.0_amd64.deb"
            )
        );
        assert_eq!(
            notice.command.as_deref(),
            Some("sudo apt install /srv/dl/chatwithwork-desktop_0.12.0_amd64.deb")
        );
        let flatpak = at(
            Unsupported::Flatpak,
            managed(Manager::Flatpak, "flatpak update com.chatwithwork.desktop"),
            &fake,
        );
        assert_eq!(flatpak, Installed::FlatpakBundle);
        assert_eq!(
            super::notice(&flatpak, "0.12.0", "x86_64", Some(Path::new("/d")))
                .command
                .as_deref(),
            Some("flatpak install --user /d/chatwithwork-desktop-0.12.0-x86_64.flatpak")
        );
    }

    // Linux and Unix paths: these installs don't exist on Windows.
    #[cfg(unix)]
    #[test]
    fn an_appimage_points_at_its_updater_and_its_download() {
        let mut fake = Fake::default();
        fake.vars
            .insert("APPIMAGE", "/home/me/Apps/Chat with Work.AppImage".into());
        fake.vars.insert("APPDIR", "/tmp/.mount_cwwXYZ".into());
        let appimage = installed(
            &Unsupported::MissingAsset,
            Path::new("/tmp/.mount_cwwXYZ/usr/bin/cww-app"),
            false,
            None,
            &fake,
        );
        assert_eq!(
            appimage,
            Installed::AppImage("/home/me/Apps/Chat with Work.AppImage".into())
        );
        let notice = notice(&appimage, "0.12.0", "x86_64", None);
        assert_eq!(
            notice.command.as_deref(),
            Some("appimageupdatetool '/home/me/Apps/Chat with Work.AppImage'")
        );
        assert!(
            notice
                .download
                .unwrap()
                .ends_with("/v0.12.0/chatwithwork-desktop-0.12.0-x86_64.AppImage")
        );
        // A program an AppImage started isn't that AppImage.
        assert_ne!(
            installed(
                &Unsupported::NotPortable,
                Path::new("/usr/local/bin/cww-app"),
                false,
                None,
                &fake
            ),
            appimage
        );
    }

    #[test]
    fn the_installer_the_pkg_and_the_rest() {
        let fake = Fake::default();
        if cfg!(target_os = "linux") {
            assert_eq!(
                installed(
                    &Unsupported::NotPortable,
                    Path::new("/home/me/.local/bin/cww-app"),
                    false,
                    None,
                    &fake
                ),
                Installed::Installer
            );
        }
        assert_eq!(
            notice(&Installed::Installer, "0.12.0", "x86_64", None)
                .command
                .as_deref(),
            Some(INSTALLER)
        );
        let pkg = installed(
            &Unsupported::NotPortable,
            Path::new("/Applications/x"),
            true,
            None,
            &fake,
        );
        assert_eq!(pkg, Installed::Pkg);
        let pkg = notice(&pkg, "0.12.0", "aarch64", None);
        assert!(pkg.command.is_none());
        assert!(
            pkg.download
                .unwrap()
                .ends_with("/cww-v0.12.0-macos-universal.pkg")
        );
        let system = notice(
            &Installed::Other(Unsupported::SystemDirectory),
            "0.12.0",
            "x86_64",
            None,
        );
        assert!(system.how.contains("package manager"));
        assert!(system.command.is_none());
    }

    // Linux and Unix paths: these installs don't exist on Windows.
    #[cfg(unix)]
    #[test]
    fn paths_are_quoted_for_the_shell_only_when_they_need_it() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(
            shell_path(Path::new("/home/me/Downloads/a.deb"), home),
            "~/Downloads/a.deb"
        );
        assert_eq!(shell_path(Path::new("/srv/a.deb"), home), "/srv/a.deb");
        assert_eq!(
            shell_path(Path::new("/home/me/My Files/it's.deb"), home),
            r"'/home/me/My Files/it'\''s.deb'"
        );
    }

    #[test]
    fn no_sentence_has_an_em_dash() {
        let mut installs = vec![
            Installed::Deb,
            Installed::Rpm,
            Installed::FlatpakBundle,
            Installed::AppImage("/a".into()),
            Installed::Installer,
            Installed::Pkg,
            Installed::Other(Unsupported::Cargo),
        ];
        for manager in [
            Manager::Pacman,
            Manager::Aur,
            Manager::Dnf,
            Manager::Zypper,
            Manager::Homebrew,
            Manager::HomebrewCask,
            Manager::Nix,
            Manager::Snap,
            Manager::Cargo,
        ] {
            installs.push(Installed::Managed(Managed {
                manager,
                command: None,
            }));
        }
        for installed in installs {
            let notice = notice(&installed, "0.12.0", "x86_64", None);
            assert!(!notice.how.contains('\u{2014}'), "{installed:?}");
        }
    }
}
