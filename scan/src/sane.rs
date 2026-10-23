//! SANE, on Linux (the distribution's `libsane.so.1`) and on macOS
//! (Homebrew's `libsane.1.dylib`, for scanners whose makers ship no
//! ImageCapture driver, such as Canon's P-208II), loaded at run time.
//!
//! No maintained Rust crate wraps SANE, and the dozen functions a frontend
//! needs are a small, stable C API (SANE 1.0), so this is our own binding.
//! libsane is never shipped: its backends are GPL with an exception for
//! frontends that load them, and a computer without SANE still runs the app
//! (the scan page says what to install).
//!
//! SANE allows one `sane_init` per process and isn't thread-safe, so one
//! [`Sane`] lives in the scan helper process and every call is made on the
//! thread that holds it. `sane_cancel` is the exception the standard makes:
//! it may be called at any time, which is how a scan is stopped.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use image::{DynamicImage, GrayImage, RgbImage};
use libloading::Library;

use crate::{Backend, Capabilities, Device, End, Request, ScanError, Scanned};

type Status = c_int;
type Handle = *mut c_void;
type Word = c_int;

const GOOD: Status = 0;
const CANCELLED: Status = 2;
const DEVICE_BUSY: Status = 3;
const EOF: Status = 5;
const JAMMED: Status = 6;
const NO_DOCS: Status = 7;
const COVER_OPEN: Status = 8;
const ACCESS_DENIED: Status = 11;

const TYPE_BOOL: c_int = 0;
const TYPE_INT: c_int = 1;
const TYPE_FIXED: c_int = 2;
const TYPE_STRING: c_int = 3;

const CONSTRAINT_RANGE: c_int = 1;
const CONSTRAINT_WORD_LIST: c_int = 2;
const CONSTRAINT_STRING_LIST: c_int = 3;

const CAP_SOFT_SELECT: c_int = 1;
const CAP_INACTIVE: c_int = 32;

const ACTION_GET: c_int = 0;
const ACTION_SET: c_int = 1;

const FRAME_GRAY: c_int = 0;
const FRAME_RGB: c_int = 1;
const FRAME_RED: c_int = 2;
const FRAME_GREEN: c_int = 3;
const FRAME_BLUE: c_int = 4;

#[repr(C)]
struct RawDevice {
    name: *const c_char,
    vendor: *const c_char,
    model: *const c_char,
    kind: *const c_char,
}

#[repr(C)]
struct Range {
    min: Word,
    max: Word,
    quant: Word,
}

#[repr(C)]
struct OptionDescriptor {
    name: *const c_char,
    title: *const c_char,
    desc: *const c_char,
    kind: c_int,
    unit: c_int,
    size: c_int,
    cap: c_int,
    constraint_type: c_int,
    constraint: *const c_void,
}

#[repr(C)]
#[derive(Default)]
struct Parameters {
    format: c_int,
    last_frame: Word,
    bytes_per_line: c_int,
    pixels_per_line: c_int,
    lines: c_int,
    depth: c_int,
}

type AuthCallback = Option<unsafe extern "C" fn(*const c_char, *mut c_char, *mut c_char)>;

struct Api {
    init: unsafe extern "C" fn(*mut c_int, AuthCallback) -> Status,
    exit: unsafe extern "C" fn(),
    get_devices: unsafe extern "C" fn(*mut *const *const RawDevice, Word) -> Status,
    open: unsafe extern "C" fn(*const c_char, *mut Handle) -> Status,
    close: unsafe extern "C" fn(Handle),
    get_option_descriptor: unsafe extern "C" fn(Handle, c_int) -> *const OptionDescriptor,
    control_option: unsafe extern "C" fn(Handle, c_int, c_int, *mut c_void, *mut c_int) -> Status,
    get_parameters: unsafe extern "C" fn(Handle, *mut Parameters) -> Status,
    start: unsafe extern "C" fn(Handle) -> Status,
    read: unsafe extern "C" fn(Handle, *mut u8, c_int, *mut c_int) -> Status,
    cancel: unsafe extern "C" fn(Handle),
    strstatus: unsafe extern "C" fn(Status) -> *const c_char,
}

