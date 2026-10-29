//! The scan window: scan paper with a scanner attached to this computer,
//! check and fix the pages, and send them to Chat with Work as one
//! searchable PDF (cww-scan does the scanning, OCR and PDF).
//!
//! - The scanner is driven by a helper process (`cww-app --scan-helper`),
//!   so a driver that crashes or hangs never takes the app with it. It runs
//!   while the window is open and goes with it.
//! - Pages are kept in a private folder of the app's own (0700, under the
//!   app's state directory) until Rescan, the window closing, or the app's
//!   next start deletes them. Nothing leaves the computer before Send.
//! - Send makes the PDF here (OCR included) and hands it to the daemon,
//!   which sends it as this computer's owner, with the device's token, to a
//!   new chat or to a bot (CONTROL.md, `scan_send`).
//!
//! Only bundled pages in the scan window reach these commands
//! (capabilities/scan.json, and [`scan_page`] checks again).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cww::control::ControlRequest;
use cww_scan::edit::Edits;
use cww_scan::helper::{Canceller, Helper, PageFile};
use cww_scan::ocr::{Engine, Language};
use cww_scan::{Capabilities, ColorMode, Device, End, Request, ScanError, Settings};
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State, Webview};

use crate::desktop::Desktop;
use crate::windows::{SCAN, is_local_url};

/// Chat with Work's largest file.
pub const MAX_PDF_BYTES: usize = cww::chats::MAX_UPLOAD_BYTES;
/// A thumbnail's longest side, in pixels.
const THUMB: u32 = 360;
/// The crop view's longest side.
const PREVIEW: u32 = 1600;

/// A page of the batch.
#[derive(Clone)]
struct Page {
    id: u64,
    path: PathBuf,
    resolution: u32,
    color: ColorMode,
    /// Judged blank as it arrived: left out unless brought back.
    blank: bool,
    edits: Edits,
    thumb: String,
    width: u32,
    height: u32,
}

impl Page {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "blank": self.blank,
            "thumb": self.thumb,
            "turns": self.edits.turns,
            "crop": self.edits.crop,
            "width": self.width,
            "height": self.height,
        })
    }
}

#[derive(Default)]
struct Session {
    /// The private folder the pages are in, made on the first scan.
    dir: Option<PathBuf>,
    pages: Vec<Page>,
    /// The scanner the batch came from, for the bot's run.
    scanner: Option<String>,
    /// So a retried send is heard once.
    scan_id: Option<String>,
}

/// The person's choices, remembered between scans (`scan.json` beside the
/// app's settings).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub settings: Option<Settings>,
    pub device: Option<String>,
    /// Where Save to Computer last saved, where its dialog starts.
    pub save_dir: Option<PathBuf>,
}

impl Preferences {
    fn file(paths: &crate::paths::Paths) -> PathBuf {
        paths.daemon.config_dir.join("scan.json")
    }

    pub fn load(paths: &crate::paths::Paths) -> Self {
        std::fs::read(Self::file(paths))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &crate::paths::Paths) -> std::io::Result<()> {
        let file = Self::file(paths);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(file, serde_json::to_vec_pretty(self).unwrap_or_default())
    }
}

/// A page of the Chat with Work window waiting for a scan for its composer
/// (bridge.rs's `scan`).
struct Attaching {
    /// Which request this is, so a failed one resolves only itself.
    request: u64,
    /// The window that asked; only it gets the PDF.
    label: String,
    reply: tokio::sync::oneshot::Sender<Option<Value>>,
}

/// A PDF nobody took in time goes, so a later new chat can't pick it up.
fn forget_expired(slot: &mut Option<Attached>) {
    if slot
        .as_ref()
        .is_some_and(|attached| attached.made.elapsed() >= ATTACHED_FOR)
    {
        *slot = None;
    }
}

/// How long a PDF made for a composer waits for its page.
const ATTACHED_FOR: std::time::Duration = std::time::Duration::from_secs(300);

/// A PDF made for a composer, until its page takes it or it's been waiting
/// longer than `ATTACHED_FOR`.
struct Attached {
    made: std::time::Instant,
    id: String,
    label: String,
    /// `{ id, name, pages, size }`, as the page hears of it.
    answer: Value,
    pdf: Vec<u8>,
}

/// What the scan window shares.
pub struct Scans {
    attaching: Mutex<Option<Attaching>>,
    attached: Mutex<Option<Attached>>,
    session: Mutex<Session>,
    helper: Mutex<Option<Helper>>,
    canceller: Mutex<Option<Canceller>>,
    busy: AtomicBool,
    next_id: AtomicU64,
    engine: Result<Arc<dyn Engine>, String>,
    /// Where languages the person adds are kept (Linux's Tesseract only).
    added_languages: Option<PathBuf>,
    language_download: LanguageDownload,
}

/// The language pack downloading, one at a time.
#[derive(Default)]
struct LanguageDownload {
    running: AtomicBool,
    cancelled: Arc<AtomicBool>,
}

impl Default for Scans {
    fn default() -> Self {
        Self {
            attaching: Mutex::default(),
            attached: Mutex::default(),
            session: Mutex::default(),
            helper: Mutex::default(),
            canceller: Mutex::default(),
            busy: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            engine: cww_scan::ocr::system_engine().map(Arc::from),
            added_languages: None,
            language_download: LanguageDownload::default(),
        }
    }
}

/// Where the languages the person adds are kept: beside the daemon's other
/// downloads (the Whisper models), never in the system's tessdata.
pub fn added_languages(paths: &crate::paths::Paths) -> PathBuf {
    paths.daemon.data_dir.join("ocr-languages")
}

/// Whether this computer can add languages by itself: Tesseract reads from
/// a folder of the app's; Vision's languages are the Mac's, and Windows adds
/// its own in Settings.
const DOWNLOADS_LANGUAGES: bool = cfg!(not(any(target_os = "macos", windows)));

/// Where sessions keep their pages: under the app's state directory, never
/// a shared temporary folder.
pub fn scans_root(paths: &crate::paths::Paths) -> PathBuf {
    paths.daemon.state_dir.join("scans")
}

