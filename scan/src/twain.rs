//! TWAIN, on Windows: the scanner maker's own driver, for scanners that
//! offer no WIA driver (Canon's P-208II among them).
//!
//! Most TWAIN data sources are 32-bit (Canon's runs under WOW on 64-bit
//! Windows), and a 32-bit driver loads only into a 32-bit process, so this
//! runs in `cww-scan-helper.exe`, built for i686, which the 64-bit app
//! starts and talks to over a pipe (helper.rs), as NAPS2 does. A driver
//! that crashes takes only that process. A 64-bit build uses the 64-bit
//! data source manager (TWAINDSM.dll) and 64-bit sources instead.
//!
//! TWAIN needs a window and a message loop on the thread that talks to it:
//! the source posts its events (a page ready, close me) to the thread's
//! queue, and every message goes through the source first. So a hidden
//! window lives as long as the backend, on the helper's main thread, which
//! is where every Backend call is made. The source's own window is never
//! shown (ShowUI false); settings go through capabilities.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use image::{DynamicImage, GrayImage, RgbImage};
use libloading::Library;
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
    TranslateMessage, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use crate::{Backend, Capabilities, Device, End, Request, ScanError, Scanned};

// Groups, data argument types and messages (twain.h).
const DG_CONTROL: u32 = 1;
const DG_IMAGE: u32 = 2;
const DAT_CAPABILITY: u16 = 0x0001;
const DAT_EVENT: u16 = 0x0002;
const DAT_IDENTITY: u16 = 0x0003;
const DAT_PARENT: u16 = 0x0004;
const DAT_PENDINGXFERS: u16 = 0x0005;
const DAT_STATUS: u16 = 0x0008;
const DAT_USERINTERFACE: u16 = 0x0009;
const DAT_IMAGEINFO: u16 = 0x0101;
const DAT_IMAGENATIVEXFER: u16 = 0x0104;
const MSG_GET: u16 = 0x0001;
const MSG_GETFIRST: u16 = 0x0004;
const MSG_GETNEXT: u16 = 0x0005;
const MSG_SET: u16 = 0x0006;
const MSG_RESET: u16 = 0x0007;
const MSG_XFERREADY: u16 = 0x0101;
const MSG_CLOSEDSREQ: u16 = 0x0102;
const MSG_CLOSEDSOK: u16 = 0x0103;
const MSG_OPENDSM: u16 = 0x0301;
const MSG_CLOSEDSM: u16 = 0x0302;
const MSG_OPENDS: u16 = 0x0401;
const MSG_CLOSEDS: u16 = 0x0402;
const MSG_DISABLEDS: u16 = 0x0501;
const MSG_ENABLEDS: u16 = 0x0502;
const MSG_PROCESSEVENT: u16 = 0x0601;
const MSG_ENDXFER: u16 = 0x0701;

const TWRC_SUCCESS: u16 = 0;
const TWRC_CANCEL: u16 = 3;
const TWRC_DSEVENT: u16 = 4;
const TWRC_XFERDONE: u16 = 6;

const TWCC_NOMEDIA: u16 = 31;
const TWCC_PAPERJAM: u16 = 20;
const TWCC_PAPERDOUBLEFEED: u16 = 21;

const TWON_ARRAY: u16 = 3;
const TWON_ENUMERATION: u16 = 4;
const TWON_ONEVALUE: u16 = 5;
const TWON_RANGE: u16 = 6;
const TWTY_INT16: u16 = 1;
const TWTY_UINT16: u16 = 4;
const TWTY_BOOL: u16 = 6;
const TWTY_FIX32: u16 = 7;

const CAP_XFERCOUNT: u16 = 0x0001;
const ICAP_PIXELTYPE: u16 = 0x0101;
const ICAP_UNITS: u16 = 0x0102;
const ICAP_XFERMECH: u16 = 0x0103;
const CAP_FEEDERENABLED: u16 = 0x1002;
const CAP_AUTOFEED: u16 = 0x1007;
const CAP_INDICATORS: u16 = 0x100b;
const CAP_DUPLEX: u16 = 0x1012;
const CAP_DUPLEXENABLED: u16 = 0x1013;
const ICAP_XRESOLUTION: u16 = 0x1118;
const ICAP_YRESOLUTION: u16 = 0x1119;
const ICAP_BITDEPTH: u16 = 0x112b;
const ICAP_AUTOMATICBORDERDETECTION: u16 = 0x1150;
const ICAP_AUTOMATICDESKEW: u16 = 0x1151;

const TWPT_GRAY: u16 = 1;
const TWPT_RGB: u16 = 2;

// TWAIN's structures are packed to two bytes on Windows.
#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct Version {
    major: u16,
    minor: u16,
    language: u16,
    country: u16,
    info: [u8; 34],
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct Identity {
    /// TW_MEMREF on 64-bit, TW_UINT32 on 32-bit.
    id: usize,
    version: Version,
    protocol_major: u16,
    protocol_minor: u16,
    supported_groups: u32,
    manufacturer: [u8; 34],
    product_family: [u8; 34],
    product_name: [u8; 34],
}

