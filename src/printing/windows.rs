//! The Windows print spooler. Printers come from `EnumPrintersW`. A spooler
//! takes what its driver takes, which isn't a PDF, so each page is drawn by
//! Windows' own PDF renderer (`Windows.Data.Pdf`) at the printer's resolution
//! and sent through GDI, which works with every driver there is, the IPP
//! class driver and "Microsoft Print to PDF" among them, and adds nothing to
//! the installer.

use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat};
use windows::Storage::Streams::{Buffer, DataReader, InMemoryRandomAccessStream};
use windows::Storage::{FileAccessMode, StorageFile};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DEVMODEW, DIB_RGB_COLORS, DM_COPIES,
    DM_DUPLEX, DMDUP_SIMPLEX, DMDUP_VERTICAL, DeleteDC, GetDeviceCaps, HALFTONE, HDC, HORZRES,
    LOGPIXELSX, LOGPIXELSY, SRCCOPY, SetStretchBltMode, StretchDIBits, VERTRES,
};
use windows::Win32::Graphics::Printing::{
    ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW,
    PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE, PRINTER_INFO_2W,
};
use windows::Win32::Storage::Xps::{
    AbortDoc, DC_COLORDEVICE, DC_DUPLEX, DOCINFOW, DeviceCapabilitiesW, EndDoc, EndPage, StartDocW,
    StartPage,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::{Job, SystemPrinter};
use crate::commands::RunError;

const PRINTER_STATUS_PAUSED: u32 = 0x1;
const PRINTER_STATUS_ERROR: u32 = 0x2;
const PRINTER_STATUS_PRINTING: u32 = 0x400;
const PRINTER_STATUS_OFFLINE: u32 = 0x80;
const PRINTER_ATTRIBUTE_WORK_OFFLINE: u32 = 0x400;
/// Where a job goes instead of the printer's port, for tests.
const TO_FILE: &str = "CWW_PRINT_TO_FILE";
const DM_OUT_BUFFER: u32 = 2;
const DM_IN_BUFFER: u32 = 8;

fn failed(what: &str) -> RunError {
    RunError::new("unavailable", format!("Windows couldn't {what}"))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn printers() -> Result<Vec<SystemPrinter>, RunError> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let (mut needed, mut count) = (0u32, 0u32);
    // SAFETY: asks only for the size needed, with no buffer.
    let _ = unsafe { EnumPrintersW(flags, PCWSTR::null(), 2, None, &mut needed, &mut count) };
    if needed == 0 {
        return Ok(Vec::new());
    }
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer is `needed` bytes, aligned for PRINTER_INFO_2W.
    unsafe {
        let bytes =
            std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), needed as usize);
        EnumPrintersW(
            flags,
            PCWSTR::null(),
            2,
            Some(bytes),
            &mut needed,
            &mut count,
        )
    }
    .map_err(|_| failed("list its printers"))?;
    let default = default_printer();
    // "Microsoft Print to PDF" and its like ask where to save in a dialog,
    // which a background agent can't answer. They're no printers for it,
    // but for trying this file without paper (docs/printing.md).
    let to_file = std::env::var_os(TO_FILE).is_some();
    // SAFETY: Windows wrote `count` PRINTER_INFO_2W at the buffer's start,
    // their strings further in, all alive until the buffer goes.
    let infos = unsafe {
        std::slice::from_raw_parts(buffer.as_ptr().cast::<PRINTER_INFO_2W>(), count as usize)
    };
    Ok(infos
        .iter()
        .filter_map(|info| {
            // SAFETY: a string in the buffer.
            let queue = unsafe { info.pPrinterName.to_string() }.ok()?;
            let port = info.pPortName;
            // SAFETY: as above.
            let saves_to_a_file =
                unsafe { port.to_string() }.is_ok_and(|port| port == "PORTPROMPT:");
            if saves_to_a_file && !to_file {
                return None;
            }
            let name = wide(&queue);
            let capability = |which| {
                // SAFETY: both strings end in NUL, and these capabilities
                // write nothing.
                unsafe {
                    DeviceCapabilitiesW(PCWSTR(name.as_ptr()), PCWSTR(port.0), which, None, None)
                }
            };
            let stopped = info.Status
                & (PRINTER_STATUS_PAUSED | PRINTER_STATUS_ERROR | PRINTER_STATUS_OFFLINE)
                != 0
                || info.Attributes & PRINTER_ATTRIBUTE_WORK_OFFLINE != 0;
            Some(SystemPrinter {
                default: default.as_deref() == Some(queue.as_str()),
                description: None,
                state: if stopped {
                    "stopped"
                } else if info.Status & PRINTER_STATUS_PRINTING != 0 {
                    "printing"
                } else {
                    "idle"
                }
                .into(),
                duplex: capability(DC_DUPLEX) == 1,
                color: capability(DC_COLORDEVICE) == 1,
                queue,
            })
        })
        .collect())
}