/// The library's names, newest first.
#[cfg(target_os = "linux")]
const LIBRARIES: &[&str] = &["libsane.so.1", "libsane.so"];
/// Homebrew's, on Apple silicon and on Intel, then whatever the loader finds.
#[cfg(target_os = "macos")]
const LIBRARIES: &[&str] = &[
    "/opt/homebrew/lib/libsane.1.dylib",
    "/usr/local/lib/libsane.1.dylib",
    "/opt/local/lib/libsane.1.dylib",
    "libsane.1.dylib",
];

/// SANE, initialised. Dropping it calls `sane_exit`.
pub struct Sane {
    api: Api,
    // Kept loaded for as long as the function pointers in `api` are used.
    _library: Library,
}

fn missing() -> ScanError {
    ScanError::new(
        "no_sane",
        if cfg!(target_os = "macos") {
            "This scanner needs SANE on a Mac: install it with Homebrew (brew install sane-backends), then look for scanners again."
        } else {
            "Scanning needs SANE. Install your distribution's sane package (sane-backends, libsane1 or sane), then open this page again."
        },
    )
}

impl Sane {
    pub fn load() -> Result<Self, ScanError> {
        // SAFETY: libsane runs no code on load that needs anything from us.
        let library = LIBRARIES
            .iter()
            .find_map(|name| unsafe { Library::new(name) }.ok())
            .ok_or_else(missing)?;
        // SAFETY: every symbol has the SANE 1.0 signature it is declared
        // with here, and `library` outlives `api` inside `Sane`.
        let api = unsafe {
            macro_rules! sym {
                ($name:literal) => {
                    *library
                        .get(concat!($name, "\0").as_bytes())
                        .map_err(|_| missing())?
                };
            }
            Api {
                init: sym!("sane_init"),
                exit: sym!("sane_exit"),
                get_devices: sym!("sane_get_devices"),
                open: sym!("sane_open"),
                close: sym!("sane_close"),
                get_option_descriptor: sym!("sane_get_option_descriptor"),
                control_option: sym!("sane_control_option"),
                get_parameters: sym!("sane_get_parameters"),
                start: sym!("sane_start"),
                read: sym!("sane_read"),
                cancel: sym!("sane_cancel"),
                strstatus: sym!("sane_strstatus"),
            }
        };
        let mut version = 0;
        // SAFETY: called once for this library; no authorisation callback
        // (USB scanners don't ask, and we never prompt for network ones).
        let status = unsafe { (api.init)(&mut version, None) };
        if status != GOOD {
            return Err(ScanError::new("no_sane", "SANE couldn't start."));
        }
        Ok(Self {
            api,
            _library: library,
        })
    }

    fn status_text(&self, status: Status) -> String {
        // SAFETY: sane_strstatus returns a static string or null.
        let text = unsafe { (self.api.strstatus)(status) };
        if text.is_null() {
            format!("status {status}")
        } else {
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        }
    }

    fn failure(&self, status: Status, doing: &str) -> ScanError {
        match status {
            JAMMED => ScanError::new(
                "jammed",
                "The paper jammed. Clear it from the scanner and scan again.",
            ),
            COVER_OPEN => ScanError::new(
                "cover_open",
                "The scanner's cover is open. Close it and scan again.",
            ),
            DEVICE_BUSY => ScanError::new(
                "busy",
                "The scanner is busy, perhaps with another program. Close it and try again.",
            ),
            ACCESS_DENIED => ScanError::new(
                "access_denied",
                "This user can't use the scanner. Your distribution's sane package sets that up; you may need to plug it in again or log in again.",
            ),
            _ => ScanError::new(
                "scanner_error",
                format!(
                    "The scanner failed while {doing}: {}.",
                    self.status_text(status)
                ),
            ),
        }
    }

