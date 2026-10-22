//! Pictures of the windows, for docs and reviews, in `--demo` builds on
//! Linux only. WebKitGTK paints the snapshot itself, so it works while the
//! window is on a hidden workspace where the compositor shows nothing.
//!
//! `CWW_APP_SNAPSHOT=<dir>` takes the settings window's pages one by one
//! (or `CWW_APP_SNAPSHOT_PAGES=folders,general`, where `main` is the web
//! app's window), in the theme
//! `CWW_APP_SNAPSHOT_THEME` names (`light` or `dark`), saves
//! `<dir>/<page>-<theme>.png`, and quits.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::windows::SETTINGS;

pub fn requested() -> Option<PathBuf> {
    std::env::var_os("CWW_APP_SNAPSHOT").map(PathBuf::from)
}

/// Takes the pictures, then quits. Call from a thread of its own.
pub fn run(app: AppHandle, dir: PathBuf) {
    let theme = std::env::var("CWW_APP_SNAPSHOT_THEME").unwrap_or_else(|_| "light".into());
    let pages: Vec<String> = std::env::var("CWW_APP_SNAPSHOT_PAGES")
        .map(|list| list.split(',').map(str::to_string).collect())
        .unwrap_or_else(|_| crate::args::PAGES.iter().map(|p| p.to_string()).collect());
    let _ = std::fs::create_dir_all(&dir);
    let mut login = std::env::var("CWW_APP_SNAPSHOT_LOGIN")
        .ok()
        .and_then(|value| {
            value
                .split_once(':')
                .map(|(e, p)| (e.to_string(), p.to_string()))
        });
    std::thread::sleep(Duration::from_secs(3));
    let Some(window) = app.get_webview_window(SETTINGS) else {
        log::error!("snapshot: no settings window");
        app.exit(1);
        return;
    };
    let _ = window.set_theme(Some(if theme == "dark" {
        tauri::Theme::Dark
    } else {
        tauri::Theme::Light
    }));
    for page in pages {
        // `bridge`: checks the web app's bridge in the real window. The page
        // asks for a loopback callback, the check plays the browser coming
        // back, and the page's title reports what it received.
        if page == "bridge" {
            bridge_check(&app);
            continue;
        }
        // `main` is the web app's window on its start page, `main:/path` on
        // one of the server's pages. With `CWW_APP_SNAPSHOT_LOGIN`
        // (`email:password`, a development server's synthetic login), the
        // first one signs in through the page's own form first.
        if let Some(rest) = page.strip_prefix("main") {
            let desktop = app.state::<std::sync::Arc<crate::desktop::Desktop>>();
            let url = rest
                .strip_prefix(':')
                .and_then(|path| desktop.server().page(path));
            let _ = crate::windows::show_main(&app, url);
            std::thread::sleep(Duration::from_secs(6));
            if let Some(main) = app.get_webview_window(crate::windows::MAIN) {
                if let Some((email, password)) = login.take() {
                    let script = format!(
                        r#"(() => {{ const f = document.querySelector('input[name="identity[password]"]')?.form; if (!f) return; f.querySelector('[name="identity[email]"]').value = {}; f.querySelector('[name="identity[password]"]').value = {}; f.requestSubmit(); }})()"#,
                        serde_json::json!(email),
                        serde_json::json!(password)
                    );
                    let _ = main.eval(&script);
                    std::thread::sleep(Duration::from_secs(6));
                    if let Some(url) = rest
                        .strip_prefix(':')
                        .and_then(|path| desktop.server().page(path))
                    {
                        let _ = main.navigate(url);
                        std::thread::sleep(Duration::from_secs(5));
                    }
                }
                let name = rest
                    .trim_start_matches(':')
                    .trim_matches('/')
                    .replace(|c: char| !c.is_ascii_alphanumeric(), "-");
                let name = if name.is_empty() {
                    "main".to_string()
                } else {
                    format!("main-{name}")
                };
                let path = dir.join(format!("{name}-{theme}.png"));
                match capture(&main, &path) {
                    Ok(()) => println!(
                        "{} ({})",
                        path.display(),
                        main.url().map(|u| u.to_string()).unwrap_or_default()
                    ),
                    Err(e) => log::error!("snapshot of the main window: {e}"),
                }
            }
            continue;
        }
        // `scan`, then `scan:<step>`: the scan window, with each step played
        // in the page itself (no input reaches the system).
        if let Some(step) = page.strip_prefix("scan") {
            scan_step(&app, step.trim_start_matches(':'), &dir, &theme);
            continue;
        }
        crate::windows::tell_settings(&app, "navigate", &page);
        std::thread::sleep(Duration::from_millis(1500));
        let path = dir.join(format!("{page}-{theme}.png"));
        match capture(&window, &path) {
            Ok(()) => println!("{}", path.display()),
            Err(e) => log::error!("snapshot of {page}: {e}"),
        }
    }
    app.exit(0);
}

