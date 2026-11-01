//! Prints a PDF through `cww::printing`, without a daemon or Chat with Work,
//! to try a system's spooler: `print_file <queue> <file.pdf> [pages]`. With
//! no arguments, lists the system's printers.

use cww::config::{Config, Printer};
use cww::paths::Paths;
use cww::printing::{Request, print, system_printers};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [queue, file, ..] = args.as_slice() else {
        for printer in system_printers().map_err(|e| anyhow::anyhow!(e.message))? {
            println!("{printer:?}");
        }
        return Ok(());
    };
    let home = tempfile::tempdir()?;
    let paths = Paths::under(home.path());
    let dir = paths.state_dir.join("printing");
    std::fs::create_dir_all(&dir)?;
    let transfer = "00112233445566778899aabbccddeeff".to_string();
    std::fs::copy(file, dir.join(format!("{transfer}.pdf")))?;
    let config = Config {
        printers: vec![Printer {
            id: "try".into(),
            queue: queue.clone(),
            label: "Try".into(),
            chats: true,
            bots: Vec::new(),
        }],
        ..Config::default()
    };
    let receipt = print(
        &paths,
        &config,
        &Request {
            printer: "try".into(),
            transfer,
            title: "cww print_file".into(),
            copies: 1,
            duplex: None,
            pages: args.get(2).cloned(),
            pages_counted: 1,
            bot: None,
            unattended: false,
        },
    )
    .map_err(|e| anyhow::anyhow!("{}: {}", e.code, e.message))?;
    println!("{receipt:?}");
    Ok(())
}