impl Identity {
    fn empty() -> Self {
        // SAFETY: all-zero is a valid TW_IDENTITY (plain data).
        unsafe { std::mem::zeroed() }
    }

    fn app() -> Self {
        let mut id = Self::empty();
        id.version.major = 1;
        // TWLG_ENGLISH, as NAPS2 sends: Canon's driver makes a C++ locale
        // from it and throws "bad locale name" for TWLG_USA (13).
        id.version.language = 2;
        id.version.country = 1; // TWCY_USA
        copy_str(&mut id.version.info, env!("CARGO_PKG_VERSION"));
        id.protocol_major = 1;
        id.protocol_minor = 9;
        id.supported_groups = DG_CONTROL | DG_IMAGE;
        copy_str(&mut id.manufacturer, "Chat with Work");
        copy_str(&mut id.product_family, "Chat with Work Desktop");
        copy_str(&mut id.product_name, "Chat with Work");
        id
    }

    fn name(&self) -> String {
        let name = self.product_name;
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        String::from_utf8_lossy(&name[..end]).trim().to_string()
    }
}

fn copy_str(field: &mut [u8; 34], value: &str) {
    for (slot, byte) in field.iter_mut().zip(value.bytes().take(33)) {
        *slot = byte;
    }
}

#[repr(C, packed(2))]
struct UserInterface {
    show_ui: u16,
    modal_ui: u16,
    parent: HANDLE,
}

#[repr(C, packed(2))]
struct Event {
    event: *mut c_void,
    message: u16,
}

#[repr(C, packed(2))]
struct PendingXfers {
    count: u16,
    reserved: u32,
}

#[repr(C, packed(2))]
struct Capability {
    cap: u16,
    con_type: u16,
    container: HANDLE,
}

#[repr(C, packed(2))]
struct OneValue {
    item_type: u16,
    item: u32,
}

#[repr(C, packed(2))]
struct Range {
    item_type: u16,
    min: u32,
    max: u32,
    step: u32,
    default: u32,
    current: u32,
}

#[repr(C, packed(2))]
struct EnumerationHeader {
    item_type: u16,
    count: u32,
    current: u32,
    default: u32,
}

#[repr(C, packed(2))]
#[derive(Default)]
struct ImageInfo {
    x_resolution: u32,
    y_resolution: u32,
    width: i32,
    length: i32,
    samples_per_pixel: i16,
    bits_per_sample: [i16; 8],
    bits_per_pixel: i16,
    planar: u16,
    pixel_type: i16,
    compression: u16,
}

#[repr(C, packed(2))]
#[derive(Default)]
struct Status {
    condition: u16,
    reserved: u16,
}

/// TW_FIX32, as it sits in a container's u32: the whole part in the low
/// word, the fraction in the high one.
fn fix32(value: f64) -> u32 {
    let whole = value.floor() as i16;
    let frac = ((value - f64::from(whole)) * 65536.0).round() as u16;
    u32::from(whole as u16) | (u32::from(frac) << 16)
}

fn unfix32(raw: u32) -> f64 {
    f64::from((raw & 0xffff) as u16 as i16) + f64::from((raw >> 16) as u16) / 65536.0
}

/// Each TWAIN step on stderr, with CWW_SCAN_TRACE=1: drivers differ, and
/// a step that hangs or fails shows where.
macro_rules! trace {
    ($($arg:tt)*) => {
        if std::env::var_os("CWW_SCAN_TRACE").is_some() {
            eprintln!("twain: {}", format!($($arg)*));
        }
    };
}

type Entry =
    unsafe extern "system" fn(*mut Identity, *mut Identity, u32, u16, u16, *mut c_void) -> u16;

/// The data source manager, open, with a hidden window for its events.
pub struct Twain {
    entry: Entry,
    app: Box<Identity>,
    window: HWND,
    sources: Vec<Identity>,
    _library: Library,
}

