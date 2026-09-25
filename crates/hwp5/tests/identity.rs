//! identity-reserialize 게이트 (L0).
//!
//! 전체 fixture의 레코드 스트림을 strict 스캔 → 트리 → 재직렬화했을 때
//! **압축 해제 스트림 기준 바이트 동일**해야 한다. 이 게이트가 통과해야
//! "우리 레코드 계층이 빠뜨리는 것이 없다"는 1차 증명이 된다 (M6 전제).

use std::path::PathBuf;

use hwp5::record::{RecordNode, ScanMode, scan_stream};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/hwp5")
        .join(name)
}

const ALL: &[&str] = &[
    "hello_world.hwp",
    "bookmark.hwp",
    "color_fill.hwp",
    "outline.hwp",
    "work_report.hwp",
    "annual_report.hwp",
];

#[path = "../../hwp-cli/tests/common/fixture_skip.rs"]
mod fixture_skip;

/// Fixture binaries are not in the repository (local only). `true` means skip, and the skip is
/// counted (#275).
#[track_caller]
fn skip_if_no_fixtures() -> bool {
    fixture_skip::fixture_missing(&fixture("hello_world.hwp"))
}

/// The #275 accounting probe: guards the path `scripts/tests/fixture-skip-accounting.sh` names in
/// `HWP_FIXTURE_SKIP_PROBE`, exactly as a fixture guard would, so the script can drive both the
/// absent and the present case on any checkout. The guard is the whole test.
#[test]
#[ignore = "driven by scripts/tests/fixture-skip-accounting.sh"]
fn fixture_skip_accounting_probe() {
    let probe = std::env::var_os("HWP_FIXTURE_SKIP_PROBE").expect("HWP_FIXTURE_SKIP_PROBE");
    fixture_skip::fixture_missing(std::path::Path::new(&probe));
}

#[test]
fn 레코드_스트림_바이트_동일_재직렬화() {
    if skip_if_no_fixtures() {
        return;
    }
    for name in ALL {
        let mut c = hwp5::Hwp5Container::open(&fixture(name)).expect(name);
        let mut targets = vec!["/DocInfo".to_string()];
        targets.extend(c.body_sections());

        for stream in targets {
            let original = c.read_record_stream(&stream).expect(&stream);
            let scan = scan_stream(&original, ScanMode::Strict)
                .unwrap_or_else(|e| panic!("{name} {stream}: strict 스캔 실패 — {e}"));
            let reserialized = RecordNode::serialize_forest(&scan.roots);
            assert_eq!(
                reserialized,
                original,
                "{name} {stream}: 재직렬화 바이트 불일치 (원본 {}B vs 재직렬화 {}B)",
                original.len(),
                reserialized.len()
            );
        }
    }
}