    fn open(&self, device: &str) -> Result<Open<'_>, ScanError> {
        let name = CString::new(device)
            .map_err(|_| ScanError::new("invalid", "That isn't a scanner's name."))?;
        let mut handle: Handle = std::ptr::null_mut();
        // SAFETY: a valid C string and an out pointer.
        let status = unsafe { (self.api.open)(name.as_ptr(), &mut handle) };
        if status != GOOD || handle.is_null() {
            return Err(match status {
                GOOD | 1 | 4 => ScanError::new(
                    "not_found",
                    "That scanner isn't connected any more. Plug it in and look for scanners again.",
                ),
                other => self.failure(other, "opening it"),
            });
        }
        Ok(Open { sane: self, handle })
    }
}

impl Drop for Sane {
    fn drop(&mut self) {
        // SAFETY: after every handle closed (they borrow `self`).
        unsafe { (self.api.exit)() }
    }
}

/// An option as SANE describes it.
struct Opt {
    index: c_int,
    kind: c_int,
    size: c_int,
    cap: c_int,
    constraint: Constraint,
}

enum Constraint {
    None,
    Range { min: i32, max: i32, quant: i32 },
    Words(Vec<i32>),
    Strings(Vec<String>),
}

impl Opt {
    fn settable(&self) -> bool {
        self.cap & CAP_SOFT_SELECT != 0 && self.cap & CAP_INACTIVE == 0
    }

    fn strings(&self) -> &[String] {
        match &self.constraint {
            Constraint::Strings(list) => list,
            _ => &[],
        }
    }

    /// A number in the option's own units (SANE_Fixed is 16.16).
    fn number(&self, raw: i32) -> f64 {
        if self.kind == TYPE_FIXED {
            f64::from(raw) / 65536.0
        } else {
            f64::from(raw)
        }
    }

    fn raw(&self, value: f64) -> i32 {
        if self.kind == TYPE_FIXED {
            (value * 65536.0).round() as i32
        } else {
            value.round() as i32
        }
    }
}

/// An open scanner. Closed when dropped.
struct Open<'a> {
    sane: &'a Sane,
    handle: Handle,
}

impl Drop for Open<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle came from sane_open and is closed once.
        unsafe {
            (self.sane.api.cancel)(self.handle);
            (self.sane.api.close)(self.handle);
        }
    }
}

/// Resolutions offered from a range, as scanning programs commonly do.
const COMMON_RESOLUTIONS: [u32; 9] = [75, 100, 150, 200, 240, 300, 400, 600, 1200];