fn no_twain() -> ScanError {
    ScanError::new(
        "no_twain",
        "Scanning on Windows needs the scanner maker's TWAIN driver. Install it from their support site, plug the scanner in again, and look for scanners again.",
    )
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: the default procedure for a window that does nothing itself.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// With CWW_SCAN_TRACE=1, the windows this process has, every few seconds:
/// a driver that stops answering is usually showing a window of its own.
fn trace_windows() {
    use windows_sys::Win32::Foundation::{LPARAM, TRUE};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };
    unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> i32 {
        // SAFETY: reading a window's owner and title.
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == GetCurrentProcessId() {
                let mut title = [0u16; 256];
                let n = GetWindowTextW(hwnd, title.as_mut_ptr(), 256).max(0) as usize;
                trace!(
                    "window {:?} visible={} title={:?}",
                    hwnd,
                    IsWindowVisible(hwnd) != 0,
                    String::from_utf16_lossy(&title[..n])
                );
                // A visible window's text: a driver's message.
                if IsWindowVisible(hwnd) != 0 {
                    unsafe extern "system" fn child(hwnd: HWND, _: LPARAM) -> i32 {
                        // SAFETY: reading a child window's text.
                        unsafe {
                            let mut text = [0u16; 512];
                            let n = GetWindowTextW(hwnd, text.as_mut_ptr(), 512).max(0) as usize;
                            if n > 0 {
                                trace!("  text {:?}", String::from_utf16_lossy(&text[..n]));
                            }
                        }
                        TRUE
                    }
                    windows_sys::Win32::UI::WindowsAndMessaging::EnumChildWindows(
                        hwnd,
                        Some(child),
                        0,
                    );
                }
            }
        }
        TRUE
    }
    if std::env::var_os("CWW_SCAN_TRACE").is_none() {
        return;
    }
    // Exceptions a driver raises, with the module they come from: one that
    // escapes the driver unwinds through us.
    unsafe extern "system" fn exception(
        info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
    ) -> i32 {
        // SAFETY: the system hands over valid exception pointers.
        unsafe {
            let record = &*(*info).ExceptionRecord;
            let mut module = [0u16; 260];
            let mut handle = std::ptr::null_mut();
            let found = windows_sys::Win32::System::LibraryLoader::GetModuleHandleExW(
                0x4 | 0x2, // FROM_ADDRESS | UNCHANGED_REFCOUNT
                record.ExceptionAddress as *const u16,
                &mut handle,
            );
            let n = if found != 0 {
                windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(
                    handle,
                    module.as_mut_ptr(),
                    260,
                ) as usize
            } else {
                0
            };
            trace!(
                "exception {:#010x} at {:?} in {} (params {:?})",
                record.ExceptionCode as u32,
                record.ExceptionAddress,
                String::from_utf16_lossy(&module[..n]),
                &record.ExceptionInformation[..(record.NumberParameters as usize).min(4)]
            );
            // The stack, as module and offset, to see which part of a driver
            // raised it.
            let mut frames = [std::ptr::null_mut::<c_void>(); 40];
            let count = windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace(
                0,
                frames.len() as u32,
                frames.as_mut_ptr(),
                std::ptr::null_mut(),
            );
            for frame in &frames[..count as usize] {
                let mut handle = std::ptr::null_mut();
                let mut name = [0u16; 260];
                if windows_sys::Win32::System::LibraryLoader::GetModuleHandleExW(
                    0x4 | 0x2,
                    *frame as *const u16,
                    &mut handle,
                ) != 0
                {
                    let n = windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(
                        handle,
                        name.as_mut_ptr(),
                        260,
                    ) as usize;
                    let module = String::from_utf16_lossy(&name[..n]);
                    let short = module.rsplit('\\').next().unwrap_or_default().to_string();
                    trace!("  at {short}+{:#x}", *frame as usize - handle as usize);
                }
            }
            // A C++ exception on 32-bit MSVC: its type names, from the throw
            // information, and what() where it's a std::exception.
            #[cfg(target_arch = "x86")]
            if record.ExceptionCode as u32 == 0xe06d_7363 && record.NumberParameters >= 3 {
                let object = record.ExceptionInformation[1] as *const *const usize;
                let throw_info = record.ExceptionInformation[2] as *const u32;
                if !throw_info.is_null() {
                    let catchable = *throw_info.add(3) as *const u32;
                    let count = (*catchable).min(8) as usize;
                    let mut is_std_exception = false;
                    for i in 0..count {
                        let ty = *catchable.add(1 + i) as *const u32;
                        let descriptor = *ty.add(1) as *const u8;
                        let name =
                            std::ffi::CStr::from_ptr(descriptor.add(8).cast()).to_string_lossy();
                        trace!("  c++ type {name}");
                        // An object deriving from std::exception, never a
                        // pointer to one (".PAV..."), has what() in its vtable.
                        is_std_exception |= name == ".?AVexception@std@@";
                    }
                    if !object.is_null() && is_std_exception {
                        let vtable = *object;
                        let what: unsafe extern "thiscall" fn(*const *const usize) -> *const i8 =
                            std::mem::transmute(*vtable.add(1));
                        let message = what(object);
                        if !message.is_null() {
                            trace!(
                                "  what(): {}",
                                std::ffi::CStr::from_ptr(message).to_string_lossy()
                            );
                        }
                    }
                }
            }
        }
        0 // EXCEPTION_CONTINUE_SEARCH
    }
    // SAFETY: a first-chance handler that only logs.
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(
            1,
            Some(exception),
        )
    };
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            // SAFETY: enumerating top-level windows with a plain callback.
            unsafe { EnumWindows(Some(each), 0) };
        }
    });
}