fn default_printer() -> Option<String> {
    let mut size = 0u32;
    // SAFETY: asks for the size, then fills a buffer of that size.
    unsafe {
        let _ = GetDefaultPrinterW(None, &mut size);
        if size == 0 || GetLastError() != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
        let mut name = vec![0u16; size as usize];
        GetDefaultPrinterW(Some(PWSTR(name.as_mut_ptr())), &mut size)
            .as_bool()
            .then(|| String::from_utf16_lossy(&name[..size.saturating_sub(1) as usize]))
    }
}

/// The printer's own settings with the job's copies and sides on them.
fn job_settings(queue: &[u16], job: &Job) -> Result<Vec<u64>, RunError> {
    let mut printer = PRINTER_HANDLE::default();
    // SAFETY: `queue` ends in NUL; the handle is closed below.
    unsafe { OpenPrinterW(PCWSTR(queue.as_ptr()), &mut printer, None) }
        .map_err(|_| failed("open that printer"))?;
    let close = |printer| {
        // SAFETY: opened above.
        let _ = unsafe { ClosePrinter(printer) };
    };
    // SAFETY: with no buffers, this asks for the size of the settings.
    let size = unsafe { DocumentPropertiesW(None, printer, PCWSTR(queue.as_ptr()), None, None, 0) };
    if size <= 0 {
        close(printer);
        return Err(failed("read that printer's settings"));
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    let devmode = buffer.as_mut_ptr().cast::<DEVMODEW>();
    // SAFETY: the buffer is as large as Windows said, and aligned for DEVMODEW.
    let read = unsafe {
        DocumentPropertiesW(
            None,
            printer,
            PCWSTR(queue.as_ptr()),
            Some(devmode),
            None,
            DM_OUT_BUFFER,
        )
    };
    if read < 0 {
        close(printer);
        return Err(failed("read that printer's settings"));
    }
    // SAFETY: Windows filled it; the fields set are within DEVMODEW.
    unsafe {
        (*devmode).dmFields |= DM_COPIES;
        (*devmode).Anonymous1.Anonymous1.dmCopies = job.copies.min(i16::MAX as u32) as i16;
        if let Some(duplex) = job.duplex {
            (*devmode).dmFields |= DM_DUPLEX;
            (*devmode).dmDuplex = if duplex {
                DMDUP_VERTICAL
            } else {
                DMDUP_SIMPLEX
            };
        }
    }
    // SAFETY: as above; merges the fields set into what the driver keeps.
    let merged = unsafe {
        DocumentPropertiesW(
            None,
            printer,
            PCWSTR(queue.as_ptr()),
            Some(devmode),
            Some(devmode),
            DM_IN_BUFFER | DM_OUT_BUFFER,
        )
    };
    close(printer);
    if merged < 0 {
        return Err(failed("use those settings on that printer"));
    }
    Ok(buffer)
}

/// The pages a job prints, from 0, of a file of `total`.
fn page_numbers(pages: Option<&str>, total: u32) -> Vec<u32> {
    let Some(list) = pages else {
        return (0..total).collect();
    };
    super::tools::parse_pages(list)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|(from, to)| (from - 1)..to.min(total))
        .collect()
}

struct Document(HDC);