impl Open<'_> {
    fn option(&self, name: &str) -> Option<Opt> {
        let count = self.count();
        (1..count).find_map(|index| {
            // SAFETY: index is below the option count.
            let desc = unsafe { (self.sane.api.get_option_descriptor)(self.handle, index) };
            if desc.is_null() {
                return None;
            }
            // SAFETY: SANE keeps the descriptor valid until options reload.
            let desc = unsafe { &*desc };
            if desc.name.is_null()
                || unsafe { CStr::from_ptr(desc.name) }.to_bytes() != name.as_bytes()
            {
                return None;
            }
            Some(Opt {
                index,
                kind: desc.kind,
                size: desc.size,
                cap: desc.cap,
                constraint: unsafe { constraint(desc) },
            })
        })
    }

    fn count(&self) -> c_int {
        let mut count: Word = 0;
        // SAFETY: option 0 is the option count, an SANE_Int.
        let status = unsafe {
            (self.sane.api.control_option)(
                self.handle,
                0,
                ACTION_GET,
                (&mut count as *mut Word).cast(),
                std::ptr::null_mut(),
            )
        };
        if status == GOOD { count } else { 0 }
    }

    fn get_word(&self, opt: &Opt) -> Option<i32> {
        if !matches!(opt.kind, TYPE_INT | TYPE_FIXED | TYPE_BOOL) || opt.size < 4 {
            return None;
        }
        let mut words = vec![0 as Word; (opt.size as usize / 4).max(1)];
        // SAFETY: a buffer of the option's size.
        let status = unsafe {
            (self.sane.api.control_option)(
                self.handle,
                opt.index,
                ACTION_GET,
                words.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            )
        };
        (status == GOOD).then(|| words[0])
    }

    fn set_word(&self, opt: &Opt, value: i32) -> bool {
        if !opt.settable() || !matches!(opt.kind, TYPE_INT | TYPE_FIXED | TYPE_BOOL) {
            return false;
        }
        let mut words = vec![value as Word; (opt.size as usize / 4).max(1)];
        // SAFETY: a buffer of the option's size.
        let status = unsafe {
            (self.sane.api.control_option)(
                self.handle,
                opt.index,
                ACTION_SET,
                words.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            )
        };
        status == GOOD
    }

    fn set_string(&self, opt: &Opt, value: &str) -> bool {
        if !opt.settable() || opt.kind != TYPE_STRING {
            return false;
        }
        let mut buffer = vec![0u8; (opt.size as usize).max(value.len() + 1)];
        buffer[..value.len()].copy_from_slice(value.as_bytes());
        // SAFETY: a nul-terminated buffer at least the option's size.
        let status = unsafe {
            (self.sane.api.control_option)(
                self.handle,
                opt.index,
                ACTION_SET,
                buffer.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            )
        };
        status == GOOD
    }

    /// Sets a named option to a number in its own units, if it has one.
    fn set_number(&self, name: &str, value: f64) -> bool {
        self.option(name)
            .is_some_and(|opt| self.set_word(&opt, opt.raw(value)))
    }

    /// Sets a named number option to the largest (or smallest) it takes.
    fn set_extreme(&self, name: &str, largest: bool) {
        if let Some(opt) = self.option(name)
            && let Constraint::Range { min, max, .. } = opt.constraint
        {
            self.set_word(&opt, if largest { max } else { min });
        }
    }

    fn set_bool(&self, name: &str, on: bool) {
        if let Some(opt) = self.option(name)
            && opt.kind == TYPE_BOOL
        {
            self.set_word(&opt, i32::from(on));
        }
    }

    fn capabilities(&self) -> Capabilities {
        let resolutions = match self.option("resolution") {
            Some(opt) => match &opt.constraint {
                Constraint::Words(words) => {
                    let mut list: Vec<u32> = words
                        .iter()
                        .map(|w| opt.number(*w).round() as u32)
                        .filter(|r| *r > 0)
                        .collect();
                    list.sort_unstable();
                    list.dedup();
                    list
                }
                Constraint::Range { min, max, quant } => {
                    let (min, max) = (opt.number(*min), opt.number(*max));
                    let quant = opt.number(*quant);
                    COMMON_RESOLUTIONS
                        .iter()
                        .copied()
                        .filter(|r| {
                            let r = f64::from(*r);
                            r >= min
                                && r <= max
                                && (quant <= 1.0 || ((r - min) / quant).fract().abs() < 1e-6)
                        })
                        .collect()
                }
                _ => vec![],
            },
            None => vec![],
        };
        let modes = self
            .option("mode")
            .map(|o| o.strings().to_vec())
            .unwrap_or_default();
        let sources = self
            .option("source")
            .map(|o| o.strings().to_vec())
            .unwrap_or_default();
        Capabilities {
            resolutions,
            color: modes.iter().any(|m| is_color_mode(m)) || modes.is_empty(),
            feeder: sources.iter().any(|s| is_feeder(s)),
            duplex: sources.iter().any(|s| is_duplex(s)),
            flatbed: sources.is_empty() || sources.iter().any(|s| !is_feeder(s)),
        }
    }

    /// Sets everything a request asks for. Returns whether the source is a
    /// feeder, which scans until it runs out of paper.
    fn configure(&self, request: &Request) -> bool {
        if let Some(opt) = self.option("source")
            && let Some(source) = pick_source(opt.strings(), request.duplex)
        {
            self.set_string(&opt, &source);
        }
        if let Some(opt) = self.option("mode") {
            let wanted = opt
                .strings()
                .iter()
                .find(|m| {
                    if request.color {
                        is_color_mode(m)
                    } else {
                        is_gray_mode(m)
                    }
                })
                .cloned();
            if let Some(mode) = wanted {
                self.set_string(&opt, &mode);
            }
        }
        if let Some(opt) = self.option("depth") {
            self.set_word(&opt, 8);
        }
        self.set_number("resolution", f64::from(request.resolution));
        // The whole width and length the scanner takes: a feeder's page
        // ends where the paper does (or the backend's own crop finds it).
        self.set_extreme("page-width", true);
        self.set_extreme("page-height", true);
        self.set_extreme("tl-x", false);
        self.set_extreme("tl-y", false);
        self.set_extreme("br-x", true);
        self.set_extreme("br-y", true);
        // Software options some backends offer for small scanners (canon_dr,
        // fujitsu): find the paper's edges and straighten it.
        self.set_bool("swcrop", true);
        self.set_bool("swdeskew", true);
        self.option("source")
            .and_then(|opt| {
                // What the source is now, after setting it.
                let mut buffer = vec![0u8; opt.size.max(1) as usize];
                // SAFETY: a buffer of the option's size.
                let status = unsafe {
                    (self.sane.api.control_option)(
                        self.handle,
                        opt.index,
                        ACTION_GET,
                        buffer.as_mut_ptr().cast(),
                        std::ptr::null_mut(),
                    )
                };
                (status == GOOD).then(|| {
                    CStr::from_bytes_until_nul(&buffer)
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
            })
            .is_some_and(|source| is_feeder(&source))
    }

    fn parameters(&self) -> Result<Parameters, Status> {
        let mut params = Parameters::default();
        // SAFETY: an out pointer to a Parameters.
        let status = unsafe { (self.sane.api.get_parameters)(self.handle, &mut params) };
        if status == GOOD {
            Ok(params)
        } else {
            Err(status)
        }
    }

    /// Reads one frame until EOF.
    fn read_frame(&self, cancel: &AtomicBool) -> Result<Vec<u8>, Status> {
        let mut data = Vec::new();
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(CANCELLED);
            }
            let mut length: c_int = 0;
            // SAFETY: a buffer and its length.
            let status = unsafe {
                (self.sane.api.read)(
                    self.handle,
                    buffer.as_mut_ptr(),
                    buffer.len() as c_int,
                    &mut length,
                )
            };
            match status {
                GOOD => data.extend_from_slice(&buffer[..length.max(0) as usize]),
                EOF => return Ok(data),
                other => return Err(other),
            }
        }
    }

    /// One page: one frame, or three for a three-pass scanner.
    fn read_page(&self, cancel: &AtomicBool, resolution: u32) -> Result<Scanned, Status> {
        let mut planes: [Option<(Vec<u8>, Parameters)>; 3] = [None, None, None];
        loop {
            let params = self.parameters()?;
            let data = self.read_frame(cancel)?;
            let last = params.last_frame != 0;
            match params.format {
                FRAME_GRAY | FRAME_RGB => {
                    return to_image(&data, &params)
                        .map(|image| Scanned { image, resolution })
                        .ok_or(EOF);
                }
                FRAME_RED | FRAME_GREEN | FRAME_BLUE => {
                    planes[(params.format - FRAME_RED) as usize] = Some((data, params));
                }
                _ => return Err(EOF),
            }
            if last {
                break;
            }
            // SAFETY: the next frame of the same page.
            let status = unsafe { (self.sane.api.start)(self.handle) };
            if status != GOOD {
                return Err(status);
            }
        }
        let [Some(red), Some(green), Some(blue)] = planes else {
            return Err(EOF);
        };
        three_pass(&red, &green, &blue)
            .map(|image| Scanned { image, resolution })
            .ok_or(EOF)
    }
}

