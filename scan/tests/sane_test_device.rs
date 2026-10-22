//! End to end against SANE's own `test` backend, a fake scanner with a
//! feeder that empties after ten pages. Skipped where libsane isn't
//! installed; CI on Linux installs it (sane-backends' `test` backend must
//! be enabled in dll.conf, or SANE_CONFIG_DIR point at a dll.conf naming it).

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cww_scan::{Backend, End, Request};

/// CI sets CWW_SCAN_REQUIRE_SANE=1, so the test fails there rather than
/// skipping.
fn skip(why: &str) {
    assert!(std::env::var_os("CWW_SCAN_REQUIRE_SANE").is_none(), "{why}");
    eprintln!("skipped: {why}");
}

fn sane() -> Option<cww_scan::sane::Sane> {
    // SAFETY: tests in this file run one at a time (one test).
    unsafe { std::env::set_var("CWW_SCAN_TEST_DEVICES", "1") };
    match cww_scan::sane::Sane::load() {
        Ok(sane) => Some(sane),
        Err(e) => {
            skip(&e.to_string());
            None
        }
    }
}

#[test]
fn scans_the_test_devices_feeder_to_the_end() {
    let Some(mut sane) = sane() else { return };
    let devices = sane.devices().unwrap();
    let Some(device) = devices.iter().find(|d| d.id.starts_with("test:")) else {
        skip("the test backend isn't enabled in dll.conf");
        return;
    };
    let caps = sane.capabilities(&device.id).unwrap();
    assert!(caps.feeder && caps.flatbed && caps.color);
    assert!(caps.resolutions.contains(&300));

    let cancel = Arc::new(AtomicBool::new(false));
    let request = Request {
        device: device.id.clone(),
        color: false,
        resolution: 50,
        duplex: true,
    };
    let mut pages = Vec::new();
    let end = sane
        .scan(&request, &cancel, &mut |page| {
            pages.push(page);
            Ok(())
        })
        .unwrap();
    assert_eq!(end, End::Done);
    assert_eq!(pages.len(), 10);
    // 80 x 100 mm at 50 dpi... the whole area the device offers.
    assert!(pages[0].image.width() > 100 && pages[0].image.height() > 100);
    assert_eq!(pages[0].resolution, 50);
    assert!(pages[0].image.as_luma8().is_some());

    let request = Request {
        color: true,
        ..request
    };
    let mut colour = None;
    let end = sane
        .scan(&request, &cancel, &mut |page| {
            colour.get_or_insert(page);
            Ok(())
        })
        .unwrap();
    assert_eq!(end, End::Done);
    assert!(colour.unwrap().image.as_rgb8().is_some());
}
