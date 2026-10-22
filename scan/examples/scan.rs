//! Scans with this computer's scanner into a searchable PDF, the way the
//! scan window does, without a window: `cargo run -p cww-scan --example
//! scan -- <out.pdf> [language] [dpi]`. Grey, both sides, blank pages left
//! out. For trying a real scanner over SSH.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cww_scan::{ColorMode, Request};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("usage: scan <out.pdf> [language] [dpi]"))?;
    let language = args.get(1).cloned();
    let resolution: u32 = args.get(2).map(|d| d.parse()).transpose()?.unwrap_or(300);

    let started = std::time::Instant::now();
    let mut backend = cww_scan::system_backend().map_err(anyhow::Error::msg)?;
    let device = backend
        .devices()
        .map_err(anyhow::Error::msg)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no scanner found"))?;
    let caps = backend
        .capabilities(&device.id)
        .map_err(anyhow::Error::msg)?;
    eprintln!(
        "{} ({}), found in {:?}: {caps:?}",
        device.name,
        device.id,
        started.elapsed()
    );

    let request = Request {
        device: device.id.clone(),
        color: false,
        resolution: caps.nearest_resolution(resolution),
        duplex: caps.duplex,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let mut pages = Vec::new();
    let end = backend
        .scan(&request, &cancel, &mut |page| {
            let blank = cww_scan::blank::is_blank(&page.image, page.resolution);
            eprintln!(
                "page {}: {}x{} at {} dpi{}",
                pages.len() + 1,
                page.image.width(),
                page.image.height(),
                page.resolution,
                if blank { ", blank, left out" } else { "" }
            );
            if !blank {
                pages.push(page);
            }
            Ok(())
        })
        .map_err(anyhow::Error::msg)?;
    eprintln!("scan ended: {end:?}");

    let engine = match &language {
        Some(_) => Some(cww_scan::ocr::system_engine().map_err(anyhow::Error::msg)?),
        None => None,
    };
    let mut pdf = cww_scan::pdf::Builder::new();
    for page in &pages {
        let words = match (&engine, &language) {
            (Some(engine), Some(language)) => {
                let read = std::time::Instant::now();
                let words = engine
                    .recognize(&page.image, page.resolution, language)
                    .map_err(anyhow::Error::msg)?;
                eprintln!("read {} words in {:?}", words.len(), read.elapsed());
                words
            }
            _ => vec![],
        };
        pdf.page(&page.image, page.resolution, ColorMode::Grayscale, &words)?;
    }
    std::fs::write(out, pdf.finish())?;
    eprintln!("{} page(s) into {out}", pages.len());
    Ok(())
}
