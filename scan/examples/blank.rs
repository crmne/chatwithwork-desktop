//! Says which page images look blank: `cargo run -p cww-scan --example
//! blank -- <dpi> <page.png>...`, for tuning the detector on real scans.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dpi, pages @ ..] = args.as_slice() else {
        anyhow::bail!("usage: blank <dpi> <page.png>...");
    };
    let dpi: u32 = dpi.parse()?;
    for path in pages {
        let image = image::open(path)?;
        let cells = cww_scan::blank::inked_cells(&image, dpi);
        println!(
            "{path}: {cells} inked cells, {}",
            if cww_scan::blank::is_blank(&image, dpi) {
                "blank"
            } else {
                "kept"
            }
        );
    }
    Ok(())
}