/// SAFETY: `desc` comes from sane_get_option_descriptor.
unsafe fn constraint(desc: &OptionDescriptor) -> Constraint {
    if desc.constraint.is_null() {
        return Constraint::None;
    }
    unsafe {
        match desc.constraint_type {
            CONSTRAINT_RANGE => {
                let range = &*(desc.constraint as *const Range);
                Constraint::Range {
                    min: range.min,
                    max: range.max,
                    quant: range.quant,
                }
            }
            CONSTRAINT_WORD_LIST => {
                // The first word is how many follow.
                let words = desc.constraint as *const Word;
                let n = (*words).clamp(0, 1024) as usize;
                Constraint::Words((1..=n).map(|i| *words.add(i)).collect())
            }
            CONSTRAINT_STRING_LIST => {
                let mut list = Vec::new();
                let mut item = desc.constraint as *const *const c_char;
                while !(*item).is_null() && list.len() < 256 {
                    list.push(CStr::from_ptr(*item).to_string_lossy().into_owned());
                    item = item.add(1);
                }
                Constraint::Strings(list)
            }
            _ => Constraint::None,
        }
    }
}

fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

fn is_color_mode(mode: &str) -> bool {
    let m = lower(mode);
    m.contains("color") || m.contains("colour")
}

fn is_gray_mode(mode: &str) -> bool {
    let m = lower(mode);
    m.contains("gray") || m.contains("grey")
}

