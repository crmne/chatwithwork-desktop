//! The scan window: scan paper with a scanner attached to this computer,
//! check and fix the pages, and send them to Chat with Work as one
//! searchable PDF (cww-scan does the scanning, OCR and PDF).
//!
//! - The scanner is driven by a helper process (`cww-app --scan-helper`),
//!   so a driver that crashes or hangs never takes the app with it. It runs
//!   while the window is open and goes with it.
//! - Pages are kept in a private folder of the app's own (0700, under the
//!   app's state directory) until they're sent or the window closes, then
//!   deleted. Nothing leaves the computer before Send.
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
use tauri::{AppHandle, State, Webview};

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

/// What the scan window shares.
pub struct Scans {
    session: Mutex<Session>,
    helper: Mutex<Option<Helper>>,
    canceller: Mutex<Option<Canceller>>,
    busy: AtomicBool,
    next_id: AtomicU64,
    engine: Result<Arc<dyn Engine>, String>,
}

impl Default for Scans {
    fn default() -> Self {
        Self {
            session: Mutex::default(),
            helper: Mutex::default(),
            canceller: Mutex::default(),
            busy: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            engine: cww_scan::ocr::system_engine().map(Arc::from),
        }
    }
}

/// Where sessions keep their pages: under the app's state directory, never
/// a shared temporary folder.
pub fn scans_root(paths: &crate::paths::Paths) -> PathBuf {
    paths.daemon.state_dir.join("scans")
}

impl Scans {
    /// Deletes the pages and stops the helper: the window closed, or the
    /// app starts (a crash may have left a session behind).
    pub fn close(&self, paths: &crate::paths::Paths) {
        if let Some(canceller) = self.canceller.lock().expect("canceller").take() {
            canceller.cancel();
        }
        *self.helper.lock().expect("helper") = None;
        *self.session.lock().expect("session") = Session::default();
        let _ = std::fs::remove_dir_all(scans_root(paths));
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
            let exe = std::env::current_exe()
                .map_err(|e| ScanError::new("helper_stopped", e.to_string()))?;
            let mut command = std::process::Command::new(exe);
            command.arg("--scan-helper");
            *slot = Some(Helper::spawn(command)?);
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
fn default_language(languages: &[Language]) -> Option<String> {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    let wanted = match locale.get(..2).unwrap_or_default() {
        "de" => "deu",
        "fr" => "fra",
        "it" => "ita",
        "es" => "spa",
        "nl" => "nld",
        "pt" => "por",
        "pl" => "pol",
        _ => "eng",
    };
    [wanted, "eng"]
        .iter()
        .find(|code| languages.iter().any(|l| l.code == **code))
        .map(|code| code.to_string())
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

#[tauri::command]
pub fn scan_info(
    webview: Webview,
    desktop: State<'_, Arc<Desktop>>,
    scans: State<'_, Arc<Scans>>,
) -> Answer {
    scan_page_only(&webview)?;
    let languages = match &scans.engine {
        Ok(engine) => engine.languages().unwrap_or_default(),
        Err(_) => vec![],
    };
    let preferences = Preferences::load(&desktop.paths);
    let mut settings = preferences.settings.clone().unwrap_or_default();
    // A language no longer installed falls back to one that is.
    if settings
        .ocr
        .as_ref()
        .is_some_and(|code| !languages.iter().any(|l| &l.code == code))
        || preferences.settings.is_none()
    {
        let on = preferences
            .settings
            .as_ref()
            .is_none_or(|s| s.ocr.is_some());
        settings.ocr = default_language(&languages).filter(|_| on);
    }
    let session = scans.session.lock().expect("session");
    Ok(json!({
        "settings": settings,
        "device": preferences.device,
        "languages": languages,
        "ocr": match &scans.engine {
            Ok(_) if languages.is_empty() => json!({ "available": false, "reason": cww_scan::ocr::TESSERACT_MISSING }),
            Ok(_) => json!({ "available": true }),
            Err(reason) => json!({ "available": false, "reason": reason }),
        },
        "pages": session.pages.iter().map(Page::json).collect::<Vec<_>>(),
        "busy": scans.busy.load(Ordering::SeqCst),
        "server": desktop.server().origin(),
        "max_bytes": MAX_PDF_BYTES,
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
        if pages.is_empty() {
            return Err(refusal("invalid", "There are no pages to send."));
        }
        let engine = match &language {
            Some(code) => Some((scans_.engine.clone().map_err(|reason| refusal("no_ocr", reason))?, code.clone())),
            None => None,
        };
        let pdf = build_pdf(&pages, engine.as_ref().map(|(e, l)| (e.as_ref(), l.as_str())), &|text| {
            tell(&app, json!({ "kind": "progress", "text": text }));
        })?;
        if pdf.len() > MAX_PDF_BYTES {
            return Err(refusal(
                "too_large",
                format!(
                    "This scan is {:.1} MB, and Chat with Work takes at most {} MB. Scan in grayscale or black and white, at a lower resolution, or send fewer pages.",
                    pdf.len() as f64 / 1_048_576.0,
                    MAX_PDF_BYTES / 1_048_576
                ),
            ));
        }
        tell(&app, json!({ "kind": "progress", "text": "Sending to Chat with Work…" }));
        use base64::Engine as _;
        let request = ControlRequest::ScanSend {
            destination,
            filename: filename(),
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
    let answer = result??;
    // Sent: the pages leave the computer's disk too.
    scans.discard_pages();
    Ok(answer)
}

/// "Scan 2026-10-22 14.05.pdf", in local time.
fn filename() -> String {
    let now = jiff::Zoned::now();
    format!("Scan {}.pdf", now.strftime("%Y-%m-%d %H.%M"))
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
    }

    #[test]
    fn preferences_are_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::under(dir.path());
        assert_eq!(Preferences::load(&paths), Preferences::default());
        let saved = Preferences {
            settings: Some(Settings::default()),
            device: Some("test:0".into()),
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
    fn filenames_name_the_time() {
        let name = filename();
        assert!(name.starts_with("Scan 20") && name.ends_with(".pdf"));
        assert!(!name.contains('/') && !name.contains(':'));
    }
}