impl Twain {
    pub fn load() -> Result<Self, ScanError> {
        trace_windows();
        // Drivers expect a single-threaded COM apartment with OLE on the
        // thread that talks to them, as .NET apps and most scanning programs
        // have; Canon's throws while opening without it.
        // SAFETY: once, on the thread that owns the TWAIN session.
        unsafe { windows_sys::Win32::System::Ole::OleInitialize(std::ptr::null_mut()) };
        // The open TWAIN DSM 2.x (TWAINDSM.dll) beside this program, then
        // one installed with Windows or a driver, then, for 32-bit sources,
        // Microsoft's twain_32.dll. Modern drivers are TWAIN 2 sources
        // and can fail with Microsoft's 1.x manager (Canon's P-208II
        // throws while opening), so the open DSM comes first.
        let beside = std::env::current_exe()
            .ok()
            .map(|exe| exe.with_file_name("TWAINDSM.dll"))
            .filter(|path| path.is_file());
        let mut candidates: Vec<std::ffi::OsString> =
            beside.into_iter().map(|p| p.into_os_string()).collect();
        candidates.push("TWAINDSM.dll".into());
        if cfg!(target_pointer_width = "32") {
            candidates.push("twain_32.dll".into());
        }
        // SAFETY: the DSM runs no code on load that needs anything from us.
        let library = candidates
            .iter()
            .find_map(|n| unsafe { Library::new(n) }.ok())
            .ok_or_else(no_twain)?;
        // Drivers built with old C++ runtimes need a classic locale; give
        // them one where the person's has none (twain_locale.rs), before
        // any driver loads.
        if let Some(shim) = crate::twain_locale::install() {
            trace!("locale for drivers: {shim}");
        }
        // SAFETY: DSM_Entry has this signature in every DSM.
        let entry: Entry = unsafe { *library.get(b"DSM_Entry\0").map_err(|_| no_twain())? };
        let window = hidden_window()?;
        let mut twain = Self {
            entry,
            app: Box::new(Identity::app()),
            window,
            sources: Vec::new(),
            _library: library,
        };
        let mut parent = window;
        let rc = twain.call(
            None,
            DG_CONTROL,
            DAT_PARENT,
            MSG_OPENDSM,
            (&mut parent as *mut HWND).cast(),
        );
        if rc != TWRC_SUCCESS {
            return Err(ScanError::new(
                "no_twain",
                "Windows' TWAIN manager couldn't start.",
            ));
        }
        Ok(twain)
    }

    fn call(
        &mut self,
        source: Option<&mut Identity>,
        group: u32,
        dat: u16,
        msg: u16,
        data: *mut c_void,
    ) -> u16 {
        let dest = source.map_or(std::ptr::null_mut(), |s| s as *mut Identity);
        if dat != DAT_EVENT {
            trace!("call dg={group} dat={dat:#06x} msg={msg:#06x}");
        }
        // SAFETY: the app identity lives in a Box for the DSM's lifetime, and
        // `data` points at the structure `dat` names.
        let rc = unsafe { (self.entry)(&mut *self.app, dest, group, dat, msg, data) };
        if dat != DAT_EVENT {
            trace!("  -> rc={rc}");
        }
        rc
    }

    fn condition(&mut self, source: Option<&mut Identity>) -> u16 {
        let mut status = Status::default();
        self.call(
            source,
            DG_CONTROL,
            DAT_STATUS,
            MSG_GET,
            (&mut status as *mut Status).cast(),
        );
        status.condition
    }

    fn list(&mut self) -> Vec<Identity> {
        let mut sources = Vec::new();
        let mut id = Identity::empty();
        let mut msg = MSG_GETFIRST;
        while self.call(
            None,
            DG_CONTROL,
            DAT_IDENTITY,
            msg,
            (&mut id as *mut Identity).cast(),
        ) == TWRC_SUCCESS
        {
            // Microsoft's WIA bridge isn't a scanner of its own.
            if !id.name().to_ascii_lowercase().contains("wia-") {
                sources.push(id);
            }
            msg = MSG_GETNEXT;
            if sources.len() > 64 {
                break;
            }
        }
        sources
    }

    fn open(&mut self, device: &str) -> Result<Identity, ScanError> {
        if self.sources.is_empty() {
            self.sources = self.list();
        }
        let mut source = self
            .sources
            .iter()
            .find(|s| s.name() == device)
            .copied()
            .ok_or_else(|| ScanError::new("not_found", "That scanner isn't connected any more. Plug it in and look for scanners again."))?;
        source.id = 0;
        let rc = self.call(
            None,
            DG_CONTROL,
            DAT_IDENTITY,
            MSG_OPENDS,
            (&mut source as *mut Identity).cast(),
        );
        if rc != TWRC_SUCCESS {
            return Err(ScanError::new(
                "busy",
                "The scanner's driver wouldn't start. Check it's plugged in and turned on, and that no other program is using it.",
            ));
        }
        Ok(source)
    }

