//! Shared accounting for tests missing local-only fixtures or required resources (#275).
//!
//! `fixtures/hwp5/`, `fixtures/hwpx/` and `fixtures/pdf-parity/private/` are gitignored by the
//! data policy, so a test that needs one of them returns early without it, and libtest reports
//! that early return as `ok`. Every such guard routes through [`fixture_missing`] (or
//! [`optional_fixture_missing`]) so the reduction can be seen and, when wanted, refused:
//!
//! - `HWP_FIXTURE_SKIP_LOG=<absolute path>`: each skip appends one line (test name, call site,
//!   `required` or `optional`, missing path). `scripts/check.sh` sets it, clears the file first,
//!   and prints the counts as `skipped-for-missing-fixtures=N (optional=M)` on its summary line.
//! - [`resource_unavailable`] records a missing required host resource in the same log; its
//!   label occupies the missing-path column and its diagnostic names the remedy. These skips
//!   contribute to the existing total, with `required` in the third column.
//! - `HWP_REQUIRE_FIXTURES=1`: a missing required fixture or resource fails instead of skipping.
//!   Optional ones (the ground-truth sets `fixtures/README.md` lists as not currently held) are
//!   still skipped and counted.
//!
//! With neither variable set the behaviour is the old one: a stderr note and an early return.
//! Test binaries are separate processes, so the count lives in a file, not in memory. This file
//! is included with `#[path]` by every test tree that has such a guard (no shared crate).

use std::io::Write as _;
use std::panic::Location;
use std::path::Path;

/// `true` when `path` does not exist, after recording the skip; the caller then returns early.
/// Fails instead under `HWP_REQUIRE_FIXTURES=1`.
#[allow(dead_code)] // resource-only test trees also include this module
#[track_caller]
pub fn fixture_missing(path: &Path) -> bool {
    skip_if_missing(path, true, Location::caller())
}

/// [`fixture_missing`] for a ground-truth set `fixtures/README.md` lists as not currently held:
/// the skip is counted (tagged `optional`), but `HWP_REQUIRE_FIXTURES=1` does not fail it.
#[allow(dead_code)] // not every test tree that includes this file has an optional fixture
#[track_caller]
pub fn optional_fixture_missing(path: &Path) -> bool {
    skip_if_missing(path, false, Location::caller())
}

/// Record an unavailable required test resource after the caller probes for it.
/// Returns `true` so the caller can use the same early-return guard as fixture checks.
/// Strict mode fails with the resource label and an actionable remedy.
#[allow(dead_code)] // fixture-only test trees also include this module
#[track_caller]
pub fn resource_unavailable(label: &str, remedy: &str) -> bool {
    let site = Location::caller();
    if std::env::var_os("HWP_REQUIRE_FIXTURES").is_some_and(|v| v == "1") {
        panic!(
            "HWP_REQUIRE_FIXTURES=1: required test resource unavailable ({label}) at {site}; remedy: {remedy}"
        );
    }
    eprintln!("SKIP: required test resource unavailable ({label}); remedy: {remedy}");
    record_skip(label, true, site);
    true
}

fn skip_if_missing(path: &Path, required: bool, site: &Location<'_>) -> bool {
    if path.exists() {
        return false;
    }
    if required && std::env::var_os("HWP_REQUIRE_FIXTURES").is_some_and(|v| v == "1") {
        panic!(
            "HWP_REQUIRE_FIXTURES=1: fixture 없음 ({}) at {site} — fixtures/README.md 참고",
            path.display()
        );
    }
    eprintln!(
        "스킵: fixture 없음 ({}) — fixtures/README.md 참고",
        path.display()
    );
    record_skip(&path.display().to_string(), required, site);
    true
}

fn record_skip(label: &str, required: bool, site: &Location<'_>) {
    if let Some(log) = std::env::var_os("HWP_FIXTURE_SKIP_LOG") {
        let test = std::thread::current().name().unwrap_or("?").to_owned();
        let kind = if required { "required" } else { "optional" };
        // One write of one whole line on an append-mode handle, so parallel test threads and
        // processes add lines without interleaving them.
        let line = format!("{test}\t{site}\t{kind}\t{label}\n");
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .and_then(|mut file| file.write_all(line.as_bytes()))
            .unwrap_or_else(|e| {
                // An unrecorded skip is exactly the silent reduction this exists to prevent.
                panic!(
                    "HWP_FIXTURE_SKIP_LOG ({}) 기록 실패: {e}",
                    Path::new(&log).display()
                )
            });
    }
}
