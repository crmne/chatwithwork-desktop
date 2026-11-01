use base64::Engine;

use super::settings;
use super::tools::{count_pages, parse_pages};
use super::transfer::{self, Chunk};
use super::*;
use crate::config::{BotLoan, Printer};

fn sha(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn send(dir: &std::path::Path, id: &str, bytes: &[u8]) -> Result<bool, crate::error::ToolError> {
    let sha256 = sha(bytes);
    let mut last = false;
    for (index, chunk) in bytes.chunks(transfer::MAX_CHUNK).enumerate() {
        let data = base64::engine::general_purpose::STANDARD.encode(chunk);
        last = transfer::receive(
            dir,
            &Chunk {
                transfer: id,
                offset: (index * transfer::MAX_CHUNK) as u64,
                size: bytes.len() as u64,
                sha256: &sha256,
                data: &data,
            },
        )?
        .complete;
    }
    Ok(last)
}

const ID: &str = "0123456789abcdef";

#[test]
fn a_file_arrives_in_chunks_and_must_be_the_pdf_announced() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("printing");
    let mut pdf = b"%PDF-1.7\n".to_vec();
    pdf.resize(200_000, b'x');
    assert!(send(&dir, ID, &pdf).unwrap());
    assert_eq!(
        std::fs::read(transfer::finished(&dir, ID).unwrap()).unwrap(),
        pdf
    );

    let other = "fedcba9876543210";
    let code = send(&dir, other, b"not a pdf at all").unwrap_err().code;
    assert_eq!(code, crate::error::ErrorCode::Unsupported);
    assert!(transfer::finished(&dir, other).is_none());

    let data = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.7");
    let chunk = Chunk {
        transfer: other,
        offset: 0,
        size: 8,
        sha256: "00",
        data: &data,
    };
    assert_eq!(
        transfer::receive(&dir, &chunk).unwrap_err().code,
        crate::error::ErrorCode::InvalidArgument
    );
    assert!(transfer::finished(&dir, other).is_none());
}

#[test]
fn a_transfer_id_can_only_name_a_file_in_its_folder() {
    assert!(transfer::valid_id(ID));
    for id in [
        "../../etc/passwd",
        "short",
        "0123456789ABCDEF",
        "0123456789abcdef/",
        "",
    ] {
        assert!(!transfer::valid_id(id), "{id}");
    }
    let home = tempfile::tempdir().unwrap();
    let chunk = Chunk {
        transfer: "../../../evil",
        offset: 0,
        size: 1,
        sha256: "",
        data: "AA==",
    };
    assert!(transfer::receive(home.path(), &chunk).is_err());
}

#[test]
fn a_chunk_can_be_sent_again_but_cannot_leave_a_gap() {
    let home = tempfile::tempdir().unwrap();
    let bytes = [b'%'; 100];
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes[..10]);
    let chunk = |offset| Chunk {
        transfer: ID,
        offset,
        size: 100,
        sha256: "",
        data: &data,
    };
    transfer::receive(home.path(), &chunk(0)).unwrap();
    assert_eq!(
        transfer::receive(home.path(), &chunk(0)).unwrap().received,
        10
    );
    assert!(transfer::receive(home.path(), &chunk(50)).is_err());
}

#[test]
fn page_lists() {
    assert_eq!(parse_pages("1-3,7").unwrap(), vec![(1, 3), (7, 7)]);
    for list in ["", "0", "3-1", "a", "1,", "1-", "-1", "1;rm"] {
        assert!(parse_pages(list).is_err(), "{list}");
    }
    assert_eq!(count_pages(&[(1, 3), (7, 7)], 7).unwrap(), 4);
    assert!(count_pages(&[(1, 8)], 7).is_err());
}

#[test]
fn printer_settings_stay_with_the_people_on_the_computer() {
    let system = vec![SystemPrinter {
        queue: "Brother_HL".into(),
        description: None,
        default: true,
        state: "idle".into(),
        duplex: true,
        color: false,
    }];
    let mut config = Config::default();
    assert!(settings::add(&mut config, &system, "Nope", None, true).is_err());
    let printer = settings::add(&mut config, &system, "Brother_HL", None, true).unwrap();
    assert_eq!(printer.id, "brother-hl");
    assert_eq!(printer.label, "Brother HL");
    assert!(settings::set_bot(&mut config, "brother-hl", "fixer", None, true, false).is_err());
    let printer =
        settings::set_bot(&mut config, "brother-hl", "42", Some("Door"), true, true).unwrap();
    assert!(printer.loan("42").unwrap().unattended);
    let printer = settings::set_bot(&mut config, "brother-hl", "42", None, true, false).unwrap();
    assert_eq!(printer.loan("42").unwrap().name, "Door");
    assert!(!printer.loan("42").unwrap().unattended);
    assert_eq!(settings::offered(&config).len(), 1);
    settings::remove(&mut config, "Brother_HL").unwrap();
    assert!(settings::offered(&config).is_empty());
}