impl Scans {
    /// The scanner's state, reading the languages added in `paths` too.
    pub fn new(paths: &crate::paths::Paths) -> Self {
        let added = DOWNLOADS_LANGUAGES.then(|| added_languages(paths));
        Self {
            engine: cww_scan::ocr::system_engine_with(added.clone()).map(Arc::from),
            added_languages: added,
            ..Self::default()
        }
    }

    /// Deletes the pages and stops the helper: the window closed, or the
    /// app starts (a crash may have left a session behind).
    pub fn close(&self, paths: &crate::paths::Paths) {
        // A composer still waiting gets nothing: the window closed.
        if let Some(waiting) = self.attaching.lock().expect("attaching").take() {
            let _ = waiting.reply.send(None);
        }
        if let Some(canceller) = self.canceller.lock().expect("canceller").take() {
            canceller.cancel();
        }
        *self.helper.lock().expect("helper") = None;
        *self.session.lock().expect("session") = Session::default();
        let _ = std::fs::remove_dir_all(scans_root(paths));
    }

    /// The window `label` waits for a scan for its composer. A page that
    /// was waiting already gets nothing: the newest asks.
    pub fn attach_for(&self, label: &str) -> (u64, tokio::sync::oneshot::Receiver<Option<Value>>) {
        let (reply, answer) = tokio::sync::oneshot::channel();
        let request = self.next_id.fetch_add(1, Ordering::SeqCst);
        let previous = self
            .attaching
            .lock()
            .expect("attaching")
            .replace(Attaching {
                request,
                label: label.to_string(),
                reply,
            });
        if let Some(previous) = previous {
            let _ = previous.reply.send(None);
        }
        (request, answer)
    }

    /// The scan window couldn't open for `request`: its page gets nothing,
    /// and a newer request stays as it is.
    pub fn cancel_attach(&self, request: u64) {
        let mut slot = self.attaching.lock().expect("attaching");
        if slot
            .as_ref()
            .is_some_and(|waiting| waiting.request == request)
            && let Some(waiting) = slot.take()
        {
            let _ = waiting.reply.send(None);
        }
    }

    fn is_attaching(&self) -> bool {
        self.attaching.lock().expect("attaching").is_some()
    }

    /// The PDF made for the window `label`'s composer, once.
    pub fn take_attached(&self, label: &str, id: &str) -> Option<Vec<u8>> {
        let mut slot = self.attached.lock().expect("attached");
        forget_expired(&mut slot);
        match slot.as_ref() {
            Some(attached) if attached.label == label && attached.id == id => {
                slot.take().map(|attached| attached.pdf)
            }
            _ => None,
        }
    }

    /// A scan waiting for the new chat in the window `label`, if one is.
    pub fn pending_for(&self, label: &str) -> Option<Value> {
        let mut slot = self.attached.lock().expect("attached");
        forget_expired(&mut slot);
        slot.as_ref()
            .filter(|attached| attached.label == label)
            .map(|attached| attached.answer.clone())
    }

    fn discard_pages(&self) {
        let mut session = self.session.lock().expect("session");
        if let Some(dir) = session.dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
        *session = Session::default();
    }

    fn with_helper<T>(
        &self,
        work: impl FnOnce(&mut Helper) -> Result<T, ScanError>,
    ) -> Result<T, ScanError> {
        let mut slot = self.helper.lock().expect("helper");
        if slot.is_none() {
            *slot = Some(Helper::spawn(helper_command()?)?);
        }
        let result = work(slot.as_mut().expect("just started"));
        // A helper that stopped is started again next time.
        if matches!(&result, Err(e) if e.code == "helper_stopped") {
            *slot = None;
        }
        result
    }

    fn page_dir(&self, paths: &crate::paths::Paths) -> std::io::Result<PathBuf> {
        let mut session = self.session.lock().expect("session");
        if let Some(dir) = &session.dir {
            return Ok(dir.clone());
        }
        let root = scans_root(paths);
        cww::paths::ensure_private_dir(&root).map_err(std::io::Error::other)?;
        let dir = root.join(random_id());
        cww::paths::ensure_private_dir(&dir).map_err(std::io::Error::other)?;
        session.dir = Some(dir.clone());
        Ok(dir)
    }
}

/// How the scanner's helper starts. On Windows it's `cww-scan-helper.exe`
/// beside the app, a 32-bit program, since most TWAIN drivers are 32-bit and
/// load only into a 32-bit process; elsewhere the app itself, as
/// `cww-app --scan-helper`.
fn helper_command() -> Result<std::process::Command, ScanError> {
    let exe =
        std::env::current_exe().map_err(|e| ScanError::new("helper_stopped", e.to_string()))?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let helper = exe.with_file_name("cww-scan-helper.exe");
        if !helper.is_file() {
            return Err(ScanError::new(
                "helper_stopped",
                "This copy of the app is missing its scanner helper (cww-scan-helper.exe). Install the app again.",
            ));
        }
        let mut command = std::process::Command::new(helper);
        // No console window for the helper.
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        Ok(command)
    }
    #[cfg(not(windows))]
    {
        let mut command = std::process::Command::new(exe);
        command.arg("--scan-helper");
        Ok(command)
    }
}

fn random_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

type Answer = Result<Value, Value>;

fn refusal(code: &str, message: impl Into<String>) -> Value {
    json!({ "code": code, "message": message.into() })
}

fn scan_refusal(e: ScanError) -> Value {
    refusal(e.code, e.message)
}

/// The caller must be a bundled page in the scan window.
fn scan_page_only(webview: &Webview) -> Result<(), Value> {
    if webview.label() == SCAN && webview.url().is_ok_and(|url| is_local_url(&url)) {
        Ok(())
    } else {
        Err(refusal(
            "forbidden",
            "Only the app's own scan page can ask for this.",
        ))
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Value> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| refusal("failed", e.to_string()))
}

/// The language a first scan reads in: the system's, when the engine has
/// it, else English, else the first it has.
/// Why the switch for searchable text is off when the engine reads no
/// language: the engine's own error, else what this system needs.
fn no_languages(error: &Option<String>) -> String {
    if let Some(error) = error {
        return error.clone();
    }
    if cfg!(windows) {
        "Windows has no language it reads text in. Add a language with optical character recognition in Settings, Time & language, Language & region.".into()
    } else if cfg!(target_os = "macos") {
        "This Mac lists no language it reads text in.".into()
    } else {
        cww_scan::ocr::Distro::here().tesseract_missing()
    }
}

