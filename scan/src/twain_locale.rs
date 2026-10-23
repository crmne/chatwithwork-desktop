//! A locale for TWAIN drivers built with old C++ runtimes, in the scanner
//! helper process only.
//!
//! Windows' newer locales, such as English with Germany's formats
//! (`en-DE`), have no classic locale ID: `GetUserDefaultLCID` answers
//! `LOCALE_CUSTOM_UNSPECIFIED` (0x1000). The C runtime Microsoft shipped
//! until about 2010 can't match that to a locale, so a driver that builds
//! the user's locale (`std::locale("")`) throws "bad locale name" and takes
//! its process down. Canon's P-208II driver (`CsdCore.dll`) does it in a
//! static initializer, while it loads, so nothing can patch the driver's
//! own imports in time.
//!
//! So when the person's locale has no classic ID, the helper answers the
//! few locale questions those runtimes ask with the classic locale closest
//! to it (the same language: `en-GB` for English with a European region):
//! `GetUserDefaultLCID`, `GetThreadLocale`, `GetLocaleInfoA` and `W`,
//! `IsValidLocale` and `GetUserDefaultLocaleName` jump to replacements that
//! answer through other system functions (`LCIDToLocaleName`,
//! `GetLocaleInfoEx`, `IsValidLocaleName`), never through themselves. It
//! happens in this short-lived 32-bit helper, which loads nothing but the
//! scanner's driver; the app, other programs and the person's settings are
//! untouched.

use std::sync::OnceLock;

use windows_sys::Win32::Globalization::{
    GetLocaleInfoEx, GetUserDefaultLCID, GetUserDefaultLocaleName, IsValidLocaleName,
    LCIDToLocaleName, LocaleNameToLCID, WideCharToMultiByte,
};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::core::BOOL;

const LOCALE_USER_DEFAULT: u32 = 0x0400;
const LOCALE_RETURN_NUMBER: u32 = 0x2000_0000;
const LOCALE_USE_CP_ACP: u32 = 0x4000_0000;
const CP_ACP: u32 = 0;

/// The classic locale drivers get: its ID and name, nul-terminated.
static CLASSIC: OnceLock<(u32, Vec<u16>)> = OnceLock::new();

/// Whether a locale ID is one of the custom or transient ones the old
/// runtimes can't use.
pub fn is_custom(lcid: u32) -> bool {
    matches!(lcid, 0x0000 | 0x0c00 | 0x1000 | 0x1400)
        || (0x2000..=0x4c00).contains(&lcid) && lcid & 0x03ff == 0
}

/// The classic locale closest to `name` ("en-DE"): the same language and
/// region when Windows has it classically, else the same language in a
/// fitting region ("en-GB" for English with a European region, else the
/// language's default), else English (United States).
pub fn classic_for(name: &str, lcid_of: impl Fn(&str) -> u32) -> (String, u32) {
    let classic = |candidate: &str| {
        Some(lcid_of(candidate))
            .filter(|l| !is_custom(*l))
            .map(|l| (candidate.to_string(), l))
    };
    let mut parts = name.split('-');
    let language = parts.next().unwrap_or("en").to_ascii_lowercase();
    let region = parts.next_back().unwrap_or("").to_ascii_uppercase();
    const EUROPE: &[&str] = &[
        "AT", "BE", "BG", "CH", "CY", "CZ", "DE", "DK", "EE", "ES", "FI", "FR", "GB", "GR", "HR",
        "HU", "IE", "IT", "LT", "LU", "LV", "MT", "NL", "NO", "PL", "PT", "RO", "SE", "SI", "SK",
    ];
    classic(name)
        .or_else(|| {
            (language == "en" && EUROPE.contains(&region.as_str()))
                .then(|| classic("en-GB"))
                .flatten()
        })
        .or_else(|| classic(&language))
        .or_else(|| classic("en-US"))
        .unwrap_or_else(|| ("en-US".into(), 0x0409))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn classic() -> Option<&'static (u32, Vec<u16>)> {
    CLASSIC.get().filter(|(lcid, _)| *lcid != 0)
}

