//! Locate the helper belonging to this installation, without searching PATH.
//! macOS must execute the real binary inside an application bundle; a loose
//! executable or a bundle containing a symlink to a loose binary has no native
//! notification identity. The broker validates the actual bundle identifier.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
enum Platform {
    Linux,
    Macos,
    Windows,
}

/// Resolve before entering a sandbox. A missing helper is normal for a local
/// CLI-only development build: retain the browser fallback and report native
/// delivery as unavailable. Never launch an arbitrary PATH replacement.
pub fn installed_helper() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let platform = if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    };
    find(&executable, platform)
}

fn find(executable: &Path, platform: Platform) -> Option<PathBuf> {
    let parent = executable.parent()?;
    let candidates = match platform {
        Platform::Linux => vec![parent.join("cww-notify"), parent.join("cww-app")],
        Platform::Windows => vec![
            parent.join("cww-app.exe"),
            parent.join("Chat with Work.exe"),
        ],
        Platform::Macos => vec![
            parent.join("cww-app"),
            // Portable macOS CLI archive, with the signed bundle next to cww.
            parent.join("Chat with Work.app/Contents/MacOS/cww-app"),
        ],
    };
    candidates.into_iter().find_map(|path| {
        if !path.is_absolute() || !is_executable(&path) {
            return None;
        }
        if matches!(platform, Platform::Macos) {
            let real = path.canonicalize().ok()?;
            let macos = real.parent()?;
            let contents = macos.parent()?;
            let app = contents.parent()?;
            if macos.file_name()? != "MacOS"
                || contents.file_name()? != "Contents"
                || app.extension()? != "app"
                || !contents.join("Info.plist").is_file()
            {
                return None;
            }
            Some(real)
        } else {
            Some(path)
        }
    })
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn cli_helper_is_preferred_and_missing_helpers_preserve_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("cww");
        assert_eq!(find(&executable, Platform::Linux), None);
        file(&dir.path().join("cww-app"));
        assert_eq!(
            find(&executable, Platform::Linux),
            Some(dir.path().join("cww-app"))
        );
        file(&dir.path().join("cww-notify"));
        assert_eq!(
            find(&executable, Platform::Linux),
            Some(dir.path().join("cww-notify"))
        );
    }

    #[test]
    fn renamed_windows_portable_app_is_supported() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Chat with Work.exe");
        file(&app);
        assert_eq!(
            find(&dir.path().join("cww.exe"), Platform::Windows),
            Some(app)
        );
    }

    #[test]
    fn macos_portable_archive_uses_the_bundled_helper_not_a_loose_binary() {
        let dir = tempfile::tempdir().unwrap();
        file(&dir.path().join("cww-app"));
        let executable = dir.path().join("cww");
        assert_eq!(find(&executable, Platform::Macos), None);
        let app = dir.path().join("Chat with Work.app/Contents");
        file(&app.join("MacOS/cww-app"));
        assert_eq!(find(&executable, Platform::Macos), None);
        file(&app.join("Info.plist"));
        assert_eq!(
            find(&executable, Platform::Macos),
            Some(app.join("MacOS/cww-app").canonicalize().unwrap())
        );
    }

    #[cfg(unix)]
    #[test]
    fn macos_package_symlink_resolves_into_bundle_and_reverse_symlink_is_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Chat with Work.app/Contents");
        let binary = app.join("MacOS/cww-app");
        file(&binary);
        file(&app.join("Info.plist"));
        symlink(&binary, dir.path().join("cww-app")).unwrap();
        assert_eq!(
            find(&dir.path().join("cww"), Platform::Macos),
            Some(binary.canonicalize().unwrap())
        );
        std::fs::remove_file(dir.path().join("cww-app")).unwrap();
        std::fs::remove_file(&binary).unwrap();
        file(&dir.path().join("cww-app"));
        symlink(dir.path().join("cww-app"), &binary).unwrap();
        assert_eq!(find(&app.join("MacOS/cww"), Platform::Macos), None);
    }
}