fn default_language(languages: &[Language]) -> Option<String> {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    // Tesseract names languages in three letters ("deu"), Vision by locale
    // ("de-DE"); either matches the system's two-letter language.
    let two = locale.get(..2).unwrap_or_default();
    let three = match two {
        "de" => "deu",
        "fr" => "fra",
        "it" => "ita",
        "es" => "spa",
        "nl" => "nld",
        "pt" => "por",
        "pl" => "pol",
        _ => "eng",
    };
    let matching = |two: &str, three: &str| {
        languages
            .iter()
            .find(|l| l.code == three || l.code.split('-').next() == Some(two))
            .map(|l| l.code.clone())
    };
    matching(two, three)
        .or_else(|| matching("en", "eng"))
        .or_else(|| languages.first().map(|l| l.code.clone()))
}

/// Loads a page as scanned, from the session's folder.
fn load(page: &Page) -> Result<DynamicImage, Value> {
    image::open(&page.path).map_err(|_| refusal("missing", "A page went missing. Scan it again."))
}

fn data_url(image: &DynamicImage, longest: u32, quality: u8) -> String {
    use base64::Engine as _;
    let small = if image.width().max(image.height()) > longest {
        image.thumbnail(longest, longest)
    } else {
        image.clone()
    };
    let small = match small {
        DynamicImage::ImageLuma8(_) => small,
        other => DynamicImage::ImageRgb8(other.to_rgb8()),
    };
    let mut jpeg = Vec::new();
    let _ =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality).encode_image(&small);
    format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    )
}

/// The page as it will be sent, made black and white where asked, so the
/// thumbnail shows what goes out.
fn as_sent(page: &Page, image: &DynamicImage) -> DynamicImage {
    let edited = page.edits.apply(image);
    if page.color == ColorMode::BlackAndWhite {
        DynamicImage::ImageLuma8(cww_scan::pdf::threshold(&edited.to_luma8()))
    } else {
        edited
    }
}

/// Tells the scan window something happened (`scan` events), through its
/// own `window.cwwEvent`, as the settings get theirs.
fn tell(app: &AppHandle, payload: Value) {
    crate::windows::tell_window(app, SCAN, "scan", payload);
}

/// Tells the scan page whether a composer waits for its scan, so it offers
/// Attach in place of Send.
pub fn tell_attaching(app: &AppHandle, attaching: bool) {
    tell(app, json!({ "kind": "attaching", "attaching": attaching }));
}

/// A language code in words, for saying it's missing.
fn language_name(code: &str) -> String {
    let name = match code.split('-').next().unwrap_or(code) {
        "de" | "deu" => "German",
        "en" | "eng" => "English",
        "fr" | "fra" => "French",
        "it" | "ita" => "Italian",
        "es" | "spa" => "Spanish",
        "nl" | "nld" => "Dutch",
        "pt" | "por" => "Portuguese",
        "pl" | "pol" => "Polish",
        _ => return code.to_string(),
    };
    name.to_string()
}

/// Where a missing language is added on this system, in words.
fn add_language_hint(code: &str) -> String {
    let name = language_name(code);
    if cfg!(windows) {
        format!(
            "Add {name} in Windows Settings, Time & language, Language & region, with \"Optical character recognition\" ticked, then check again."
        )
    } else if cfg!(target_os = "macos") {
        format!("This Mac's text recognition doesn't read {name}.")
    } else {
        cww_scan::ocr::Distro::here().language_hint(code, &name)
    }
}

/// Opens the system's language settings, where an OCR language is added
/// (Windows only; elsewhere it's a package, or not possible).
#[tauri::command]
pub fn scan_add_language(webview: Webview) -> Answer {
    scan_page_only(&webview)?;
    if cfg!(windows) {
        open::that_detached("ms-settings:regionlanguage")
            .map_err(|e| refusal("failed", e.to_string()))?;
    }
    Ok(json!({}))
}

/// The languages this computer can add by itself and doesn't read yet,
/// by name: `[{ code, name, bytes }]`.
#[tauri::command]
pub fn scan_language_packs(webview: Webview, scans: State<'_, Arc<Scans>>) -> Answer {
    scan_page_only(&webview)?;
    if scans.added_languages.is_none() {
        return Ok(json!({ "packs": [] }));
    }
    let have: Vec<String> = match &scans.engine {
        Ok(engine) => engine
            .languages()
            .unwrap_or_default()
            .into_iter()
            .map(|l| l.code)
            .collect(),
        Err(_) => return Ok(json!({ "packs": [] })),
    };
    Ok(json!({ "packs": packs_to_add(&have) }))
}

fn packs_to_add(have: &[String]) -> Vec<Value> {
    let mut packs: Vec<&cww_scan::languages::Pack> = cww_scan::languages::PACKS
        .iter()
        .filter(|p| !have.iter().any(|code| code == p.code))
        .collect();
    packs.sort_by_key(|p| p.name);
    packs
        .into_iter()
        .map(|p| json!({ "code": p.code, "name": p.name, "bytes": p.bytes }))
        .collect()
}

