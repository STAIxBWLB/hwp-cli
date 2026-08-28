//! `hwp merge` / `hwp split` cross-command integration tests (03-03).
//!
//! This is the automated proxy `03-VALIDATION.md` names for the Hancom verdict on
//! FLOW-02: a merge-then-split round-trip that needs no Hancom and no corpus, plus
//! the D-16 all-or-nothing publication guarantee.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

/// A fresh scratch directory per call — a distinct directory per test (and per
/// call within a test), so concurrent `cargo test` runs never collide.
fn scratch_dir(label: &str) -> PathBuf {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "hwp-cli-document-workflows-{label}-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds a genuine `.hwp` file from markdown through the public CLI import
/// path (`hwp new --from`) — no direct `hwp_convert`/writer calls, so this
/// exercises the same input-construction path a real user would use.
fn write_input_hwp(dir: &Path, stem: &str, markdown: &str) -> PathBuf {
    let md = dir.join(format!("{stem}.md"));
    std::fs::write(&md, markdown).unwrap();
    let hwp_path = dir.join(format!("{stem}.hwp"));
    let status = hwp()
        .args(["new", "--from"])
        .arg(&md)
        .arg("-o")
        .arg(&hwp_path)
        .status()
        .unwrap();
    assert!(status.success(), "hwp new --from {stem}.md 실패");
    hwp_path
}

/// Plain-text content of an HWP/HWPX file via `hwp cat --format plain`.
fn cat_plain(path: &Path) -> String {
    let output = hwp()
        .args(["cat", "--format", "plain"])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "hwp cat {} 실패", path.display());
    String::from_utf8(output.stdout).unwrap()
}

fn fragment_paths(dir: &Path, stem: &str, ext: &str) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{stem}-")) && name.ends_with(&format!(".{ext}"))
                })
        })
        .collect();
    entries.sort();
    entries
}

#[test]
fn merge_then_split_reproduces_each_input_section() {
    let dir = scratch_dir("roundtrip");
    let a = write_input_hwp(&dir, "a", "문서 A의 본문입니다\n");
    let b = write_input_hwp(&dir, "b", "문서 B의 본문입니다\n");
    let merged = dir.join("merged.hwp");

    let status = hwp()
        .arg("merge")
        .arg(&a)
        .arg(&b)
        .arg("-o")
        .arg(&merged)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");

    let out_dir = dir.join("frag");
    let status = hwp()
        .arg("split")
        .arg(&merged)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success(), "hwp split 실패");

    let fragments = fragment_paths(&out_dir, "merged", "hwp");
    assert_eq!(
        fragments.len(),
        2,
        "조각 두 개가 나와야 합니다: {fragments:?}"
    );

    let fragment_one = cat_plain(&fragments[0]);
    let fragment_two = cat_plain(&fragments[1]);
    let input_one = cat_plain(&a);
    let input_two = cat_plain(&b);

    assert!(
        fragment_one.contains(input_one.trim()),
        "조각 1에 입력 1의 본문이 있어야 합니다: {fragment_one:?}"
    );
    assert!(
        fragment_two.contains(input_two.trim()),
        "조각 2에 입력 2의 본문이 있어야 합니다: {fragment_two:?}"
    );
    assert!(!fragment_one.contains(input_two.trim()));
    assert!(!fragment_two.contains(input_one.trim()));

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn split_publishes_all_fragments_or_none() {
    let dir = scratch_dir("atomicity");
    let a = write_input_hwp(&dir, "a", "문서 A\n");
    let b = write_input_hwp(&dir, "b", "문서 B\n");
    let c = write_input_hwp(&dir, "c", "문서 C\n");
    let input = dir.join("in.hwp");

    let status = hwp()
        .arg("merge")
        .arg(&a)
        .arg(&b)
        .arg(&c)
        .arg("-o")
        .arg(&input)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");

    let out_dir = dir.join("frag");
    std::fs::create_dir_all(&out_dir).unwrap();
    // Force a failure the transaction cannot recover from: the destination
    // for the third fragment is a directory, not a regular file — the
    // publish precheck refuses to replace it (output.rs's inspect_destination).
    std::fs::create_dir_all(out_dir.join("in-003.hwp")).unwrap();

    let status = hwp()
        .arg("split")
        .arg(&input)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(!status.success(), "강제 실패인데 성공했습니다");

    // D-16: the fragment set is never partially published — none of the
    // three fragment file names exist after the forced failure.
    assert!(!out_dir.join("in-001.hwp").exists());
    assert!(!out_dir.join("in-002.hwp").exists());
    assert!(
        out_dir.join("in-003.hwp").is_dir(),
        "사전에 만든 디렉터리가 그대로 남아 있어야 합니다"
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn 단일_구역_입력은_조각_하나만_낸다() {
    let dir = scratch_dir("single-section");
    let input = write_input_hwp(&dir, "solo", "단일 구역 본문\n");
    let out_dir = dir.join("frag");

    let status = hwp()
        .arg("split")
        .arg(&input)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success());

    assert!(out_dir.join("solo-001.hwp").exists());
    assert!(!out_dir.join("solo-002.hwp").exists());

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn 동일한_두_구역은_서로_다른_조각_파일_두_개가_된다() {
    let dir = scratch_dir("adjacency");
    let a = write_input_hwp(&dir, "same", "같은 내용\n");
    let merged = dir.join("merged.hwp");

    // FLOW-02 adjacency probe: merge the same input with itself so the
    // merged document carries two byte-equal Sections (D-02: sections never
    // fuse), then confirm split never collapses them either.
    let status = hwp()
        .arg("merge")
        .arg(&a)
        .arg(&a)
        .arg("-o")
        .arg(&merged)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");

    let out_dir = dir.join("frag");
    let status = hwp()
        .arg("split")
        .arg(&merged)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success(), "hwp split 실패");

    let fragments = fragment_paths(&out_dir, "merged", "hwp");
    assert_eq!(fragments.len(), 2);
    assert_ne!(fragments[0], fragments[1]);
    assert_eq!(cat_plain(&fragments[0]), cat_plain(&fragments[1]));

    std::fs::remove_dir_all(dir).unwrap();
}

/// FLOW-02 `empty` probe (A-04): a document with zero sections is refused
/// rather than published as an empty set.
///
/// No public CLI path can author a genuine zero-section `.hwp`/`.hwpx` file
/// (`hwp new` always emits at least one section, and hand-authoring one
/// through the low-level container writers is undefined territory this
/// crate's own writers never exercise) — so this asserts the exact contract
/// `hwp split` calls into directly: `split_sections` on a document with no
/// sections at all, the same shape `commands::split` would load from any
/// input.
#[test]
fn 구역이_없는_문서는_거부되고_파일이_생기지_않는다() {
    let document = hwp_model::Document::default();
    assert!(document.sections.is_empty());
    let result = hwp_convert::document_split::split_sections(&document);
    assert!(
        result.is_err(),
        "구역이 없는 문서는 분할이 거부되어야 합니다"
    );
}