    fn close(&mut self, source: &mut Identity) {
        self.call(
            None,
            DG_CONTROL,
            DAT_IDENTITY,
            MSG_CLOSEDS,
            (source as *mut Identity).cast(),
        );
    }

    fn set_one(&mut self, source: &mut Identity, cap: u16, item_type: u16, value: u32) -> bool {
        // SAFETY: a movable global block for a TW_ONEVALUE, freed after.
        unsafe {
            let handle = GlobalAlloc(GHND, std::mem::size_of::<OneValue>());
            if handle.is_null() {
                return false;
            }
            let one = GlobalLock(handle) as *mut OneValue;
            std::ptr::write_unaligned(
                one,
                OneValue {
                    item_type,
                    item: value,
                },
            );
            GlobalUnlock(handle);
            let mut capability = Capability {
                cap,
                con_type: TWON_ONEVALUE,
                container: handle,
            };
            let rc = self.call(
                Some(source),
                DG_CONTROL,
                DAT_CAPABILITY,
                MSG_SET,
                (&mut capability as *mut Capability).cast(),
            );
            GlobalFree(handle);
            rc == TWRC_SUCCESS
        }
    }

    /// A capability's values: a list for an enumeration or array, every
    /// common one in a range, or its one value.
    fn get_values(&mut self, source: &mut Identity, cap: u16) -> Option<(u16, Vec<u32>)> {
        let mut capability = Capability {
            cap,
            con_type: 0,
            container: std::ptr::null_mut(),
        };
        if self.call(
            Some(source),
            DG_CONTROL,
            DAT_CAPABILITY,
            MSG_GET,
            (&mut capability as *mut Capability).cast(),
        ) != TWRC_SUCCESS
        {
            return None;
        }
        let handle = capability.container;
        if handle.is_null() {
            return None;
        }
        // SAFETY: the source filled a container of the type it names.
        let values = unsafe {
            let size = GlobalSize(handle);
            let base = GlobalLock(handle) as *const u8;
            let read_items = |item_type: u16, first: *const u8, count: usize| -> Vec<u32> {
                let width = item_size(item_type);
                (0..count.min(256))
                    .filter(|i| (first as usize - base as usize) + (i + 1) * width <= size)
                    .map(|i| read_item(first.add(i * width), item_type))
                    .collect()
            };
            let result = match capability.con_type {
                TWON_ONEVALUE => {
                    let one = std::ptr::read_unaligned(base as *const OneValue);
                    Some((one.item_type, vec![read_item(base.add(2), one.item_type)]))
                }
                TWON_ENUMERATION => {
                    let header = std::ptr::read_unaligned(base as *const EnumerationHeader);
                    let first = base.add(std::mem::size_of::<EnumerationHeader>());
                    Some((
                        header.item_type,
                        read_items(header.item_type, first, header.count as usize),
                    ))
                }
                TWON_ARRAY => {
                    let item_type = std::ptr::read_unaligned(base as *const u16);
                    let count = std::ptr::read_unaligned(base.add(2) as *const u32);
                    Some((
                        item_type,
                        read_items(item_type, base.add(6), count as usize),
                    ))
                }
                TWON_RANGE => {
                    let range = std::ptr::read_unaligned(base as *const Range);
                    Some((range.item_type, vec![range.min, range.max, range.step]))
                }
                _ => None,
            };
            GlobalUnlock(handle);
            GlobalFree(handle);
            result
        };
        values.map(|(t, v)| {
            if capability.con_type == TWON_RANGE {
                (t | 0x8000, v)
            } else {
                (t, v)
            }
        })
    }