impl Drop for Document {
    fn drop(&mut self) {
        // SAFETY: the context made in `submit`, used by nothing else now.
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// Draw one page across the printable area, as large as it fits, centred.
fn draw(hdc: HDC, document: &PdfDocument, index: u32) -> windows::core::Result<()> {
    let page = document.GetPage(index)?;
    let size = page.Size()?;
    // SAFETY: a printer's device context.
    let (area_x, area_y, dpi_x, dpi_y) = unsafe {
        (
            GetDeviceCaps(Some(hdc), HORZRES),
            GetDeviceCaps(Some(hdc), VERTRES),
            GetDeviceCaps(Some(hdc), LOGPIXELSX),
            GetDeviceCaps(Some(hdc), LOGPIXELSY),
        )
    };
    // Rendered only as large as it prints, at most 300 dpi, whatever size
    // the PDF claims: a page can say it's 200 inches wide. Its size comes in
    // units of 1/96 inch here.
    let (page_width, page_height) = (size.Width.max(1.0) as f64, size.Height.max(1.0) as f64);
    let fit = (area_x as f64 / page_width).min(area_y as f64 / page_height);
    let scale = fit.min(dpi_x.min(dpi_y).clamp(72, 300) as f64 / 96.0);
    let width = (page_width * scale).round().max(1.0) as u32;
    let height = (page_height * scale).round().max(1.0) as u32;

    let stream = InMemoryRandomAccessStream::new()?;
    let options = PdfPageRenderOptions::new()?;
    options.SetDestinationWidth(width)?;
    options.SetDestinationHeight(height)?;
    page.RenderWithOptionsToStreamAsync(&stream, &options)?
        .join()?;
    stream.Seek(0)?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let bitmap = decoder
        .GetSoftwareBitmapConvertedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Ignore)?
        .join()?;
    let (pixel_width, pixel_height) = (bitmap.PixelWidth()?, bitmap.PixelHeight()?);
    let bytes = u64::from(pixel_width.unsigned_abs()) * u64::from(pixel_height.unsigned_abs()) * 4;
    let bytes = u32::try_from(bytes).map_err(|_| windows::core::Error::from_thread())?;
    let buffer = Buffer::Create(bytes)?;
    bitmap.CopyToBuffer(&buffer)?;
    let reader = DataReader::FromBuffer(&buffer)?;
    let mut pixels = vec![0u8; buffer.Length()? as usize];
    reader.ReadBytes(&mut pixels)?;

    let scale = (area_x as f64 / pixel_width as f64).min(area_y as f64 / pixel_height as f64);
    let (out_x, out_y) = (
        (pixel_width as f64 * scale) as i32,
        (pixel_height as f64 * scale) as i32,
    );
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: pixel_width,
            biHeight: -pixel_height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `pixels` is the top-down 32-bit bitmap `info` describes.
    let drawn = unsafe {
        SetStretchBltMode(hdc, HALFTONE);
        StretchDIBits(
            hdc,
            (area_x - out_x) / 2,
            (area_y - out_y) / 2,
            out_x,
            out_y,
            0,
            0,
            pixel_width,
            pixel_height,
            Some(pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        )
    };
    if drawn == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

pub fn submit(job: &Job) -> Result<String, RunError> {
    // SAFETY: readies this thread for WinRT; already being ready is fine.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let queue = wide(&job.queue);
    let settings = job_settings(&queue, job)?;
    let path = std::fs::canonicalize(&job.file).map_err(|_| failed("find the file to print"))?;
    let path = path
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();
    let open = || -> windows::core::Result<PdfDocument> {
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_str()))?.join()?;
        let stream = file.OpenAsync(FileAccessMode::Read)?.join()?;
        PdfDocument::LoadFromStreamAsync(&stream)?.join()
    };
    let document = open().map_err(|_| failed("read that PDF"))?;
    let total = document.PageCount().map_err(|_| failed("read that PDF"))?;

    // SAFETY: `queue` ends in NUL, and `settings` holds a DEVMODEW.
    let hdc = unsafe {
        CreateDCW(
            PCWSTR::null(),
            PCWSTR(queue.as_ptr()),
            PCWSTR::null(),
            Some(settings.as_ptr().cast()),
        )
    };
    if hdc.is_invalid() {
        return Err(failed("open that printer"));
    }
    let context = Document(hdc);
    let title = wide(&job.title);
    let output = std::env::var_os(TO_FILE).map(|file| wide(&file.to_string_lossy()));
    let info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: PCWSTR(title.as_ptr()),
        lpszOutput: output
            .as_ref()
            .map_or(PCWSTR::null(), |file| PCWSTR(file.as_ptr())),
        ..Default::default()
    };
    // SAFETY: `info`'s strings outlive the call.
    let id = unsafe { StartDocW(context.0, &info) };
    if id <= 0 {
        return Err(failed("start the job"));
    }
    for index in page_numbers(job.pages.as_deref(), total) {
        // SAFETY: a document is open on this context.
        let drawn = unsafe {
            (StartPage(context.0) > 0)
                .then(|| draw(context.0, &document, index))
                .ok_or_else(windows::core::Error::from_thread)
                .and_then(|drawn| drawn)
                .and_then(|()| {
                    (EndPage(context.0) > 0)
                        .then_some(())
                        .ok_or_else(windows::core::Error::from_thread)
                })
        };
        if drawn.is_err() {
            // SAFETY: as above.
            let _ = unsafe { AbortDoc(context.0) };
            return Err(failed("draw that page"));
        }
    }
    // SAFETY: as above.
    if unsafe { EndDoc(context.0) } <= 0 {
        return Err(failed("finish the job"));
    }
    Ok(id.to_string())
}