/// Downloads a language pack into the app's folder, checked against its
/// pinned SHA-256. Progress arrives as `scan` events of kind `language`
/// (`code`, `bytes`, `total`, then `done` with an `error` if it failed).
#[tauri::command]
pub fn scan_download_language(
    webview: Webview,
    app: AppHandle,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<Scans>>,
    code: String,
) -> Answer {
    scan_page_only(&webview)?;
    let folder = scans.added_languages.clone().ok_or_else(|| {
        refusal(
            "unsupported",
            "This computer adds languages in its own settings.",
        )
    })?;
    let pack = cww_scan::languages::pack(&code)
        .ok_or_else(|| refusal("invalid", "That isn't a language Chat with Work can add."))?;
    let download = &scans.language_download;
    if download.running.swap(true, Ordering::SeqCst) {
        return Err(refusal("busy", "A language is already downloading."));
    }
    download.cancelled.store(false, Ordering::SeqCst);
    let scans = scans.inner().clone();
    let paths = desktop.paths.daemon.clone();
    let spawned = std::thread::Builder::new()
        .name("cww-language-download".into())
        .spawn({
            let scans = scans.clone();
            move || {
                // Clears `running` however this ends, a panic included.
                struct Running(Arc<Scans>);
                impl Drop for Running {
                    fn drop(&mut self) {
                        self.0.language_download.running.store(false, Ordering::SeqCst);
                    }
                }
                let _running = Running(scans.clone());
                let progress = |bytes| {
                    tell(
                        &app,
                        json!({ "kind": "language", "code": pack.code, "bytes": bytes, "total": pack.bytes }),
                    )
                };
                progress(0);
                let last = AtomicU64::new(0);
                let url = pack.url();
                let result = cww::fetch::verified(
                    &paths,
                    &cww::fetch::Pinned {
                        url: &url,
                        bytes: pack.bytes,
                        sha256: pack.sha256,
                        what: "language",
                    },
                    &folder.join(format!("{}.traineddata", pack.code)),
                    &scans.language_download.cancelled,
                    |bytes| {
                        // At most every quarter megabyte, so the page isn't flooded.
                        if bytes.saturating_sub(last.load(Ordering::Relaxed)) >= 250_000
                            || bytes == pack.bytes
                        {
                            last.store(bytes, Ordering::Relaxed);
                            progress(bytes);
                        }
                    },
                );
                scans.language_download.running.store(false, Ordering::SeqCst);
                let error = result.err().map(|e| format!("{e:#}"));
                tell(
                    &app,
                    json!({ "kind": "language", "code": pack.code, "done": true, "error": error }),
                );
            }
        });
    if let Err(e) = spawned {
        scans
            .language_download
            .running
            .store(false, Ordering::SeqCst);
        return Err(refusal("failed", e.to_string()));
    }
    Ok(json!({}))
}

#[tauri::command]
pub fn scan_cancel_language(webview: Webview, scans: State<'_, Arc<Scans>>) -> Answer {
    scan_page_only(&webview)?;
    scans
        .language_download
        .cancelled
        .store(true, Ordering::SeqCst);
    Ok(json!({}))
}

#[tauri::command]
pub fn scan_info(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<Scans>>,
) -> Answer {
    scan_page_only(&webview)?;
    let (languages, languages_error) = match &scans.engine {
        Ok(engine) => match engine.languages() {
            Ok(languages) => (languages, None),
            Err(e) => (Vec::new(), Some(e)),
        },
        Err(_) => (Vec::new(), None),
    };
    let preferences = Preferences::load(&desktop.paths);
    let mut settings = preferences.settings.clone().unwrap_or_default();
    // A first scan reads in the system's language where it can.
    if preferences.settings.is_none() {
        settings.ocr = default_language(&languages);
    }
    // A chosen language this computer can't read any more is kept and said
    // so, never swapped for another: the page offers to add it.
    let missing = settings
        .ocr
        .as_ref()
        .filter(|code| !languages.is_empty() && !languages.iter().any(|l| &l.code == *code))
        .map(|code| {
            json!({
                "code": code,
                "name": language_name(code),
                "how": add_language_hint(code),
                // Whether the window can download it itself.
                "pack": scans.added_languages.is_some() && cww_scan::languages::pack(code).is_some(),
            })
        });
    let session = scans.session.lock().expect("session");
    Ok(json!({
        "settings": settings,
        "device": preferences.device,
        "languages": languages,
        "ocr": match &scans.engine {
            Ok(_) if languages.is_empty() => json!({ "available": false, "reason": no_languages(&languages_error) }),
            Ok(_) => json!({
                "available": true,
                "missing": missing,
                "can_add": cfg!(windows),
                "can_download": scans.added_languages.is_some(),
                "downloading": scans.language_download.running.load(Ordering::SeqCst),
            }),
            Err(reason) => json!({ "available": false, "reason": reason }),
        },
        "pages": session.pages.iter().map(Page::json).collect::<Vec<_>>(),
        "busy": scans.busy.load(Ordering::SeqCst),
        "server": desktop.server().origin(),
        "max_bytes": MAX_PDF_BYTES,
        "attaching": scans.is_attaching(),
    }))
}

#[tauri::command]
pub async fn scan_devices(webview: Webview, scans: State<'_, Arc<Scans>>) -> Answer {
    scan_page_only(&webview)?;
    let scans = scans.inner().clone();
    if scans.busy.load(Ordering::SeqCst) {
        return Err(refusal("busy", "Wait for the scan to finish."));
    }
    let devices = blocking(move || scans.with_helper(|h| h.devices()))
        .await?
        .map_err(scan_refusal)?;
    Ok(json!({ "devices": devices }))
}

#[tauri::command]
pub async fn scan_capabilities(
    webview: Webview,
    scans: State<'_, Arc<Scans>>,
    device: String,
) -> Answer {
    scan_page_only(&webview)?;
    let scans = scans.inner().clone();
    if scans.busy.load(Ordering::SeqCst) {
        return Err(refusal("busy", "Wait for the scan to finish."));
    }
    let caps: Capabilities = blocking(move || scans.with_helper(|h| h.capabilities(&device)))
        .await?
        .map_err(scan_refusal)?;
    Ok(json!({ "capabilities": caps }))
}

#[tauri::command]
pub fn scan_save_settings(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    settings: Settings,
    device: Option<String>,
) -> Answer {
    scan_page_only(&webview)?;
    Preferences {
        settings: Some(settings),
        device,
        ..Preferences::load(&desktop.paths)
    }
    .save(&desktop.paths)
    .map(|()| json!({}))
    .map_err(|e| refusal("failed", format!("Couldn't save: {e}")))
}

