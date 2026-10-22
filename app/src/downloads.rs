//! Files the web app downloads (a chat's files, what code made, an invoice)
//! go to the Downloads folder under their own name, never replacing a file
//! already there, and the folder is shown once one arrives.

use std::path::{Path, PathBuf};

use tauri::webview::DownloadEvent;
use tauri::{Runtime, Webview};

pub fn handle<R: Runtime>(webview: Webview<R>, event: DownloadEvent<'_>) -> bool {
    match event {
        DownloadEvent::Requested { url, destination } => {
            // Only the web app downloads; the bundled pages never do.
            let from_server = crate::windows::is_remote_label(webview.label());
            let Some(folder) =
                dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            else {
                return false;
            };
            if !from_server {
                return false;
            }
            let suggested = destination
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .or_else(|| {
                    url.path_segments()
                        .and_then(|mut s| s.next_back())
                        .map(str::to_string)
                })
                .unwrap_or_default();
            let _ = std::fs::create_dir_all(&folder);
            *destination = unique(&folder, &file_name(&suggested));
            true
        }
        DownloadEvent::Finished { path, success, .. } => {
            if success {
                reveal(path.as_deref());
            }
            true
        }
        _ => true,
    }
}

/// A file name safe to create: no folders, no leading dots, nothing a
/// file system refuses, at most 120 characters.
pub fn file_name(suggested: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(suggested).decode_utf8_lossy();
    let base = decoded.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' ']);
    let cleaned: String = cleaned.chars().take(120).collect();
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned
    }
}

/// `name`, or `name (2).ext`, `name (3).ext`... whichever doesn't exist yet.
pub fn unique(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}){ext}")))
        .find(|path| !path.exists())
        .expect("some name is free")
}

/// Shows the downloaded file in the file manager (selected where the
/// platform can), or the Downloads folder where the path isn't known
/// (macOS doesn't say).
fn reveal(path: Option<&Path>) {
    let folder = dirs::download_dir();
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        match path {
            Some(path) if path.is_file() => command.arg("-R").arg(path),
            _ => match &folder {
                Some(folder) => command.arg(folder),
                None => return,
            },
        };
        let _ = command.spawn();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer");
        match path {
            // Explorer wants the quotes around the path only, not the whole
            // argument, which `arg` would quote for a path with spaces.
            Some(path) if path.is_file() => {
                command.raw_arg(format!("/select,\"{}\"", path.display()))
            }
            _ => match &folder {
                Some(folder) => command.arg(folder),
                None => return,
            },
        };
        let _ = command.spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = path
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or(folder);
        if let Some(target) = target {
            let _ = open::that_detached(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_folders_and_characters_file_systems_refuse() {
        assert_eq!(file_name("Q3 plan.xlsx"), "Q3 plan.xlsx");
        assert_eq!(file_name("../../.bashrc"), "bashrc");
        assert_eq!(file_name("..\\evil.exe"), "evil.exe");
        assert_eq!(file_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(file_name("report%20final.pdf"), "report final.pdf");
        assert_eq!(file_name("a+b.txt"), "a+b.txt");
        assert_eq!(file_name(""), "download");
        assert_eq!(file_name("..."), "download");
        assert_eq!(file_name(&"x".repeat(300)).len(), 120);
    }

    #[test]
    fn never_replaces_a_file_already_there() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique(dir.path(), "a.txt"), dir.path().join("a.txt"));
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        assert_eq!(unique(dir.path(), "a.txt"), dir.path().join("a (2).txt"));
        std::fs::write(dir.path().join("a (2).txt"), "").unwrap();
        assert_eq!(unique(dir.path(), "a.txt"), dir.path().join("a (3).txt"));
        std::fs::write(dir.path().join("README"), "").unwrap();
        assert_eq!(unique(dir.path(), "README"), dir.path().join("README (2)"));
    }
}