    /// Runs the message loop until the source has a page, asks to close,
    /// or `cancel` is set.
    fn wait(&mut self, source: &mut Identity, cancel: &AtomicBool) -> u16 {
        loop {
            // SAFETY: a plain thread message loop on the window's thread.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    let mut event = Event {
                        event: (&mut msg as *mut MSG).cast(),
                        message: 0,
                    };
                    let rc = self.call(
                        Some(source),
                        DG_CONTROL,
                        DAT_EVENT,
                        MSG_PROCESSEVENT,
                        (&mut event as *mut Event).cast(),
                    );
                    if rc != TWRC_DSEVENT {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if event.message != 0 {
                        trace!("event message={:#06x}", event.message);
                    }
                    if matches!(
                        event.message,
                        MSG_XFERREADY | MSG_CLOSEDSREQ | MSG_CLOSEDSOK
                    ) {
                        return event.message;
                    }
                }
                if cancel.load(Ordering::SeqCst) {
                    return MSG_CLOSEDSREQ;
                }
                MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 100, QS_ALLINPUT);
            }
        }
    }

    /// The page ready now, as a native transfer: a DIB.
    fn transfer(&mut self, source: &mut Identity) -> Result<Option<Scanned>, u16> {
        let mut info = ImageInfo::default();
        self.call(
            Some(source),
            DG_IMAGE,
            DAT_IMAGEINFO,
            MSG_GET,
            (&mut info as *mut ImageInfo).cast(),
        );
        let resolution = unfix32(info.x_resolution).round().max(1.0) as u32;
        let mut handle: HANDLE = std::ptr::null_mut();
        let rc = self.call(
            Some(source),
            DG_IMAGE,
            DAT_IMAGENATIVEXFER,
            MSG_GET,
            (&mut handle as *mut HANDLE).cast(),
        );
        match rc {
            TWRC_XFERDONE if !handle.is_null() => {
                // SAFETY: a DIB in a global block the source handed over.
                let image = unsafe {
                    let size = GlobalSize(handle);
                    let bits = GlobalLock(handle) as *const u8;
                    let image = dib_to_image(std::slice::from_raw_parts(bits, size));
                    GlobalUnlock(handle);
                    GlobalFree(handle);
                    image
                };
                Ok(image.map(|image| Scanned { image, resolution }))
            }
            TWRC_CANCEL => Ok(None),
            _ => Err(rc),
        }
    }

    fn end_transfer(&mut self, source: &mut Identity, reset: bool) -> u16 {
        let mut pending = PendingXfers {
            count: 0,
            reserved: 0,
        };
        let msg = if reset { MSG_RESET } else { MSG_ENDXFER };
        self.call(
            Some(source),
            DG_CONTROL,
            DAT_PENDINGXFERS,
            msg,
            (&mut pending as *mut PendingXfers).cast(),
        );
        pending.count
    }

    fn failure(&mut self, source: &mut Identity) -> ScanError {
        match self.condition(Some(source)) {
            TWCC_PAPERJAM => ScanError::new(
                "jammed",
                "The paper jammed. Clear it from the scanner and scan again.",
            ),
            TWCC_PAPERDOUBLEFEED => ScanError::new(
                "jammed",
                "The scanner took two sheets at once. Load them again and scan again.",
            ),
            code => ScanError::new(
                "scanner_error",
                format!("The scanner's driver failed (TWAIN condition {code})."),
            ),
        }
    }
}

impl Drop for Twain {
    fn drop(&mut self) {
        let mut parent = self.window;
        self.call(
            None,
            DG_CONTROL,
            DAT_PARENT,
            MSG_CLOSEDSM,
            (&mut parent as *mut HWND).cast(),
        );
        // SAFETY: our own window, on its own thread.
        unsafe { DestroyWindow(self.window) };
    }
}

fn hidden_window() -> Result<HWND, ScanError> {
    let class = wide("CwwScanTwain");
    // SAFETY: a plain window class and a window that's never shown.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let title = wide("Chat with Work scanning");
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            Err(ScanError::new(
                "no_twain",
                "Couldn't make the window TWAIN needs.",
            ))
        } else {
            Ok(hwnd)
        }
    }
}

fn item_size(item_type: u16) -> usize {
    match item_type {
        0 | 3 => 1, // INT8, UINT8
        TWTY_INT16 | TWTY_UINT16 | TWTY_BOOL => 2,
        _ => 4, // INT32, UINT32, FIX32
    }
}

/// SAFETY: `at` points at an item of `item_type`.
unsafe fn read_item(at: *const u8, item_type: u16) -> u32 {
    unsafe {
        match item_size(item_type) {
            1 => u32::from(*at),
            2 => u32::from(std::ptr::read_unaligned(at as *const u16)),
            _ => std::ptr::read_unaligned(at as *const u32),
        }
    }
}