/// The locale a driver asked about, as the name to ask Windows with: the
/// classic one for the user's default or a custom ID.
fn name_for(lcid: u32) -> Vec<u16> {
    if let Some((classic, name)) = classic()
        && (lcid == LOCALE_USER_DEFAULT || is_custom(lcid) || lcid == *classic)
    {
        return name.clone();
    }
    let mut buffer = [0u16; 85];
    // SAFETY: a buffer of LOCALE_NAME_MAX_LENGTH.
    let n = unsafe { LCIDToLocaleName(lcid, buffer.as_mut_ptr(), buffer.len() as i32, 0) };
    if n > 0 {
        buffer[..n as usize].to_vec()
    } else {
        vec![0]
    }
}

// The replacements. Each answers through functions it doesn't replace.

unsafe extern "system" fn get_user_default_lcid() -> u32 {
    classic().map_or(LOCALE_USER_DEFAULT, |(lcid, _)| *lcid)
}

unsafe extern "system" fn get_locale_info_w(
    lcid: u32,
    kind: u32,
    data: *mut u16,
    size: i32,
) -> i32 {
    let name = name_for(lcid);
    // SAFETY: the driver's own buffer and size, asked by name instead.
    unsafe { GetLocaleInfoEx(name.as_ptr(), kind & !LOCALE_USE_CP_ACP, data, size) }
}

