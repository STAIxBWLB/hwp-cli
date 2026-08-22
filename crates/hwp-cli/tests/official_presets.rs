use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "hwp-cli-official-presets-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn nested_html(depth: usize) -> String {
    let mut html = String::new();
    for level in 1..=depth {
        html.push_str(&format!("<ol><li>level {level}"));
    }
    for _ in 0..depth {
        html.push_str("</li></ol>");
    }
    html
}

#[test]
fn embedded_html_depth9_publishes_nothing() {
    let root = temp_dir("embedded-html-depth9");
    let input = root.join("input.md");
    let output = root.join("rejected.hwpx");
    fs::write(&input, nested_html(9)).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(["new", "--from"])
        .arg(&input)
        .args(["--output"])
        .arg(&output)
        .args(["--preset", "gian"])
        .status()
        .unwrap();

    assert!(!status.success(), "depth 9 must fail closed");
    assert!(
        !output.exists(),
        "the failed import must not publish an output document"
    );

    fs::remove_dir_all(root).unwrap();
}
