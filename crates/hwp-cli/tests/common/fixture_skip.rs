//! Shared accounting for tests that skip because a local-only fixture is absent (#275).
//!
//! `fixtures/hwp5/`, `fixtures/hwpx/` and `fixtures/pdf-parity/private/` are gitignored by the
//! data policy, so a test that needs one of them returns early without it, and libtest reports
//! that early return as `ok`. Every such guard routes through [`fixture_missing`] so the
//! reduction can be seen and, when wanted, refused:
//!
//! - `HWP_FIXTURE_SKIP_LOG=<absolute path>`: each skip appends one line (test name, call site,
//!   missing path). `scripts/check.sh` sets it, clears the file first, and prints the line count
//!   as `skipped-for-missing-fixtures=N` on its summary line.
//! - `HWP_REQUIRE_FIXTURES=1`: a missing fixture fails the test instead of skipping it.
//!
//! With neither variable set the behaviour is the old one: a stderr note and an early return.
//! Test binaries are separate processes, so the count lives in a file, not in memory. This file
//! is included with `#[path]` by every test tree that has such a guard (no shared crate).

use std::io::Write as _;
use std::path::Path;

/// `true` when `path` does not exist, after recording the skip; the caller then returns early.
#[track_caller]
pub fn fixture_missing(path: &Path) -> bool {
    if path.exists() {
        return false;
    }
    let site = std::panic::Location::caller();
    if std::env::var_os("HWP_REQUIRE_FIXTURES").is_some_and(|v| v == "1") {
        panic!(
            "HWP_REQUIRE_FIXTURES=1: fixture 없음 ({}) at {site} — fixtures/README.md 참고",
            path.display()
        );
    }
    eprintln!(
        "스킵: fixture 없음 ({}) — fixtures/README.md 참고",
        path.display()
    );
    if let Some(log) = std::env::var_os("HWP_FIXTURE_SKIP_LOG") {
        let test = std::thread::current().name().unwrap_or("?").to_owned();
        // One write of one whole line on an append-mode handle, so parallel test threads and
        // processes add lines without interleaving them.
        let line = format!("{test}\t{site}\t{}\n", path.display());
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
    true
}