unsafe extern "system" fn get_locale_info_a(lcid: u32, kind: u32, data: *mut u8, size: i32) -> i32 {
    let name = name_for(lcid);
    let kind = kind & !LOCALE_USE_CP_ACP;
    // SAFETY: as above; the answer comes back wide and is narrowed for the
    // driver, or as the four bytes of a number.
    unsafe {
        if kind & LOCALE_RETURN_NUMBER != 0 {
            let mut number = [0u16; 2];
            if GetLocaleInfoEx(name.as_ptr(), kind, number.as_mut_ptr(), 2) == 0 {
                return 0;
            }
            if size == 0 {
                return 4;
            }
            if size < 4 || data.is_null() {
                return 0;
            }
            std::ptr::copy_nonoverlapping(number.as_ptr().cast::<u8>(), data, 4);
            return 4;
        }
        let needed = GetLocaleInfoEx(name.as_ptr(), kind, std::ptr::null_mut(), 0);
        if needed <= 0 {
            return 0;
        }
        let mut text = vec![0u16; needed as usize];
        let got = GetLocaleInfoEx(name.as_ptr(), kind, text.as_mut_ptr(), needed);
        if got <= 0 {
            return 0;
        }
        WideCharToMultiByte(
            CP_ACP,
            0,
            text.as_ptr(),
            got,
            if size == 0 {
                std::ptr::null_mut()
            } else {
                data
            },
            size,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    }
}

unsafe extern "system" fn is_valid_locale(lcid: u32, _flags: u32) -> BOOL {
    let name = name_for(lcid);
    // SAFETY: a nul-terminated locale name.
    unsafe { IsValidLocaleName(name.as_ptr()) }
}

unsafe extern "system" fn get_user_default_locale_name(buffer: *mut u16, size: i32) -> i32 {
    let Some((_, name)) = classic() else {
        return 0;
    };
    let len = name.len() as i32; // with its nul
    if size < len || buffer.is_null() {
        return 0;
    }
    // SAFETY: the caller's buffer, at least `len` long.
    unsafe { std::ptr::copy_nonoverlapping(name.as_ptr(), buffer, name.len()) };
    len
}

/// Points a system function at a replacement with a jump at its entry.
///
/// SAFETY: `target` is the entry of a function with the replacement's
/// signature, and nothing calls the original afterwards except through the
/// replacement.
unsafe fn redirect(target: *mut u8, replacement: usize) -> bool {
    #[cfg(target_arch = "x86")]
    let code: Vec<u8> = {
        // jmp rel32
        let rel = (replacement as isize - (target as isize + 5)) as i32;
        let mut code = vec![0xe9];
        code.extend_from_slice(&rel.to_le_bytes());
        code
    };
    #[cfg(not(target_arch = "x86"))]
    let code: Vec<u8> = {
        // jmp [rip+0], the absolute address after it
        let mut code = vec![0xff, 0x25, 0, 0, 0, 0];
        code.extend_from_slice(&(replacement as u64).to_le_bytes());
        code
    };
    unsafe {
        let mut old = 0;
        if VirtualProtect(target.cast(), code.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        std::ptr::copy_nonoverlapping(code.as_ptr(), target, code.len());
        VirtualProtect(target.cast(), code.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), target.cast(), code.len());
    }
    true
}

/// Gives drivers a classic locale when the person's has no classic ID.
/// Call once, before loading any driver. Returns what it did, in words.
pub fn install() -> Option<String> {
    // SAFETY: plain locale queries.
    let user = unsafe { GetUserDefaultLCID() };
    if !is_custom(user) {
        let _ = CLASSIC.set((0, vec![0]));
        return None;
    }
    let mut buffer = [0u16; 85];
    // SAFETY: a buffer of LOCALE_NAME_MAX_LENGTH.
    let n = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    let name = String::from_utf16_lossy(&buffer[..(n.max(1) - 1) as usize]);
    // SAFETY: a nul-terminated locale name.
    let (classic, lcid) = classic_for(&name, |candidate| unsafe {
        LocaleNameToLCID(wide(candidate).as_ptr(), 0)
    });
    if CLASSIC.set((lcid, wide(&classic))).is_err() {
        return None;
    }
    let replacements: [(&[u8], usize); 6] = [
        (
            b"GetUserDefaultLCID\0",
            get_user_default_lcid as *const () as usize,
        ),
        (
            b"GetThreadLocale\0",
            get_user_default_lcid as *const () as usize,
        ),
        (b"GetLocaleInfoA\0", get_locale_info_a as *const () as usize),
        (b"GetLocaleInfoW\0", get_locale_info_w as *const () as usize),
        (b"IsValidLocale\0", is_valid_locale as *const () as usize),
        (
            b"GetUserDefaultLocaleName\0",
            get_user_default_locale_name as *const () as usize,
        ),
    ];
    let mut done = Vec::new();
    for module in ["kernel32.dll", "kernelbase.dll"] {
        // SAFETY: both are loaded in every process.
        let handle = unsafe { GetModuleHandleW(wide(module).as_ptr()) };
        if handle.is_null() {
            continue;
        }
        for (function, replacement) in replacements {
            // SAFETY: an export of a loaded module, by name.
            if let Some(address) = unsafe { GetProcAddress(handle, function.as_ptr()) } {
                let address = address as *mut u8;
                // SAFETY: the export's entry, with the replacement's signature.
                if !done.contains(&(address as usize)) && unsafe { redirect(address, replacement) }
                {
                    done.push(address as usize);
                }
            }
        }
    }
    Some(format!(
        "{name} has no classic locale ID; drivers get {classic} ({} entries redirected)",
        done.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcid_of(name: &str) -> u32 {
        match name {
            "en-GB" => 0x0809,
            "en-US" => 0x0409,
            "en" => 0x0009,
            "de-DE" => 0x0407,
            "de" => 0x0007,
            _ => 0x1000,
        }
    }

    #[test]
    fn hybrid_locales_get_the_closest_classic_one() {
        assert_eq!(classic_for("en-DE", lcid_of), ("en-GB".into(), 0x0809));
        assert_eq!(classic_for("de-DE", lcid_of), ("de-DE".into(), 0x0407));
        assert_eq!(classic_for("en-SG", lcid_of), ("en".into(), 0x0009));
        assert_eq!(classic_for("xx-YY", lcid_of), ("en-US".into(), 0x0409));
    }

    #[test]
    fn custom_ids_are_told_apart() {
        assert!(is_custom(0x1000) && is_custom(0x0c00) && is_custom(0x2000) && is_custom(0x2400));
        assert!(!is_custom(0x0407) && !is_custom(0x0809) && !is_custom(0x0009));
    }
}