fn capture(window: &tauri::WebviewWindow, path: &Path) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let path = path.to_path_buf();
    window
        .with_webview(move |webview| {
            use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
            webview.inner().snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                None::<&webkit2gtk::gio::Cancellable>,
                move |result| {
                    let saved = result.map_err(|e| e.to_string()).and_then(|surface| {
                        let image = cairo::ImageSurface::try_from(surface)
                            .map_err(|_| "not an image surface".to_string())?;
                        let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
                        image.write_to_png(&mut file).map_err(|e| e.to_string())
                    });
                    let _ = tx.send(saved);
                },
            );
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(20))
        .map_err(|_| "timed out".to_string())?
}

fn bridge_check(app: &AppHandle) {
    let Some(main) = app.get_webview_window(crate::windows::MAIN) else {
        println!("bridge: no main window");
        return;
    };
    let _ = main.eval(
        r#"(async () => {
            const api = window.ChatWithWorkDesktop;
            if (!api) { document.title = "bridge:missing"; return; }
            window.addEventListener("chatwithwork-desktop:callback", (e) => {
                document.title = "bridge:callback:" + JSON.stringify(e.detail.params);
            });
            try {
                const { callback } = await api.listen();
                document.title = "bridge:listening:" + callback;
            } catch (e) {
                document.title = "bridge:refused:" + JSON.stringify(e);
            }
        })()"#,
    );
    std::thread::sleep(Duration::from_secs(2));
    let title = main.title().unwrap_or_default();
    println!("{title}");
    if let Some(callback) = title.strip_prefix("bridge:listening:") {
        let address = callback.trim_start_matches("http://");
        let (host, path) = address.split_once('/').unwrap_or((address, ""));
        if let Ok(mut stream) = std::net::TcpStream::connect(host) {
            use std::io::{Read, Write};
            let _ = write!(
                stream,
                "GET /{path}?token=T1&verifier=nope HTTP/1.1\r\nHost: {host}\r\n\r\n"
            );
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            println!(
                "bridge: browser saw {}",
                answer.lines().next().unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_secs(2));
        println!("{}", main.title().unwrap_or_default());
    }
}

/// One step of the scan window, then its picture (`scan-<step>-<theme>.png`):
/// `scanned` presses Scan and waits for the feeder to empty, `blank` opens
/// the blank pages left out, `crop` opens the first page's crop view,
/// `cropped` crops the first page, `turned` turns the second, and `send`
/// sends to the first bot (with `CWW_DEMO_SCAN_OUT`, the demo daemon keeps
/// the PDF).
fn scan_step(app: &AppHandle, step: &str, dir: &Path, theme: &str) {
    let Ok(()) = crate::windows::show_scan(app) else {
        log::error!("snapshot: no scan window");
        return;
    };
    std::thread::sleep(Duration::from_secs(if step.is_empty() { 4 } else { 1 }));
    let Some(window) = app.get_webview_window(crate::windows::SCAN) else {
        return;
    };
    let _ = window.set_theme(Some(if theme == "dark" {
        tauri::Theme::Dark
    } else {
        tauri::Theme::Light
    }));
    let (script, wait) = match step {
        "scanned" => (
            r#"document.querySelector('[data-key="scan"]')?.click()"#,
            12,
        ),
        "blank" => (
            r#"document.querySelector('details.left-out')?.setAttribute('open', '')"#,
            1,
        ),
        "crop" => (
            r#"document.querySelector('[data-key^="crop-"]')?.click()"#,
            3,
        ),
        "cropped" => (
            r#"(async () => { document.getElementById('crop').close(); const { pages } = await window.__TAURI_INTERNALS__.invoke('scan_info'); await window.__TAURI_INTERNALS__.invoke('scan_edit', { id: pages[0].id, crop: { x: 0.05, y: 0.05, width: 0.9, height: 0.55 } }); location.reload(); })()"#,
            4,
        ),
        "saved" => (
            r#"document.querySelector('[data-key="save"]')?.click()"#,
            10,
        ),
        "turned" => (
            r#"document.querySelectorAll('[data-key^="turn-"]')[1]?.click()"#,
            3,
        ),
        "send" => (
            r#"(() => { const s = document.querySelector('[data-key="destination"]'); if (s && s.options.length > 1) { s.value = s.options[1].value; s.dispatchEvent(new Event('change')); } setTimeout(() => document.querySelector('[data-key="send"]')?.click(), 300); })()"#,
            15,
        ),
        _ => ("", 0),
    };
    if !script.is_empty() {
        let _ = window.eval(script);
    }
    std::thread::sleep(Duration::from_secs(wait));
    // WebKitGTK takes prefers-color-scheme from GTK, not the window's theme,
    // so the picture's theme is set on the page.
    let _ = window.eval(format!(
        "document.documentElement.style.colorScheme = {}",
        serde_json::json!(theme)
    ));
    std::thread::sleep(Duration::from_millis(400));
    let name = if step.is_empty() {
        "scan".to_string()
    } else {
        format!("scan-{step}")
    };
    let path = dir.join(format!("{name}-{theme}.png"));
    match capture(&window, &path) {
        Ok(()) => println!("{}", path.display()),
        Err(e) => log::error!("snapshot of {name}: {e}"),
    }
}