/// Starts scanning; pages arrive as `scan` events (`page`, then `end` or
/// `error`). `append` adds to the batch ("Scan more pages"); without it the
/// batch is replaced ("Rescan").
#[tauri::command]
pub fn scan_start(
    webview: Webview,
    app: AppHandle,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<Scans>>,
    device: Device,
    settings: Settings,
    append: bool,
) -> Answer {
    scan_page_only(&webview)?;
    if scans.busy.swap(true, Ordering::SeqCst) {
        return Err(refusal("busy", "A scan is already running."));
    }
    if !append {
        scans.discard_pages();
    }
    let _ = Preferences {
        settings: Some(settings.clone()),
        device: Some(device.id.clone()),
        ..Preferences::load(&desktop.paths)
    }
    .save(&desktop.paths);
    let dir = match scans.page_dir(&desktop.paths) {
        Ok(dir) => dir,
        Err(e) => {
            scans.busy.store(false, Ordering::SeqCst);
            return Err(refusal("disk", format!("Couldn't keep pages: {e}")));
        }
    };
    {
        let mut session = scans.session.lock().expect("session");
        session.scanner = Some(device.name.clone());
        // A changed batch is a new scan.
        session.scan_id = None;
    }
    let scans = scans.inner().clone();
    std::thread::spawn(move || {
        let request = Request::new(&device.id, &settings);
        let mut left_out = 0;
        let result = scans.with_helper(|helper| {
            *scans.canceller.lock().expect("canceller") = Some(helper.canceller());
            helper.scan(&request, &dir, &mut |file: PageFile| match arrived(
                &scans, &file, &settings,
            ) {
                Ok(page) => {
                    left_out += usize::from(page.blank);
                    tell(&app, json!({ "kind": "page", "page": page.json() }));
                }
                Err(e) => tell(&app, json!({ "kind": "error", "message": e })),
            })
        });
        scans.canceller.lock().expect("canceller").take();
        scans.busy.store(false, Ordering::SeqCst);
        match result {
            Ok(end) => tell(
                &app,
                json!({
                    "kind": "end",
                    "end": end,
                    "left_out": left_out,
                    "message": match end {
                        End::NoPaper => Some("Put the pages in the scanner's feeder, then scan again."),
                        _ => None,
                    },
                }),
            ),
            Err(e) => tell(
                &app,
                json!({ "kind": "error", "code": e.code, "message": e.message }),
            ),
        }
    });
    Ok(json!({}))
}

/// A page the helper wrote: judged blank or not, and thumbnailed.
fn arrived(scans: &Scans, file: &PageFile, settings: &Settings) -> Result<Page, String> {
    let image = image::open(&file.path).map_err(|_| "A page couldn't be read.".to_string())?;
    let blank = settings.remove_blank_pages && cww_scan::blank::is_blank(&image, file.resolution);
    let mut page = Page {
        id: scans.next_id.fetch_add(1, Ordering::SeqCst),
        path: file.path.clone(),
        resolution: file.resolution,
        color: settings.color,
        blank,
        edits: Edits::default(),
        thumb: String::new(),
        width: image.width(),
        height: image.height(),
    };
    page.thumb = data_url(&as_sent(&page, &image), THUMB, 70);
    scans
        .session
        .lock()
        .expect("session")
        .pages
        .push(page.clone());
    Ok(page)
}

#[tauri::command]
pub fn scan_cancel(webview: Webview, scans: State<'_, Arc<Scans>>) -> Answer {
    scan_page_only(&webview)?;
    if let Some(canceller) = scans.canceller.lock().expect("canceller").as_ref() {
        canceller.cancel();
    }
    Ok(json!({}))
}

fn find(session: &Session, id: u64) -> Result<Page, Value> {
    session
        .pages
        .iter()
        .find(|p| p.id == id)
        .cloned()
        .ok_or_else(|| refusal("not_found", "That page isn't in the scan any more."))
}

/// A larger picture of a page as scanned (turned, not cropped), for the
/// crop view.
#[tauri::command]
pub async fn scan_page(webview: Webview, scans: State<'_, Arc<Scans>>, id: u64) -> Answer {
    scan_page_only(&webview)?;
    let page = find(&scans.session.lock().expect("session"), id)?;
    blocking(move || {
        let image = load(&page)?;
        let turned = Edits {
            turns: page.edits.turns,
            crop: None,
        }
        .apply(&image);
        Ok(json!({ "image": data_url(&turned, PREVIEW, 82), "crop": page.edits.crop }))
    })
    .await?
}

/// Changes a page: `turn` once more clockwise, a new `crop` (or `whole`
/// for the whole page again), or `keep` a page judged blank.
#[tauri::command]
pub async fn scan_edit(
    webview: Webview,
    scans: State<'_, Arc<Scans>>,
    id: u64,
    turn: Option<bool>,
    crop: Option<cww_scan::edit::Crop>,
    whole: Option<bool>,
    keep: Option<bool>,
) -> Answer {
    scan_page_only(&webview)?;
    let mut page = find(&scans.session.lock().expect("session"), id)?;
    if turn == Some(true) {
        page.edits = page.edits.turned();
    }
    if whole == Some(true) {
        page.edits.crop = None;
    } else if let Some(crop) = crop {
        page.edits.crop = crop.normalized();
    }
    if let Some(keep) = keep {
        page.blank = !keep;
    }
    let scans = scans.inner().clone();
    blocking(move || {
        let image = load(&page)?;
        page.thumb = data_url(&as_sent(&page, &image), THUMB, 70);
        let mut session = scans.session.lock().expect("session");
        if let Some(slot) = session.pages.iter_mut().find(|p| p.id == page.id) {
            *slot = page.clone();
            session.scan_id = None;
        }
        Ok(json!({ "page": page.json() }))
    })
    .await?
}

/// Deletes a page, or moves it to `index` among all pages.
#[tauri::command]
pub fn scan_arrange(
    webview: Webview,
    scans: State<'_, Arc<Scans>>,
    id: u64,
    index: Option<usize>,
    delete: Option<bool>,
) -> Answer {
    scan_page_only(&webview)?;
    let mut session = scans.session.lock().expect("session");
    let at = session
        .pages
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| refusal("not_found", "That page isn't in the scan any more."))?;
    let page = session.pages.remove(at);
    if delete == Some(true) {
        let _ = std::fs::remove_file(&page.path);
    } else {
        let index = index.unwrap_or(at).min(session.pages.len());
        session.pages.insert(index, page);
    }
    session.scan_id = None;
    Ok(json!({ "pages": session.pages.iter().map(Page::json).collect::<Vec<_>>() }))
}