fn is_feeder(source: &str) -> bool {
    let s = lower(source);
    s.contains("adf") || s.contains("feeder") || s.contains("document") || s.contains("duplex")
}

fn is_duplex(source: &str) -> bool {
    lower(source).contains("duplex")
}

/// The source for a request: duplex where asked and offered, else the
/// feeder's front, else whatever comes first (a flatbed).
pub(crate) fn pick_source(sources: &[String], duplex: bool) -> Option<String> {
    let feeder_front = || {
        sources
            .iter()
            .find(|s| is_feeder(s) && !is_duplex(s) && !lower(s).contains("back"))
    };
    let chosen = if duplex {
        sources.iter().find(|s| is_duplex(s)).or_else(feeder_front)
    } else {
        feeder_front()
    };
    chosen.or_else(|| sources.first()).cloned()
}

/// A frame as an 8-bit image: grey or RGB, from 1, 8 or 16 bits per sample.
fn to_image(data: &[u8], params: &Parameters) -> Option<DynamicImage> {
    let width = u32::try_from(params.pixels_per_line)
        .ok()
        .filter(|w| *w > 0)?;
    let bpl = usize::try_from(params.bytes_per_line)
        .ok()
        .filter(|b| *b > 0)?;
    // A hand scanner, or a feeder that doesn't know the length: as many
    // lines as arrived.
    let lines = data.len() / bpl;
    let height = u32::try_from(lines).ok().filter(|h| *h > 0)?;
    let channels = if params.format == FRAME_RGB { 3 } else { 1 };
    let mut out = Vec::with_capacity(width as usize * lines * channels);
    for line in data.chunks_exact(bpl).take(lines) {
        for x in 0..(width as usize * channels) {
            let sample = match params.depth {
                // Lineart: 1 is black.
                1 => {
                    if line.get(x / 8)? & (0x80 >> (x % 8)) != 0 {
                        0
                    } else {
                        255
                    }
                }
                8 => *line.get(x)?,
                // SANE sends 16-bit samples in the machine's byte order.
                16 => {
                    let bytes = [*line.get(x * 2)?, *line.get(x * 2 + 1)?];
                    (u16::from_ne_bytes(bytes) >> 8) as u8
                }
                _ => return None,
            };
            out.push(sample);
        }
    }
    if channels == 3 {
        RgbImage::from_raw(width, height, out).map(DynamicImage::ImageRgb8)
    } else {
        GrayImage::from_raw(width, height, out).map(DynamicImage::ImageLuma8)
    }
}