/// A packed DIB (BITMAPINFOHEADER, palette, bits) as an 8-bit image: grey for
/// 1 and 8 bits a pixel through the palette, RGB for 24.
pub fn dib_to_image(dib: &[u8]) -> Option<DynamicImage> {
    let u32_at = |o: usize| {
        dib.get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let i32_at = |o: usize| u32_at(o).map(|v| v as i32);
    let u16_at = |o: usize| {
        dib.get(o..o + 2)
            .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
    };
    let header = u32_at(0)? as usize;
    let width = i32_at(4)?;
    let height = i32_at(8)?;
    let bits = u16_at(14)?;
    let compression = u32_at(16)?;
    let used = u32_at(32)? as usize;
    if width <= 0 || height == 0 || compression != 0 || header < 40 {
        return None;
    }
    let (w, h) = (width as u32, height.unsigned_abs());
    let colors = match bits {
        1 | 8 => {
            if used == 0 {
                1usize << bits
            } else {
                used
            }
        }
        _ => 0,
    };
    let palette = &dib.get(header..header + colors * 4)?;
    let data = &dib[header + colors * 4..];
    let stride = (w as usize * bits as usize).div_ceil(32) * 4;
    // Bottom-up unless the height is negative.
    let row = |y: u32| -> Option<&[u8]> {
        let line = if height > 0 { h - 1 - y } else { y } as usize;
        data.get(line * stride..line * stride + stride)
    };
    let gray_of = |index: usize| -> u8 {
        let c = &palette[index * 4..index * 4 + 4];
        ((u16::from(c[0]) + u16::from(c[1]) + u16::from(c[2])) / 3) as u8
    };
    match bits {
        24 => {
            let mut out = Vec::with_capacity((w * h * 3) as usize);
            for y in 0..h {
                let line = row(y)?;
                for x in 0..w as usize {
                    // BGR in a DIB.
                    out.extend_from_slice(&[line[x * 3 + 2], line[x * 3 + 1], line[x * 3]]);
                }
            }
            RgbImage::from_raw(w, h, out).map(DynamicImage::ImageRgb8)
        }
        8 => {
            let mut out = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                let line = row(y)?;
                out.extend(
                    line[..w as usize]
                        .iter()
                        .map(|&i| gray_of(i as usize % colors.max(1))),
                );
            }
            GrayImage::from_raw(w, h, out).map(DynamicImage::ImageLuma8)
        }
        1 => {
            let mut out = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                let line = row(y)?;
                out.extend(
                    (0..w as usize).map(|x| gray_of(((line[x / 8] >> (7 - x % 8)) & 1) as usize)),
                );
            }
            GrayImage::from_raw(w, h, out).map(DynamicImage::ImageLuma8)
        }
        _ => None,
    }
}

fn resolutions(values: Option<(u16, Vec<u32>)>) -> Vec<u32> {
    const COMMON: [u32; 9] = [75, 100, 150, 200, 240, 300, 400, 600, 1200];
    let mut list: Vec<u32> = match values {
        Some((t, v)) if t & 0x8000 != 0 && v.len() == 3 => {
            let (min, max) = (unfix32(v[0]), unfix32(v[1]));
            COMMON
                .iter()
                .copied()
                .filter(|r| f64::from(*r) >= min && f64::from(*r) <= max)
                .collect()
        }
        Some((TWTY_FIX32, v)) => v.iter().map(|r| unfix32(*r).round() as u32).collect(),
        Some((_, v)) => v,
        None => vec![],
    };
    list.retain(|r| *r > 0);
    list.sort_unstable();
    list.dedup();
    list
}

impl Backend for Twain {
    fn devices(&mut self) -> Result<Vec<Device>, ScanError> {
        self.sources = self.list();
        Ok(self
            .sources
            .iter()
            .map(|s| Device {
                id: s.name(),
                name: s.name(),
                kind: "TWAIN".into(),
            })
            .collect())
    }

    fn capabilities(&mut self, device: &str) -> Result<Capabilities, ScanError> {
        let mut source = self.open(device)?;
        let pixel = self
            .get_values(&mut source, ICAP_PIXELTYPE)
            .map(|(_, v)| v)
            .unwrap_or_default();
        let caps = Capabilities {
            resolutions: resolutions(self.get_values(&mut source, ICAP_XRESOLUTION)),
            color: pixel.is_empty() || pixel.contains(&u32::from(TWPT_RGB)),
            feeder: self.get_values(&mut source, CAP_FEEDERENABLED).is_some(),
            duplex: self
                .get_values(&mut source, CAP_DUPLEX)
                .is_some_and(|(_, v)| v.first().is_some_and(|d| *d != 0)),
            flatbed: false,
        };
        self.close(&mut source);
        Ok(caps)
    }

    fn scan(
        &mut self,
        request: &Request,
        cancel: &Arc<AtomicBool>,
        page: &mut dyn FnMut(Scanned) -> Result<(), ScanError>,
    ) -> Result<End, ScanError> {
        let mut source = self.open(&request.device)?;
        let result = self.scan_open(&mut source, request, cancel, page);
        self.close(&mut source);
        result
    }
}

