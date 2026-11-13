//! The POSIX installer is exercised with local archives and a fully isolated fake home.

#[cfg(unix)]
#[test]
fn installer_contract() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/tests/install.sh");
    let output = std::process::Command::new("bash")
        .arg(script)
        .output()
        .expect("run the isolated installer fixture");
    assert!(
        output.status.success(),
        "installer contract failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