fn three_pass(
    red: &(Vec<u8>, Parameters),
    green: &(Vec<u8>, Parameters),
    blue: &(Vec<u8>, Parameters),
) -> Option<DynamicImage> {
    let as_gray = |(data, params): &(Vec<u8>, Parameters)| {
        let gray = Parameters {
            format: FRAME_GRAY,
            ..*params
        };
        to_image(data, &gray).map(|i| i.into_luma8())
    };
    let (r, g, b) = (as_gray(red)?, as_gray(green)?, as_gray(blue)?);
    let (w, h) = (
        r.width().min(g.width()).min(b.width()),
        r.height().min(g.height()).min(b.height()),
    );
    Some(DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([
            r.get_pixel(x, y)[0],
            g.get_pixel(x, y)[0],
            b.get_pixel(x, y)[0],
        ])
    })))
}

impl Clone for Parameters {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for Parameters {}

/// Whether a SANE device is a real scanner, not a camera or a test device
/// someone left enabled. Test devices stay listed only when asked for
/// (`CWW_SCAN_TEST_DEVICES=1`), for development and tests.
fn listed(name: &str, kind: &str) -> bool {
    let test = name.starts_with("test:");
    if test {
        return std::env::var_os("CWW_SCAN_TEST_DEVICES").is_some_and(|v| v == "1");
    }
    let kind = lower(kind);
    !(name.starts_with("v4l:") || kind.contains("camera") || kind.contains("video"))
}

fn text(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        // SAFETY: SANE's device strings are nul-terminated.
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .trim()
            .to_string()
    }
}

impl Backend for Sane {
    fn devices(&mut self) -> Result<Vec<Device>, ScanError> {
        let mut list: *const *const RawDevice = std::ptr::null();
        // SAFETY: an out pointer; the list stays valid until the next call.
        // Local devices only: SANE's network backends (net, escl) look around
        // the network for seconds; network scanners get a backend of their own.
        let status = unsafe { (self.api.get_devices)(&mut list, 1) };
        if status != GOOD {
            return Err(self.failure(status, "looking for scanners"));
        }
        let mut devices = Vec::new();
        let mut item = list;
        // SAFETY: a null-terminated array of device pointers.
        while !item.is_null() && unsafe { !(*item).is_null() } && devices.len() < 64 {
            let raw = unsafe { &**item };
            let id = text(raw.name);
            let (vendor, model, kind) = (text(raw.vendor), text(raw.model), text(raw.kind));
            if !id.is_empty() && listed(&id, &kind) {
                let name = if model
                    .to_ascii_lowercase()
                    .starts_with(&vendor.to_ascii_lowercase())
                    || vendor.is_empty()
                {
                    model.clone()
                } else {
                    format!("{vendor} {model}")
                };
                devices.push(Device {
                    id,
                    name: if name.is_empty() {
                        "Scanner".into()
                    } else {
                        name
                    },
                    kind,
                });
            }
            item = unsafe { item.add(1) };
        }
        Ok(devices)
    }

    fn capabilities(&mut self, device: &str) -> Result<Capabilities, ScanError> {
        Ok(self.open(device)?.capabilities())
    }

