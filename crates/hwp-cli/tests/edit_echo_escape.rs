//! `hwp edit` progress echoes must not let a user value forge stderr lines (#530).
//!
//! `--set-meta` echoed its value with Display, so a value holding `\nError: forged` printed a
//! fake `Error: ` line ahead of a later operation's real refusal. Every echo now uses Debug.

use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

#[test]
fn set_meta_value_cannot_forge_an_error_line() {
    let dir = std::env::temp_dir().join(format!("hwp-cli-echo-escape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("in.md");
    std::fs::write(&md, "# T\n\nhello\n").unwrap();
    let src = dir.join("in.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .status()
            .unwrap()
            .success()
    );

    let out = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(dir.join("out.hwpx"))
        .args(["--set-meta", "title=x\nError: forged", "--add-row", "99"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "the --add-row 99 refusal must fail the run"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first_error = stderr
        .lines()
        .find(|line| line.starts_with("Error: "))
        .unwrap_or_else(|| panic!("no Error: line in stderr:\n{stderr}"));
    assert!(
        !first_error.contains("forged") && first_error.contains("99"),
        "first Error: line must be the real refusal, got {first_error:?}\nstderr:\n{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
