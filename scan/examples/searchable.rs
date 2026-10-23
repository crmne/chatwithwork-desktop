//! Makes a searchable PDF from page images on this computer, the way the
//! scan page does: `cargo run -p cww-scan --example searchable -- <dpi>
//! <language> <out.pdf> <page.png>...`. For trying the OCR and the text
//! layer without a scanner.

use cww_scan::ColorMode;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dpi, language, out, pages @ ..] = args.as_slice() else {
        anyhow::bail!("usage: searchable <dpi> <language> <out.pdf> <page.png>...");
    };
    let dpi: u32 = dpi.parse()?;
    let engine = cww_scan::ocr::system_engine().map_err(anyhow::Error::msg)?;
    let mut pdf = cww_scan::pdf::Builder::new();
    for path in pages {
        let image = image::open(path)?;
        let started = std::time::Instant::now();
        let words = engine
            .recognize(&image, dpi, language)
            .map_err(anyhow::Error::msg)?;
        eprintln!("{path}: {} words in {:?}", words.len(), started.elapsed());
        pdf.page(&image, dpi, ColorMode::Grayscale, &words)?;
    }
    std::fs::write(out, pdf.finish())?;
    Ok(())
}