#[tauri::command]
pub fn scan_discard(webview: Webview, scans: State<'_, Arc<Scans>>) -> Answer {
    scan_page_only(&webview)?;
    if scans.busy.load(Ordering::SeqCst) {
        return Err(refusal("busy", "Stop the scan first."));
    }
    scans.discard_pages();
    Ok(json!({}))
}

/// Asks the daemon, waiting as long as it takes (an upload can take a
/// while), and keeps a refusal's code for the page.
fn daemon_call(paths: &crate::paths::Paths, request: &ControlRequest) -> Result<Value, Value> {
    let not_running = || {
        refusal(
            "not_running",
            "The background agent isn't running. Start it in Settings, then try again.",
        )
    };
    let mut client = cww::control::Client::connect(&paths.socket_path())
        .map_err(|e| refusal("refused", format!("{e:#}")))?
        .ok_or_else(not_running)?;
    let value = client
        .call_slow(request)
        .map_err(|e| refusal("refused", format!("{e:#}")))?;
    if value["ok"] == json!(true) {
        Ok(value)
    } else {
        Err(refusal(
            value["code"].as_str().unwrap_or("refused"),
            value["error"]
                .as_str()
                .unwrap_or("The background agent refused."),
        ))
    }
}

/// Where a scan can go, from the server: a new chat, and the bots that take
/// scans.
#[tauri::command]
pub async fn scan_destinations(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    scan_page_only(&webview)?;
    let paths = desktop.paths.clone();
    blocking(move || daemon_call(&paths, &ControlRequest::ScanDestinations)).await?
}

/// Makes the PDF (reading each page's text where asked) and sends it.
/// Progress arrives as `scan` events of kind `progress`.
#[tauri::command]
pub async fn scan_send(
    webview: Webview,
    app: AppHandle,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<Scans>>,
    destination: cww::scans::Destination,
    message: Option<String>,
    language: Option<String>,
) -> Answer {
    scan_page_only(&webview)?;
    if scans.busy.swap(true, Ordering::SeqCst) {
        return Err(refusal("busy", "Wait for the scan to finish."));
    }
    log::info!("sending a scan, reading text in {language:?}");
    let (pages, scanner, scan_id) = {
        let mut session = scans.session.lock().expect("session");
        let id = session.scan_id.get_or_insert_with(random_id).clone();
        (
            session
                .pages
                .iter()
                .filter(|p| !p.blank)
                .cloned()
                .collect::<Vec<_>>(),
            session.scanner.clone(),
            id,
        )
    };
    let scans_ = scans.inner().clone();
    let paths = desktop.paths.clone();
    let result = blocking(move || -> Result<Value, Value> {
        let pdf = make_pdf(&app, &scans_, &pages, language.as_deref())?;
        too_large(&pdf)?;
        tell(
            &app,
            json!({ "kind": "progress", "text": "Sending to Chat with Work…" }),
        );
        use base64::Engine as _;
        let request = ControlRequest::ScanSend {
            destination,
            filename: filename(pages.len()),
            data: base64::engine::general_purpose::STANDARD.encode(&pdf),
            pages: pages.len() as u32,
            scan_id,
            scanner,
            message: message.filter(|m| !m.trim().is_empty()),
        };
        daemon_call(&paths, &request)
    })
    .await;
    scans.busy.store(false, Ordering::SeqCst);
    // The pages stay until Rescan or the window closes, so the same scan can
    // also be saved, or sent somewhere else.
    result?
}

/// Makes the PDF and hands it to Chat with Work's own composer, then
/// closes the window: the composer that asked for a scan (its attach menu),
/// or else a new chat's, in the Chat with Work window. The person writes
/// their message there, with everything the composer offers, and the file
/// is uploaded from there like any other.
#[tauri::command]
pub async fn scan_attach(
    webview: Webview,
    app: AppHandle,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<Scans>>,
    language: Option<String>,
) -> Answer {
    scan_page_only(&webview)?;
    let pages = kept_pages(&scans);
    if pages.is_empty() {
        return Err(refusal("invalid", "There are no pages to attach."));
    }
    if scans.busy.swap(true, Ordering::SeqCst) {
        return Err(refusal("busy", "Wait for the scan to finish."));
    }
    let count = pages.len();
    let scans_ = scans.inner().clone();
    let maker = app.clone();
    let result = blocking(move || -> Result<Vec<u8>, Value> {
        let pdf = make_pdf(&maker, &scans_, &pages, language.as_deref())?;
        too_large(&pdf)?;
        Ok(pdf)
    })
    .await;
    scans.busy.store(false, Ordering::SeqCst);
    let pdf = result??;
    let id = random_id();
    let answer = json!({ "id": id, "name": filename(count), "pages": count, "size": pdf.len() });
    let waiting = scans.attaching.lock().expect("attaching").take();
    let label = waiting
        .as_ref()
        .map_or(crate::windows::MAIN.to_string(), |w| w.label.clone());
    *scans.attached.lock().expect("attached") = Some(Attached {
        made: std::time::Instant::now(),
        id,
        label: label.clone(),
        answer: answer.clone(),
        pdf,
    });
    match waiting {
        Some(waiting) => {
            let _ = waiting.reply.send(Some(answer));
            if let Some(window) = app.get_webview(&label) {
                let _ = window.set_focus();
            }
        }
        // A new chat, whose composer takes the scan as it opens.
        None => {
            let opener = app.clone();
            let start = desktop.server().start_url();
            std::thread::spawn(move || {
                let _ = crate::windows::show_main(&opener, Some(start));
            });
        }
    }
    // Closing the window deletes the pages.
    if let Some(window) = app.get_webview_window(SCAN) {
        let _ = window.close();
    }
    Ok(json!({}))
}

