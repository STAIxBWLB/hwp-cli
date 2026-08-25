//! `hwp slots` and `hwp fill` must agree about which `{{slot}}` a document has.
//!
//! They read the document two different ways: `slots` walks the IR, where a paragraph's
//! characters are already joined, while `fill` rewrites the raw section XML. Inline formatting
//! inside a slot name splits it across text runs, and before #145 that split was invisible to
//! `slots` and fatal to `fill` — the name was listed and then refused.
//!
//! CI-safe: markdown source and temp dirs only, no fixtures, no fonts.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hwp-slot-fill-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Slot names `hwp slots` reports, one `name<TAB>count` line each.
fn reported_slots(path: &Path) -> BTreeSet<String> {
    let output = hwp().arg("slots").arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "hwp slots failed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split('\t').next().unwrap_or(line).to_string())
        .collect()
}

fn document_text(path: &Path) -> String {
    let output = hwp().arg("cat").arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "hwp cat failed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Every slot `hwp slots` lists must be fillable, including the ones inline formatting split
/// across runs. `hwp fill` is fail-closed, so a name it cannot find aborts the whole command —
/// which makes a single fill of all reported names the sharpest form of this assertion.
#[test]
fn every_reported_slot_is_fillable() {
    let dir = test_dir("agreement");
    let source = dir.join("source.md");
    std::fs::write(
        &source,
        // 제목: whole in one run. 이름/기관명: split by emphasis and by bold.
        // 연락처: split twice over. 비고: formatting around, not inside, the name.
        "제목  {{제목}}\n\n\
         이름: {{이*름*}}\n\n\
         기관: {{기**관**명}}\n\n\
         연락: {{연*락*처}}\n\n\
         비고: *{{비고}}*\n",
    )
    .unwrap();

    let created = dir.join("doc.hwpx");
    let run = hwp()
        .args(["new", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&created)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "hwp new failed\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let reported = reported_slots(&created);
    let expected: BTreeSet<String> = ["제목", "이름", "기관명", "연락처", "비고"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "hwp slots must report every slot, split across runs or not"
    );

    // One fill of all of them. Without --allow-partial this exits non-zero if any name is
    // unfillable, so success is the agreement assertion.
    let filled = dir.join("filled.hwpx");
    let mut fill = hwp();
    fill.arg("fill").arg(&created).arg("-o").arg(&filled);
    for name in &reported {
        fill.arg("--set").arg(format!("{name}=값-{name}"));
    }
    let run = fill.output().unwrap();
    assert!(
        run.status.success(),
        "every reported slot must be fillable, but fill refused\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    // Nothing left behind, and the values actually landed.
    assert!(
        reported_slots(&filled).is_empty(),
        "no slot may survive a fill of every reported name"
    );
    let text = document_text(&filled);
    for name in &reported {
        assert!(
            text.contains(&format!("값-{name}")),
            "value for {name} missing from the filled document:\n{text}"
        );
    }

    let run = hwp().arg("validate").arg(&filled).output().unwrap();
    assert!(
        run.status.success(),
        "the filled document must validate\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Text around a split placeholder must survive the coalescing untouched — the failure mode of
/// a run-range rewrite is eating a neighbouring character, which no slot-name check would catch.
#[test]
fn coalescing_a_split_slot_preserves_its_neighbouring_text() {
    let dir = test_dir("neighbours");
    let source = dir.join("source.md");
    std::fs::write(&source, "앞말 {{이*름*}} 뒷말\n").unwrap();

    let created = dir.join("doc.hwpx");
    hwp()
        .args(["new", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&created)
        .output()
        .unwrap();

    let filled = dir.join("filled.hwpx");
    let run = hwp()
        .arg("fill")
        .arg(&created)
        .arg("--set")
        .arg("이름=홍길동")
        .arg("-o")
        .arg(&filled)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "fill failed\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(document_text(&filled).trim(), "앞말 홍길동 뒷말");

    let _ = std::fs::remove_dir_all(&dir);
}