impl Twain {
    fn scan_open(
        &mut self,
        source: &mut Identity,
        request: &Request,
        cancel: &Arc<AtomicBool>,
        page: &mut dyn FnMut(Scanned) -> Result<(), ScanError>,
    ) -> Result<End, ScanError> {
        // What we can set: a source refusing one of these still scans.
        self.set_one(source, ICAP_XFERMECH, TWTY_UINT16, 0); // native
        self.set_one(source, ICAP_UNITS, TWTY_UINT16, 0); // inches
        self.set_one(
            source,
            ICAP_PIXELTYPE,
            TWTY_UINT16,
            u32::from(if request.color { TWPT_RGB } else { TWPT_GRAY }),
        );
        self.set_one(
            source,
            ICAP_BITDEPTH,
            TWTY_UINT16,
            if request.color { 24 } else { 8 },
        );
        let dpi = fix32(f64::from(request.resolution));
        self.set_one(source, ICAP_XRESOLUTION, TWTY_FIX32, dpi);
        self.set_one(source, ICAP_YRESOLUTION, TWTY_FIX32, dpi);
        let feeder = self.set_one(source, CAP_FEEDERENABLED, TWTY_BOOL, 1);
        self.set_one(source, CAP_AUTOFEED, TWTY_BOOL, 1);
        self.set_one(
            source,
            CAP_DUPLEXENABLED,
            TWTY_BOOL,
            u32::from(request.duplex),
        );
        self.set_one(source, CAP_XFERCOUNT, TWTY_INT16, 0xffff); // all
        self.set_one(source, CAP_INDICATORS, TWTY_BOOL, 0);
        self.set_one(source, ICAP_AUTOMATICBORDERDETECTION, TWTY_BOOL, 1);
        self.set_one(source, ICAP_AUTOMATICDESKEW, TWTY_BOOL, 1);

        let mut ui = UserInterface {
            show_ui: 0,
            modal_ui: 0,
            parent: self.window,
        };
        if self.call(
            Some(source),
            DG_CONTROL,
            DAT_USERINTERFACE,
            MSG_ENABLEDS,
            (&mut ui as *mut UserInterface).cast(),
        ) != TWRC_SUCCESS
        {
            return Err(match self.condition(Some(source)) {
                TWCC_NOMEDIA => return Ok(End::NoPaper),
                _ => self.failure(source),
            });
        }
        let mut pages = 0usize;
        let mut end = End::Done;
        let mut failure = None;
        // Once a page is ready, every pending page is handed over in turn;
        // the source says how many are left after each.
        if self.wait(source, cancel) == MSG_XFERREADY {
            loop {
                if cancel.load(Ordering::SeqCst) {
                    self.end_transfer(source, true);
                    end = End::Cancelled;
                    break;
                }
                match self.transfer(source) {
                    Ok(Some(scanned)) => {
                        pages += 1;
                        if let Err(e) = page(scanned) {
                            self.end_transfer(source, true);
                            failure = Some(e);
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        let e = self.failure(source);
                        self.end_transfer(source, true);
                        failure = Some(e);
                        break;
                    }
                }
                if self.end_transfer(source, false) == 0 {
                    break;
                }
            }
        } else if cancel.load(Ordering::SeqCst) {
            end = End::Cancelled;
        }
        let mut ui = UserInterface {
            show_ui: 0,
            modal_ui: 0,
            parent: self.window,
        };
        self.call(
            Some(source),
            DG_CONTROL,
            DAT_USERINTERFACE,
            MSG_DISABLEDS,
            (&mut ui as *mut UserInterface).cast(),
        );
        let _ = feeder;
        match failure {
            Some(e) => Err(e),
            None => Ok(if pages == 0 && end == End::Done {
                End::NoPaper
            } else {
                end
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_numbers_round_trip() {
        assert_eq!(unfix32(fix32(300.0)), 300.0);
        assert_eq!(unfix32(fix32(72.5)), 72.5);
        assert_eq!(fix32(300.0), 300);
    }

    #[test]
    fn structures_have_twains_sizes() {
        // twain.h, packed to two bytes: TW_IDENTITY is 156 bytes with a
        // 32-bit Id, TW_IMAGEINFO 42.
        let id_width = std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<Identity>(), 152 + id_width);
        assert_eq!(std::mem::size_of::<ImageInfo>(), 42);
        assert_eq!(std::mem::size_of::<OneValue>(), 6);
        assert_eq!(std::mem::size_of::<Range>(), 22);
    }

    fn dib(bits: u16, width: i32, height: i32, palette: &[[u8; 4]], rows: &[&[u8]]) -> Vec<u8> {
        let mut out = vec![0u8; 40];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&width.to_le_bytes());
        out[8..12].copy_from_slice(&height.to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&bits.to_le_bytes());
        out[32..36].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for c in palette {
            out.extend_from_slice(c);
        }
        for r in rows {
            out.extend_from_slice(r);
        }
        out
    }

    #[test]
    fn reads_bottom_up_grey_and_bgr_dibs() {
        let grey: Vec<[u8; 4]> = (0..=255u8).map(|v| [v, v, v, 0]).collect();
        // Two rows of three pixels, padded to four bytes, bottom row first.
        let image = dib_to_image(&dib(8, 3, 2, &grey, &[&[1, 2, 3, 0], &[7, 8, 9, 0]])).unwrap();
        assert_eq!(image.to_luma8().into_raw(), vec![7, 8, 9, 1, 2, 3]);
        let image = dib_to_image(&dib(24, 1, 1, &[], &[&[10, 20, 30, 0]])).unwrap();
        assert_eq!(image.to_rgb8().into_raw(), vec![30, 20, 10]);
        let bw = dib_to_image(&dib(
            1,
            2,
            1,
            &[[0, 0, 0, 0], [255, 255, 255, 0]],
            &[&[0b0100_0000, 0, 0, 0]],
        ))
        .unwrap();
        assert_eq!(bw.to_luma8().into_raw(), vec![0, 255]);
    }
}