/// Chat with Work takes files of at most 25 MB.
fn too_large(pdf: &[u8]) -> Result<(), Value> {
    if pdf.len() <= MAX_PDF_BYTES {
        return Ok(());
    }
    Err(refusal(
        "too_large",
        format!(
            "This scan is {:.1} MB, and Chat with Work takes at most {} MB. Scan in grayscale or black and white, at a lower resolution, or send fewer pages.",
            pdf.len() as f64 / 1_048_576.0,
            MAX_PDF_BYTES / 1_048_576
        ),
    ))
}

/// The pages to send or save: all but the blank ones left out.
fn kept_pages(scans: &Scans) -> Vec<Page> {
    let session = scans.session.lock().expect("session");
    session.pages.iter().filter(|p| !p.blank).cloned().collect()
}

/// The PDF of the pages, read in `language` where given, telling the page
/// how far it got.
fn make_pdf(
    app: &AppHandle,
    scans: &Scans,
    pages: &[Page],
    language: Option<&str>,
) -> Result<Vec<u8>, Value> {
    if pages.is_empty() {
        return Err(refusal("invalid", "There are no pages."));
    }
    let engine = match language {
        Some(code) => Some((
            scans
                .engine
                .clone()
                .map_err(|reason| refusal("no_ocr", reason))?,
            code,
        )),
        None => None,
    };
    build_pdf(
        pages,
        engine.as_ref().map(|(e, l)| (e.as_ref(), *l)),
        &|text| {
            tell(app, json!({ "kind": "progress", "text": text }));
        },
    )
}

/// Saves the PDF on this computer, where the person picks, without Chat
/// with Work: no pairing, no sign-in, nothing sent. The dialog starts in the
/// folder saved to last (Documents the first time) and remembers the one
/// picked.
#[tauri::command]
pub async fn scan_save(
    webview: Webview,
    app: AppHandle,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<Scans>>,
    language: Option<String>,
) -> Answer {
    scan_page_only(&webview)?;
    let pages = kept_pages(&scans);
    if pages.is_empty() {
        return Err(refusal("invalid", "There are no pages to save."));
    }
    use tauri_plugin_dialog::DialogExt;
    let paths = desktop.paths.clone();
    let start = Preferences::load(&paths)
        .save_dir
        .filter(|dir| dir.is_dir())
        .or_else(crate::paths::documents_dir);
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save Scan")
        .set_file_name(filename(pages.len()))
        .add_filter("PDF", &["pdf"]);
    if let Some(dir) = start {
        dialog = dialog.set_directory(dir);
    }
    // Demo builds can answer the dialog for pictures and checks, which can't
    // click it (`CWW_DEMO_SAVE_TO=<file>`); real builds always ask.
    #[cfg(feature = "demo")]
    let preset = std::env::var_os("CWW_DEMO_SAVE_TO").map(PathBuf::from);
    #[cfg(not(feature = "demo"))]
    let preset: Option<PathBuf> = None;
    let picked = match preset {
        Some(path) => Some(path),
        None => blocking(move || dialog.blocking_save_file())
            .await?
            .and_then(|p| p.into_path().ok()),
    };
    let Some(path) = picked else {
        return Ok(json!({ "saved": null }));
    };
    let path = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        path
    } else {
        path.with_extension("pdf")
    };
    if scans.busy.swap(true, Ordering::SeqCst) {
        return Err(refusal("busy", "Wait for the scan to finish."));
    }
    let scans_ = scans.inner().clone();
    let target = path.clone();
    let result = blocking(move || -> Result<(), Value> {
        let pdf = make_pdf(&app, &scans_, &pages, language.as_deref())?;
        tell(&app, json!({ "kind": "progress", "text": "Saving…" }));
        std::fs::write(&target, pdf)
            .map_err(|e| refusal("failed", format!("Couldn't save the scan: {e}")))
    })
    .await;
    scans.busy.store(false, Ordering::SeqCst);
    result??;
    let _ = Preferences {
        save_dir: path.parent().map(PathBuf::from),
        ..Preferences::load(&paths)
    }
    .save(&paths);
    Ok(json!({ "saved": path, "name": path.file_name().map(|n| n.to_string_lossy().into_owned()) }))
}

/// Opens the settings on Account, where the computer is paired, for a scan
/// page that can't send yet.
#[tauri::command]
pub fn scan_open_account(webview: Webview, desktop: State<'_, Arc<Desktop>>) -> Answer {
    scan_page_only(&webview)?;
    let app = desktop.app();
    std::thread::spawn(move || {
        let _ = crate::windows::show_settings(&app, Some("account"));
    });
    Ok(json!({}))
}

/// Shows a saved scan's folder: only the folder the window saved to last.
#[tauri::command]
pub fn scan_reveal(webview: Webview, desktop: State<'_, Arc<Desktop>>, path: PathBuf) -> Answer {
    scan_page_only(&webview)?;
    let saved_to = Preferences::load(&desktop.paths).save_dir;
    match path.parent() {
        Some(dir) if saved_to.as_deref() == Some(dir) => {
            let _ = open::that_detached(dir);
            Ok(json!({}))
        }
        _ => Err(refusal("invalid", "That isn't a scan this window saved.")),
    }
}

/// "Scan 2026-10-22 14.05, 3 pages.pdf", in local time.
fn filename(pages: usize) -> String {
    let now = jiff::Zoned::now();
    let noun = if pages == 1 { "page" } else { "pages" };
    format!(
        "Scan {}, {pages} {noun}.pdf",
        now.strftime("%Y-%m-%d %H.%M")
    )
}

