//! Executable fixtures prepared without writable descriptors in the test runner.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub fn write_script(path: &Path, body: &str) {
    // Nix supplies its shell through PATH, not /bin/sh.
    let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("sh"))
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
        .expect("test PATH contains sh");
    let script = format!("#!{}\n{body}\n", shell.display());
    // A concurrent fork can inherit an open writer until its exec closes it,
    // making immediate execution fail with ETXTBSY even after fs::write returns.
    // Write in a separate process and wait for it, so the parallel test runner
    // never owns a writable descriptor to an executable fixture.
    let status = std::process::Command::new(shell)
        .args(["-c", "printf '%s' \"$2\" > \"$1\"", "test-script"])
        .arg(path)
        .arg(script)
        .status()
        .expect("write executable fixture");
    assert!(status.success(), "write executable fixture: {status}");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