    fn scan(
        &mut self,
        request: &Request,
        cancel: &Arc<AtomicBool>,
        page: &mut dyn FnMut(Scanned) -> Result<(), ScanError>,
    ) -> Result<End, ScanError> {
        let open = self.open(&request.device)?;
        let feeder = open.configure(request);
        let resolution = open
            .option("resolution")
            .and_then(|opt| {
                open.get_word(&opt)
                    .map(|raw| opt.number(raw).round() as u32)
            })
            .filter(|r| *r > 0)
            .unwrap_or(request.resolution);
        // A watcher calls sane_cancel when the person stops the scan, so a
        // read waiting on the scanner returns at once.
        let handle = open.handle as usize;
        let cancel_fn = self.api.cancel;
        let done = Arc::new(AtomicBool::new(false));
        let watcher = {
            let (cancel, done) = (cancel.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    if cancel.load(Ordering::SeqCst) {
                        // SAFETY: SANE lets sane_cancel be called at any time,
                        // from any context, while the handle is open; `done`
                        // is set before it closes.
                        unsafe { cancel_fn(handle as Handle) };
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            })
        };
        let result = (|| {
            let mut pages = 0usize;
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return Ok(End::Cancelled);
                }
                // SAFETY: an open handle.
                let status = unsafe { (self.api.start)(open.handle) };
                match status {
                    GOOD => {}
                    NO_DOCS if pages == 0 => return Ok(End::NoPaper),
                    NO_DOCS => return Ok(End::Done),
                    CANCELLED => return Ok(End::Cancelled),
                    other => return Err(self.failure(other, "starting")),
                }
                match open.read_page(cancel, resolution) {
                    Ok(scanned) => {
                        pages += 1;
                        page(scanned)?;
                    }
                    Err(CANCELLED) => return Ok(End::Cancelled),
                    Err(NO_DOCS) => return Ok(if pages == 0 { End::NoPaper } else { End::Done }),
                    Err(status) => return Err(self.failure(status, "reading a page")),
                }
                if !feeder {
                    return Ok(End::Done);
                }
            }
        })();
        done.store(true, Ordering::SeqCst);
        let _ = watcher.join();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn picks_duplex_then_the_front_of_the_feeder() {
        let canon = list(&["ADF Front", "ADF Back", "ADF Duplex"]);
        assert_eq!(pick_source(&canon, true).as_deref(), Some("ADF Duplex"));
        assert_eq!(pick_source(&canon, false).as_deref(), Some("ADF Front"));
        let flatbed_and_feeder = list(&["Flatbed", "Automatic Document Feeder"]);
        assert_eq!(
            pick_source(&flatbed_and_feeder, true).as_deref(),
            Some("Automatic Document Feeder")
        );
        assert_eq!(
            pick_source(&list(&["Flatbed"]), true).as_deref(),
            Some("Flatbed")
        );
        assert_eq!(pick_source(&[], true), None);
    }

    #[test]
    fn reads_lineart_grey_and_colour_frames() {
        let params = |format, depth, ppl, bpl| Parameters {
            format,
            last_frame: 1,
            bytes_per_line: bpl,
            pixels_per_line: ppl,
            lines: -1,
            depth,
        };
        let lineart = to_image(&[0b1010_0000, 0], &params(FRAME_GRAY, 1, 4, 1)).unwrap();
        assert_eq!(
            lineart.to_luma8().into_raw(),
            vec![0, 255, 0, 255, 255, 255, 255, 255]
        );
        let gray = to_image(&[1, 2, 3, 4], &params(FRAME_GRAY, 8, 2, 2)).unwrap();
        assert_eq!((gray.width(), gray.height()), (2, 2));
        let rgb = to_image(&[9, 8, 7], &params(FRAME_RGB, 8, 1, 3)).unwrap();
        assert_eq!(rgb.to_rgb8().into_raw(), vec![9, 8, 7]);
        // A half line at the end is dropped, not read past.
        assert_eq!(
            to_image(&[1, 2, 3], &params(FRAME_GRAY, 8, 2, 2))
                .unwrap()
                .height(),
            1
        );
    }

    #[test]
    fn test_devices_stay_hidden_unless_asked_for() {
        assert!(listed("canon_dr:libusb:001:004", "sheetfed scanner"));
        assert!(!listed("v4l:/dev/video0", "virtual device"));
    }
}
