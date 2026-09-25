//! Section nesting depth bound (#317). The HWPX section parser recurses once
//! per nesting level, and a stack overflow aborts instead of unwinding, so a
//! document nested past the bound must be refused with an error.

#[path = "common/nested_tables.rs"]
mod nested_tables;

use std::path::PathBuf;
use std::process::Command;

use nested_tables::{TABLE_MARKDOWN, write_nested_tables};

fn tmp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hwp-nesting-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `hwp new` output holding one 2x2 table, the seed for nesting.
fn flat_table(dir: &std::path::Path) -> PathBuf {
    let markdown = dir.join("table.md");
    std::fs::write(&markdown, TABLE_MARKDOWN).unwrap();
    let flat = dir.join("flat.hwpx");
    let created = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(["new", "--from"])
        .arg(&markdown)
        .arg("-o")
        .arg(&flat)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "hwp new: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    flat
}

/// 42 nested tables are element depth 256, the bound; 43 are 262. This runs on
/// the test harness's default 2 MiB thread, in debug under CI: the bound must
/// stay below where that stack overflows (measured near 100 tables).
#[test]
fn nested_tables_at_the_bound_read_and_one_past_it_is_refused() {
    let dir = tmp_dir("bound");
    let flat = flat_table(&dir);

    let at_bound = dir.join("at-bound.hwpx");
    write_nested_tables(&flat, 42, &at_bound);
    hwpx::read::read_document(&at_bound).expect("깊이 상한의 문서를 읽지 못했습니다");

    let past = dir.join("past.hwpx");
    write_nested_tables(&flat, 43, &past);
    let error = hwpx::read::read_document(&past)
        .map(|_| ())
        .expect_err("깊이 상한을 넘은 문서가 읽혔습니다");
    assert!(
        matches!(error, hwpx::HwpxError::PackageLimit(ref message) if message.contains("256")),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A deep file makes `hwp cat` exit with an error, not a stack-overflow abort:
/// 2000 nested tables overflowed even the CLI's 32 MiB thread in debug.
#[test]
fn hwp_cat_refuses_a_deep_file_instead_of_aborting() {
    let dir = tmp_dir("cat");
    let flat = flat_table(&dir);
    let deep = dir.join("deep.hwpx");
    write_nested_tables(&flat, 2000, &deep);

    let output = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("cat")
        .arg(&deep)
        .output()
        .unwrap();
    // A signal (SIGABRT from a stack overflow) leaves no exit code.
    let code = output.status.code();
    assert!(
        code.is_some_and(|code| code != 0),
        "hwp cat 가 오류 코드로 끝나지 않았습니다: {:?}",
        output.status
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("256"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
