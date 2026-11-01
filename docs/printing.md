# Printing

Chat with Work can print a PDF from a chat's files on a printer attached to this computer, or one this computer already knows on your network: a guest list before a meetup, a form to sign, a letter. It prints through the system's own printing, so it works with every printer you can already print to.

**Nothing prints here unless you say so on this computer.** You choose which printers Chat with Work may use, and for whom. The server can't add a printer, lend one, or let a job print unattended.

Status: built on Linux and macOS (CUPS) and Windows (the print spooler). IPP Everywhere, for a network printer the system doesn't have, isn't built ([Not built yet](#not-built-yet)). Tested for real on 2026-11-01: on Linux, jobs through a private `cupsd` and its real `lp`, `lpstat` and `lpoptions` into a backend that keeps the job (`tests/printing_cups.rs`); on macOS 27, a job through its CUPS to a queue on `/dev/null`, which CUPS reported completed; on Windows 11, jobs through the spooler to "Microsoft Print to PDF", every page and a page list. No paper was used.

## Allowing a printer

In the app: **Settings, This computer, Printers** turns a printer on for your chats. Each bot's page has **Share this computer**, which lends a printer to that bot and, separately, lets it print there unattended. Both ask you first, in a dialog the app draws.

From the command line:

```
cww printers                                  # what this computer has, and who may use it
cww printers add Office_Brother               # your chats may print there
cww printers add Office_Brother --bots-only   # only the bots you lend it to
cww printers share office-brother 42 --name "Door list"
cww printers share office-brother 42 --unattended
cww printers unshare office-brother 42
cww printers remove office-brother
```

A bot's number is the one in its address in Chat with Work.

## Who approves

Every job is approved in Chat with Work, on the web or in any of its apps, by a card that shows the file, its pages, the copies and the printer. Unlike a change to a file, a job can't be allowed for the rest of a chat, and no permission mode runs it unasked.

The one exception is yours to make, here: **unattended** lets one bot print on one printer without anyone approving each job. That's what a list that prints itself three hours before every meetup needs. A person's own chats never print unattended.

The daemon can't tell an approval from a forged one, so what it holds to is what you set here: which printer, for whom, and the day's pages. A compromised server could print on a printer you allowed; it couldn't reach another one, or print past the limit.

## The day's pages

This computer prints at most 200 pages in any 24 hours, copies counted, however many printers it has, so a bot gone wrong can't empty a tray. It's `print_pages_per_day` under `[limits]` in `config.toml`; 0 means no limit. Past it, a job is refused and says how many pages are left.

## How it works

```
Chat with Work ──print_bytes, 64 KiB at a time──▶ cww daemon (confined)
               ──print──────────────────────────▶ cww daemon: reads the PDF, counts its pages
                                                        │ one JSON line
                                                        ▼
                                                  supervisor: checks everything again
                                                        │ lp, or the spooler
                                                        ▼
                                                     printer
```

- **The file** comes over the tunnel in chunks, into `<state dir>/printing/` (0700), only after the person approved the job: a dry run needs no file, so the card is made with nothing sent. It must have the SHA-256 it was announced with and be a PDF. It's deleted when it's printed, or after an hour.
- **The confined daemon** reads the PDF to count its pages, as it reads any document, inside its sandbox. An encrypted PDF, or one it can't read, doesn't print.
- **The supervisor** does the printing, since the sandbox allows neither `lp` nor the spooler's sockets. It's told a printer's ID and a file's ID, reads `config.toml` again, and makes every check itself. A job's title and page list reach `lp` as arguments of their own, after checks, never through a shell.
- **CUPS** (Linux, macOS): `lpstat` and `lpoptions` list printers, `lp -d <queue> -n <copies> -t <title> [-o sides=...] [-P <pages>] -- <file>` prints. CUPS turns the PDF into what the printer takes. On Linux that's the `cups-client` package (`cups` on Arch and Fedora); without it there are no printers to allow, and `cww printers` says so.
- **The Windows spooler**: printers come from `EnumPrintersW`. A spooler takes what its driver takes, which isn't a PDF, so each page is drawn by Windows' own renderer (`Windows.Data.Pdf`) at the printer's resolution, at most 300 dpi, and sent through GDI, with copies and sides on the job's settings. That works with every driver, and adds nothing to the installer. Printers that ask where to save in a dialog, such as "Microsoft Print to PDF" (their port is `PORTPROMPT:`), aren't listed: a background agent can't answer one.
- **Once only**: tools run at least once, so a printed job keeps a receipt for a day, by the same transfer id. A call made again answers from it (`already: true`) and prints nothing.
- **The audit log** says a job printed, on which printer, its pages, copies and job number, and for which bot. Never the file.

The wire details are in [PROTOCOL.md, section 8.21](../PROTOCOL.md#821-printing), the settings in [CONTROL.md, Printers](../CONTROL.md#printers).

## Developing without paper

- `cargo test --test printing_cups` runs a private `cupsd` of the test's own on a port of its own, whose only printer is a backend that keeps the job in a file, so it can't reach a real printer. Set `CWW_PRINT_REQUIRE_CUPS=1` to fail, not skip, without a `cupsd` this user can run (Arch installs it readable by root only).
- `cargo run --example print_file` lists the system's printers, and `print_file <queue> <file.pdf> [pages]` prints without a daemon. Point `CUPS_SERVER` at a private server first.
- On Windows, `CWW_PRINT_TO_FILE=C:\path\out.pdf` lists printers like "Microsoft Print to PDF" and sends the job to that file.

## What it doesn't protect against

- **A job can't be called back from here.** Once the system has it, cancel it in the system's print queue.
- **A queued job isn't a printed one.** `queued` means the system took it. Out of paper or a jam shows at the printer, and in the system's queue.
- **Unattended means unattended.** A bot can print at night, up to the day's pages. Quiet hours aren't built.

## Not built yet

- IPP Everywhere directly, for a network printer the system doesn't have: a printer without PDF needs its pages rendered first. Add the printer to the system instead.
- Trays, paper sizes and colour choices: a job uses the printer's own defaults.
