//! Prints for real through CUPS's own commands: a private `cupsd` of this
//! test's, on a port of its own, whose one printer is a backend of this test's
//! that keeps the job in a file, so nothing reaches a real printer whatever
//! this computer has. Without
//! `cupsd`, the test says so and passes, unless `CWW_PRINT_REQUIRE_CUPS` is
//! set, as CI's Linux job does.
#![cfg(unix)]

use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cww::config::{Config, Printer};
use cww::paths::Paths;
use cww::printing::{Request, print, system_printers};

struct Cupsd(Child);

impl Drop for Cupsd {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn group() -> String {
    let output = Command::new("id").arg("-gn").output().unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn user() -> String {
    let output = Command::new("id").arg("-un").output().unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn start_cupsd(root: &Path, port: u16) -> Option<Cupsd> {
    for dir in ["etc/ppd", "etc/ssl", "spool/tmp", "cache", "run", "log"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    // CUPS's own programs, and a backend that keeps what it's given.
    std::fs::create_dir_all(root.join("bin/backend")).unwrap();
    for entry in std::fs::read_dir("/usr/lib/cups").ok()?.flatten() {
        if entry.file_name() != "backend" {
            std::os::unix::fs::symlink(entry.path(), root.join("bin").join(entry.file_name()))
                .unwrap();
        }
    }
    let backend = root.join("bin/backend/keep");
    std::fs::write(
        &backend,
        "#!/bin/sh\n[ $# -eq 0 ] && { echo 'direct keep \\\"Unknown\\\" \\\"Keep\\\"'; exit 0; }\n\
         out=\"${DEVICE_URI#keep:}\"\ncat \"${6:--}\" > \"$out.tmp\"\n\
         echo \"$4 $5\" > \"$out.args\"\nmv \"$out.tmp\" \"$out\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&backend, PermissionsExt::from_mode(0o755)).unwrap();
    let r = root.display();
    let (user, group) = (user(), group());
    std::fs::write(
        root.join("etc/cups-files.conf"),
        format!(
            "User {user}\nGroup {group}\nSystemGroup {group}\nSandboxing Off\n\
             ServerBin {r}/bin\nServerRoot {r}/etc\nRequestRoot {r}/spool\nTempDir {r}/spool/tmp\n\
             CacheDir {r}/cache\nStateDir {r}/run\nAccessLog {r}/log/access\n\
             ErrorLog {r}/log/error\nPageLog {r}/log/page\n"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("etc/cupsd.conf"),
        format!(
            "Listen 127.0.0.1:{port}\nWebInterface No\nBrowsing Off\nDefaultAuthType None\n\
             <Location />\nOrder allow,deny\nAllow all\n</Location>\n\
             <Policy default>\n<Limit All>\nOrder allow,deny\nAllow all\n</Limit>\n</Policy>\n"
        ),
    )
    .unwrap();
    let cupsd = ["/usr/sbin/cupsd", "/usr/bin/cupsd"]
        .into_iter()
        .find(|path| {
            std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o001 != 0)
        })?;
    let child = Command::new(cupsd)
        .args(["-f", "-c"])
        .arg(root.join("etc/cupsd.conf"))
        .arg("-s")
        .arg(root.join("etc/cups-files.conf"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    Some(Cupsd(child))
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A small PDF, with a page.
fn pdf() -> Vec<u8> {
    use lopdf::{Document, Object, Stream, dictionary};
    let mut doc = Document::with_version("1.5");
    let pages = doc.new_object_id();
    let content = doc.add_object(Stream::new(dictionary! {}, b"0 0 m".to_vec()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages,
        "Contents" => content,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    doc.objects.insert(
        pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page.into()],
            "Count" => 1,
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();
    bytes
}

#[test]
fn a_job_reaches_a_cups_queue_and_only_once() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("cups");
    let port = free_port();
    let Some(_cupsd) = start_cupsd(&root, port) else {
        assert!(
            std::env::var_os("CWW_PRINT_REQUIRE_CUPS").is_none(),
            "cupsd isn't installed"
        );
        eprintln!("skipped: no cupsd here");
        return;
    };
    let server = format!("127.0.0.1:{port}");
    // SAFETY: this test binary runs this one test, and before any thread of
    // its own: every CUPS command it starts must reach the private server.
    unsafe {
        std::env::set_var("CUPS_SERVER", &server);
        std::env::remove_var("PRINTER");
    }
    let out = home.path().join("printed.pdf");
    let started = Instant::now();
    loop {
        let added = Command::new("lpadmin")
            .args(["-h", &server, "-p", "CWW_Test", "-E", "-m", "raw", "-v"])
            .arg(format!("keep:{}", out.display()))
            .stderr(Stdio::null())
            .status()
            .unwrap();
        if added.success() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(15), "no queue");
        std::thread::sleep(Duration::from_millis(100));
    }

    let printers = system_printers().unwrap();
    assert_eq!(printers.len(), 1, "{printers:?}");
    assert_eq!(printers[0].queue, "CWW_Test");
    assert_eq!(printers[0].state, "idle");

    let paths = Paths::under(home.path());
    let dir = paths.state_dir.join("printing");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "00112233445566778899aabbccddeeff";
    let document = pdf();
    std::fs::write(dir.join(format!("{id}.pdf")), &document).unwrap();
    let config = Config {
        printers: vec![Printer {
            id: "test".into(),
            queue: "CWW_Test".into(),
            label: "Test".into(),
            chats: true,
            bots: Vec::new(),
        }],
        ..Config::default()
    };
    let request = Request {
        printer: "test".into(),
        transfer: id.into(),
        title: "Guest list".into(),
        copies: 2,
        duplex: Some(true),
        pages: None,
        pages_counted: 1,
        bot: None,
        unattended: false,
    };
    let receipt = print(&paths, &config, &request).unwrap();
    assert!(receipt.job.starts_with("CWW_Test-"), "{}", receipt.job);
    assert!(!receipt.already);
    assert!(!dir.join(format!("{id}.pdf")).exists(), "the file is gone");

    let started = Instant::now();
    while !out.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "nothing printed: {}",
            std::fs::read_to_string(root.join("log/error")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(std::fs::read(&out).unwrap(), document);
    let args = std::fs::read_to_string(home.path().join("printed.pdf.args")).unwrap();
    assert!(args.starts_with("2 "), "{args}");
    assert!(args.contains("sides=two-sided-long-edge"), "{args}");

    // The call made again (tools run at least once) prints nothing.
    let again = print(&paths, &config, &request).unwrap();
    assert!(again.already);
    assert_eq!(again.job, receipt.job);
}