fn printer(chats: bool, bots: Vec<BotLoan>) -> Printer {
    Printer {
        id: "office".into(),
        queue: "Office".into(),
        label: "Office".into(),
        chats,
        bots,
    }
}

fn request(bot: Option<&str>, unattended: bool) -> Request {
    Request {
        printer: "office".into(),
        transfer: ID.into(),
        title: "List".into(),
        copies: 2,
        duplex: None,
        pages: None,
        pages_counted: 3,
        bot: bot.map(str::to_string),
        unattended,
    }
}

fn config_with(printer: Printer) -> Config {
    Config {
        printers: vec![printer],
        ..Config::default()
    }
}

#[test]
fn the_supervisor_checks_every_job_again() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    let code =
        |config: &Config, request: &Request| print(&paths, config, request).unwrap_err().code;

    assert_eq!(
        code(&config_with(printer(false, vec![])), &request(None, false)),
        "not_lent"
    );
    assert_eq!(
        code(
            &config_with(printer(true, vec![])),
            &request(Some("42"), false)
        ),
        "not_lent"
    );
    let attended = BotLoan {
        id: "42".into(),
        name: String::new(),
        unattended: false,
    };
    assert_eq!(
        code(
            &config_with(printer(false, vec![attended])),
            &request(Some("42"), true)
        ),
        "needs_approval"
    );
    let mut bad = request(None, false);
    bad.transfer = "../../secret".into();
    assert_eq!(
        code(&config_with(printer(true, vec![])), &bad),
        "invalid_argument"
    );
    assert_eq!(
        code(&config_with(printer(true, vec![])), &request(None, false)),
        "not_found"
    );
}

#[test]
fn the_supervisor_counts_no_job_as_nothing() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    let dir = super::dir(&paths);
    std::fs::create_dir_all(&dir).unwrap();
    send(&dir, ID, b"%PDF-1.7 whatever").unwrap();
    let config = config_with(printer(true, vec![]));
    let refused = |change: &dyn Fn(&mut Request)| {
        let mut job = request(None, false);
        change(&mut job);
        print(&paths, &config, &job).unwrap_err().code
    };

    assert_eq!(refused(&|job| job.pages_counted = 0), "invalid_argument");
    assert_eq!(refused(&|job| job.copies = 0), "invalid_argument");
    assert_eq!(refused(&|job| job.copies = 100), "invalid_argument");
    assert_eq!(
        refused(&|job| job.pages = Some("1;2".into())),
        "invalid_argument"
    );
    assert_eq!(
        refused(&|job| job.pages = Some("1-50".into())),
        "invalid_argument"
    );
}

#[test]
fn the_day_keeps_its_page_limit_and_a_receipt_answers_a_second_call() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    let dir = super::dir(&paths);
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = config_with(printer(true, vec![]));
    config.limits.print_pages_per_day = 5;
    send(&dir, ID, b"%PDF-1.7 whatever").unwrap();

    // 3 pages, 2 copies: six sheets past a limit of five.
    let error = print(&paths, &config, &request(None, false)).unwrap_err();
    assert_eq!(error.code, "rate_limited");
    assert_eq!(pages_left(&paths, &config), Some(5));

    write_receipt(
        &dir,
        ID,
        &Receipt {
            job: "Office-1".into(),
            state: "queued".into(),
            pages: 3,
            copies: 2,
            already: false,
        },
    );
    let again = print(&paths, &config, &request(None, false)).unwrap();
    assert!(again.already);
    assert_eq!(again.job, "Office-1");
}

mod tools_for_callers {
    use serde_json::json;

    use super::super::tools::PrintTools;
    use super::*;
    use crate::commands::Runner;
    use crate::error::ErrorCode;
    use crate::tools::Caller;

    fn tools() -> PrintTools {
        let home = tempfile::tempdir().unwrap();
        let loan = |id: &str, unattended| BotLoan {
            id: id.into(),
            name: String::new(),
            unattended,
        };
        PrintTools {
            runner: Runner::Off("no spooler in these tests".into()),
            printers: vec![
                printer(false, vec![loan("42", false)]),
                Printer {
                    id: "door".into(),
                    queue: "Door".into(),
                    label: "Door".into(),
                    chats: true,
                    bots: vec![loan("7", true)],
                },
            ],
            dir: home.keep(),
        }
    }