/// The PDF of `pages`, read with `ocr` where given. Pages are read a few at
/// a time, in parallel; Tesseract takes a second or two a page.
fn build_pdf(
    pages: &[Page],
    ocr: Option<(&dyn Engine, &str)>,
    progress: &dyn Fn(String),
) -> Result<Vec<u8>, Value> {
    let mut builder = cww_scan::pdf::Builder::new();
    let at_once = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(1, 4));
    let total = pages.len();
    for (batch, chunk) in pages.chunks(at_once).enumerate() {
        let first = batch * at_once + 1;
        progress(match ocr {
            Some(_) if chunk.len() > 1 => format!(
                "Making pages {first} to {} of {total} searchable…",
                first + chunk.len() - 1
            ),
            Some(_) => format!("Making page {first} of {total} searchable…"),
            None => format!("Putting page {first} of {total} in the PDF…"),
        });
        let done: Vec<Result<(DynamicImage, Vec<cww_scan::ocr::Word>), Value>> =
            std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|page| {
                        scope.spawn(move || {
                            let edited = page.edits.apply(&load(page)?);
                            let words = match ocr {
                                Some((engine, language)) => engine
                                    .recognize(&edited, page.resolution, language)
                                    .map_err(|reason| refusal("ocr_failed", reason))?,
                                None => vec![],
                            };
                            log::info!("scan page {}: {} words read", page.id, words.len());
                            Ok((edited, words))
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join()
                            .unwrap_or_else(|_| Err(refusal("failed", "Reading a page failed.")))
                    })
                    .collect()
            });
        for (page, result) in chunk.iter().zip(done) {
            let (image, words) = result?;
            builder
                .page(&image, page.resolution, page.color, &words)
                .map_err(|e| refusal("failed", format!("Couldn't make the PDF: {e}")))?;
        }
    }
    Ok(builder.finish())
}

/// Opens a chat the scan went to, in the Chat with Work window: only a page
/// of the paired server.
#[tauri::command]
pub fn scan_open(webview: Webview, desktop: State<'_, Arc<Desktop>>, url: String) -> Answer {
    scan_page_only(&webview)?;
    let url = tauri::Url::parse(&url)
        .map_err(|_| refusal("invalid", "That isn't a page of Chat with Work."))?;
    if !desktop.server().owns(&url) {
        return Err(refusal("invalid", "That isn't a page of Chat with Work."));
    }
    let app = desktop.app();
    std::thread::spawn(move || {
        let _ = crate::windows::show_main(&app, Some(url));
    });
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    #[test]
    fn languages_to_add_leave_out_the_ones_here_and_go_by_name() {
        let packs = super::packs_to_add(&["eng".into(), "deu".into()]);
        let codes: Vec<&str> = packs.iter().map(|p| p["code"].as_str().unwrap()).collect();
        assert!(!codes.contains(&"eng") && !codes.contains(&"deu"));
        assert!(codes.contains(&"ita"));
        let names: Vec<&str> = packs.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert!(names.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(names[0], "Afrikaans");
    }

    use super::*;
    use cww_scan::ocr::Word;

    fn lang(code: &str) -> Language {
        Language {
            code: code.into(),
            name: code.into(),
        }
    }

    #[test]
    fn the_first_language_follows_the_system_where_installed() {
        let installed = [lang("deu"), lang("eng")];
        // SAFETY: this test alone reads and writes the locale variables.
        unsafe {
            std::env::remove_var("LC_ALL");
            std::env::remove_var("LC_MESSAGES");
            std::env::set_var("LANG", "de_DE.UTF-8");
        }
        assert_eq!(default_language(&installed).as_deref(), Some("deu"));
        assert_eq!(default_language(&[lang("eng")]).as_deref(), Some("eng"));
        assert_eq!(default_language(&[lang("ita")]).as_deref(), Some("ita"));
        assert_eq!(default_language(&[]), None);
        // Vision's codes on a Mac.
        assert_eq!(
            default_language(&[lang("en-US"), lang("de-DE")]).as_deref(),
            Some("de-DE")
        );
    }

    #[test]
    fn preferences_are_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::under(dir.path());
        assert_eq!(Preferences::load(&paths), Preferences::default());
        let saved = Preferences {
            settings: Some(Settings::default()),
            device: Some("test:0".into()),
            save_dir: Some(dir.path().into()),
        };
        saved.save(&paths).unwrap();
        assert_eq!(Preferences::load(&paths), saved);
    }

    struct Fake;
    impl Engine for Fake {
        fn languages(&self) -> Result<Vec<Language>, String> {
            Ok(vec![lang("eng")])
        }
        fn recognize(&self, page: &DynamicImage, _: u32, _: &str) -> Result<Vec<Word>, String> {
            Ok(vec![Word {
                text: format!("w{}", page.width()),
                x: 1,
                y: 1,
                width: 10,
                height: 5,
            }])
        }
    }

    #[test]
    fn builds_the_pdf_from_edited_pages_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let pages: Vec<Page> = (0..5u32)
            .map(|i| {
                let path = dir.path().join(format!("{i}.pnm"));
                image::GrayImage::from_pixel(100 + i, 140, image::Luma([255]))
                    .save_with_format(&path, image::ImageFormat::Pnm)
                    .unwrap();
                Page {
                    id: u64::from(i),
                    path,
                    resolution: 100,
                    color: ColorMode::Grayscale,
                    blank: false,
                    edits: Edits {
                        turns: u8::from(i == 2),
                        crop: None,
                    },
                    thumb: String::new(),
                    width: 100 + i,
                    height: 140,
                }
            })
            .collect();
        let said = Mutex::new(vec![]);
        let pdf = build_pdf(&pages, Some((&Fake, "eng")), &|text| {
            said.lock().unwrap().push(text)
        })
        .unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Count 5"));
        // The third page was turned: 140 x 102 px.
        assert!(text.contains("/MediaBox [0 0 100.8 73.44]"), "{text}");
        assert!(said.lock().unwrap()[0].contains("searchable"));
    }

    #[test]
    fn a_failed_scan_request_resolves_only_itself() {
        let scans = Scans::default();
        let (first, mut first_answer) = scans.attach_for("main");
        let (second, mut second_answer) = scans.attach_for("main");
        // The newer request answered the older one with nothing.
        assert_eq!(first_answer.try_recv(), Ok(None));
        scans.cancel_attach(first);
        assert!(
            scans.is_attaching(),
            "a stale cancel leaves the newer request"
        );
        scans.cancel_attach(second);
        assert!(!scans.is_attaching());
        assert_eq!(second_answer.try_recv(), Ok(None));
    }

    #[test]
    fn filenames_name_the_time() {
        let name = filename(3);
        assert!(
            name.starts_with("Scan 20") && name.ends_with(", 3 pages.pdf"),
            "{name}"
        );
        assert!(filename(1).ends_with(", 1 page.pdf"));
        assert!(!name.contains('/') && !name.contains(':'));
    }
}
