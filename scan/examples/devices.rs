//! Lists the scanners this computer's backend sees, and how long it took:
//! `cargo run -p cww-scan --example devices`.

fn main() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let mut backend = cww_scan::system_backend().map_err(anyhow::Error::msg)?;
    let devices = backend.devices().map_err(anyhow::Error::msg)?;
    println!("{} found in {:?}", devices.len(), started.elapsed());
    for device in devices {
        let caps = backend
            .capabilities(&device.id)
            .map_err(anyhow::Error::msg)?;
        println!("{} ({}, {}): {caps:?}", device.name, device.id, device.kind);
    }
    Ok(())
}
