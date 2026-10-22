# Scanning

Chat with Work Desktop scans paper with a scanner attached to your computer, makes it a searchable PDF on the computer, and sends it to Chat with Work: into a new chat, or to a bot that takes scans (a bot whose trigger is "A scan from Chat with Work Desktop", such as the Mail scanner recipe, which files each letter in Nextcloud and tells you what to do about it).

**Scans stay on your computer until you send them**, and you can use the scanner without Chat with Work at all: Save to Computer keeps the searchable PDF on your computer. The scanner is driven by the system's own scanning service, the text is read on your computer, and the pages are kept in a private folder of the app's own until you send them or close the window. Chat with Work's servers do no OCR.

Status: built on Linux (SANE and Tesseract). macOS (ImageCaptureCore and Vision) and Windows (WIA, TWAIN, and `Windows.Media.Ocr`) are next, behind the same interface.

## Using it

Choose **Scan…** in the tray menu (or run `cww-app --scan`).

- **Settings**, remembered between scans: the scanner, color mode (black and white, grayscale, color), resolution (300 dpi suits letters), both sides (on by default, where the scanner can), remove blank pages (on by default), and make text searchable (on by default) in a language you pick. The language isn't detected: guessing would mean reading every page several times.
- **Scan** reads the feeder until it's empty (or one page from a flatbed). Pages appear as they arrive. Blank pages are left out but kept below the pages, where **Keep** brings one back.
- **Each page** can be moved earlier or later (or dragged), turned, cropped, or deleted. Black and white pages show as they'll be sent.
- **Rescan** replaces the pages; **Scan More Pages** adds to them.
- **Finishing** is one split button. Its main part is **Send to Chat with Work**, and its arrow opens **Save to Computer**. On a computer that isn't paired with Chat with Work, or may not use your chats, the main part is Save to Computer and Send waits in the menu, saying what it needs and opening Settings.
- **Save to Computer** makes the PDF (reading each page's text first, a second or two a page) and saves it where you pick, named after the date and the page count (`Scan 2026-10-22 14.05, 3 pages.pdf`). The dialog starts in the folder you saved to last. It needs no Chat with Work account: the computer doesn't have to be paired.
- **Send to Chat with Work** makes the same PDF and sends it to a new chat, with an optional message, or to a bot. Sending needs the computer paired and allowed to use your chats.
- The pages stay until you scan again or close the window, so a scan can be saved and sent, or sent to more than one place. Sending the same pages to the same bot again is recognized as the same scan.

Chat with Work takes files of at most 25 MB: about 30 letter pages in grayscale at 300 dpi. A larger scan says so before anything is sent.

## What you need

| | Scanner | Searchable text |
|---|---|---|
| Linux | SANE (`sane` on Arch, `libsane1` on Debian and Ubuntu, `sane-backends-libs` on Fedora), with your scanner's backend; the packages recommend it | Tesseract and a language (`tesseract` and `tesseract-data-deu` on Arch, `tesseract-ocr` and `tesseract-ocr-deu` on Debian and Ubuntu, `tesseract` and `tesseract-langpack-deu` on Fedora) |
| macOS | Next: the scanner's ImageCaptureCore (ICA) driver from its maker | Next: built in (Vision) |
| Windows | Next: the scanner's WIA or TWAIN driver | Next: built in (`Windows.Media.Ocr`) |

`scanimage -L` lists the scanners SANE sees. A Canon P-208 II works with SANE's `canon_dr` backend once its "Auto Start" switch is off. If SANE sees a scanner but the app says you can't use it, your user may need to be in the `scanner` group, or the scanner plugged in again after installing SANE.

## How it works

```
scan page (bundled, scan.html) ──commands──▶ cww-app (scan.rs)
                                               │
                     JSON lines on a pipe      ▼
                                       cww-app --scan-helper (cww-scan)
                                               │  SANE / ImageCaptureCore / WIA / TWAIN
                                               ▼
                                            scanner
cww-app: blank pages, crop and turn, OCR, the PDF ──control socket──▶ cww daemon ──DPoP──▶ Chat with Work
```

- **`cww-scan`** (`scan/`) holds everything that doesn't depend on the window: the `Backend` trait (`devices`, `capabilities`, `scan` streaming pages), the SANE backend, blank-page detection (`blank.rs`), crops and turns (`edit.rs`), OCR engines (`ocr.rs`) and the PDF writer (`pdf.rs`).
- **The helper process.** Scanner drivers are vendor code that hangs and crashes, so the backend runs in `cww-app --scan-helper`, started by the app while the scan window is open and stopped when it closes. It writes each page as a PNM file into the session's folder and says so on its stdout (`helper.rs` documents the protocol). A driver that crashes takes only the helper; the app starts it again on the next request. The daemon never loads a driver.
- **SANE** is the distribution's `libsane.so.1`, loaded at run time with `dlopen`, never shipped (its backends are GPL with an exception for programs that load them). Our binding is the dozen SANE 1.0 functions a frontend needs. Black and white is scanned in grey and thresholded on the computer (Otsu's method), so it looks the same whatever the scanner's own "lineart" does. Feeders are scanned to `sane_start`'s "no documents"; duplex uses the source named "Duplex" (`ADF Duplex` on Canon's); `swcrop` and `swdeskew` are turned on where a backend offers them (canon_dr, fujitsu), so sheet-fed pages come cropped to the paper and straight.
- **Blank pages** are judged in grey at 150 dpi, without their margins (hole punches, staples, the feeder's shadow), as a grid of 4 mm cells: a cell holds ink when 2% of its pixels are much darker than the paper. A page with no more than two such cells is blank. Show-through from the other side is too light to count. `cargo run -p cww-scan --example blank -- <dpi> <pages>` says what it thinks of real scans.
- **OCR** returns words with their boxes. On Linux it's the `tesseract` command (TSV output, one process per page, up to four pages at once). The text layer is written by `pdf.rs`: each word as invisible text (render mode 3) in Tesseract's glyph-less font (`scan/assets/glyphless.ttf`, Apache-2.0), sized to its line and stretched to its box, over the page's image, with a ToUnicode map, so any language copies out as it was read. No Ghostscript, no Python, no OCRmyPDF.
- **The PDF** keeps grey and color pages as JPEG (quality 80) and black and white pages as one bit a pixel, at the resolution they were scanned at.
- **Sending** goes through the daemon, which holds the device's DPoP-bound token; the app sends it the PDF over the control socket (`scan_send`, CONTROL.md). A new chat is an upload and a new chat, as the terminal does; a bot is a trigger event (`POST /local_agent/scans`, PROTOCOL.md section 13). Either needs the computer to be paired and allowed to use your chats.
- **The pages** live in `<state dir>/scans/<random id>/` (0700) while the window is open. Closing the window, Rescan, and the app's next start delete them. Saving writes only the PDF you asked for, where you asked; the folder is remembered in `scan.json` beside the app's settings.

## Developing without a scanner

- `CWW_SCAN_SAMPLE_DIR=<folder>` replaces the system's scanners with "Sample scanner", whose feeder holds the folder's PNG and JPEG files in name order (one-sided scans take every other one), at `CWW_SCAN_SAMPLE_DPI` (300).
- SANE's own `test` backend is a fake scanner with a feeder that empties after ten pages. Enable it in a `dll.conf` of your own (`SANE_CONFIG_DIR`), and set `CWW_SCAN_TEST_DEVICES=1`, since the app hides test devices otherwise. `scan/tests/sane_test_device.rs` scans it end to end; CI runs it with `CWW_SCAN_REQUIRE_SANE=1`, so a missing SANE fails rather than skips.
- `cargo run -p cww-scan --example searchable -- 300 deu out.pdf page.png` makes a searchable PDF from images, to try OCR and the text layer.
- In a `--demo` build, `CWW_APP_SNAPSHOT_PAGES=scan,scan:scanned,scan:blank,scan:turned,scan:crop,scan:cropped,scan:saved,scan:send` takes pictures of each step (see `snapshot.rs`), and `CWW_DEMO_SCAN_OUT=<file>` keeps the PDF the demo daemon was sent, and `CWW_DEMO_SAVE_TO=<file>` answers the save dialog.

## Not done yet

- macOS (ImageCaptureCore, Vision) and Windows (WIA, and TWAIN in a 32-bit worker where the driver is 32-bit, with the vendor's own window hidden).
- A preview scan for flatbeds, and the scanner's own button starting a scan.
- A watched folder for scanners that save files themselves, and eSCL (AirScan) for network scanners.