    fn bot(id: &str, unattended: bool) -> Caller {
        Caller::Bot {
            id: id.into(),
            unattended,
        }
    }

    async fn refused(tools: &PrintTools, printer: &str, caller: Caller) -> ErrorCode {
        tools
            .call(
                "print",
                json!({ "printer": printer, "transfer": ID }),
                &caller,
            )
            .await
            .unwrap_err()
            .code
    }

    #[tokio::test]
    async fn only_who_the_owner_allowed_reaches_a_printer() {
        let tools = tools();
        // Lent to bot 42 only: a person's chat, or another bot, can't.
        assert_eq!(
            refused(&tools, "office", Caller::Person).await,
            ErrorCode::NotFound
        );
        assert_eq!(
            refused(&tools, "office", bot("7", false)).await,
            ErrorCode::NotLent
        );
        // The server says nobody approved, and the loan doesn't allow that.
        assert_eq!(
            refused(&tools, "office", bot("42", true)).await,
            ErrorCode::NeedsApproval
        );
    }

    #[tokio::test]
    async fn a_job_is_checked_before_anything_is_asked_of_the_system() {
        let tools = tools();
        for (arguments, code) in [
            (
                json!({ "printer": "door", "copies": 0, "dry_run": true }),
                ErrorCode::InvalidArgument,
            ),
            (
                json!({ "printer": "door", "copies": 100, "dry_run": true }),
                ErrorCode::InvalidArgument,
            ),
            (
                json!({ "printer": "door", "pages": "1;2", "dry_run": true }),
                ErrorCode::InvalidArgument,
            ),
            (json!({ "printer": "door" }), ErrorCode::InvalidArgument),
            (
                json!({ "printer": "door", "transfer": "../../x" }),
                ErrorCode::InvalidArgument,
            ),
            (
                json!({ "printer": "door", "queue": "evil" }),
                ErrorCode::InvalidArgument,
            ),
        ] {
            let error = tools
                .call("print", arguments, &Caller::Person)
                .await
                .unwrap_err();
            assert_eq!(error.code, code, "{}", error.message);
        }
    }
}

#[cfg(unix)]
mod cups_output {
    use super::super::cups::*;
    use super::*;

    #[test]
    fn reads_what_lpstat_and_lpoptions_print() {
        assert_eq!(
            parse_default("system default destination: Brother\n").as_deref(),
            Some("Brother")
        );
        assert_eq!(parse_default("no system default destination\n"), None);
        assert_eq!(
            parse_states(
                "printer Brother is idle.  enabled since Sun 01 Nov 2026\n\
                 printer Hp now printing Hp-12.  enabled since Sun\n\
                 printer Old disabled since Sun 01 Nov 2026 -\n\treason unknown\n"
            ),
            vec![
                ("Brother".into(), "idle".into()),
                ("Hp".into(), "printing".into()),
                ("Old".into(), "stopped".into())
            ]
        );
        assert_eq!(
            parse_options(
                "PageSize/Media Size: *A4 Letter\n\
                 Duplex/2-Sided Printing: *None DuplexNoTumble DuplexTumble\n\
                 ColorModel/Color Mode: Gray *RGB\n"
            ),
            (true, true)
        );
        assert_eq!(
            parse_options("Duplex/2-Sided: *None\nColorModel/Color: *Gray\n"),
            (false, false)
        );
        assert_eq!(
            parse_job("request id is Brother-12 (1 file(s))\n").as_deref(),
            Some("Brother-12")
        );
    }

    #[test]
    fn a_command_that_prints_a_lot_ends_with_all_of_it() {
        let printed = run("sh", &["-c", "head -c 300000 /dev/zero | tr '\\0' x"]).unwrap();
        assert_eq!(printed.len(), 300_000);
    }

    #[test]
    fn a_job_name_cannot_become_an_option() {
        let args = lp_args(&Job {
            queue: "Office".into(),
            file: std::path::PathBuf::from("/state/printing/x.pdf"),
            title: "-o evil".into(),
            copies: 2,
            duplex: Some(true),
            pages: Some("1-3".into()),
        });
        assert_eq!(
            args,
            [
                "-d",
                "Office",
                "-n",
                "2",
                "-t",
                "-o evil",
                "-o",
                "sides=two-sided-long-edge",
                "-P",
                "1-3",
                "--",
                "/state/printing/x.pdf"
            ]
        );
    }
}
