//! A short, unique test directory name (#412 item 6).
//!
//! Windows refuses to create a directory whose full path is 248 characters or more
//! (`os error 206`), and `hwp fill` and `hwp convert` nest two private staging workspaces of
//! about 70 characters each inside their output directory. The failing run on 2026-09-27 had a
//! 248 character path: a 37 character temp dir, a 38 character test directory name and a
//! sub-directory, plus those staging names. The product now canonicalizes its nested workspaces
//! (#396), so this is the test-side half: keep the part the test controls short, and keep it
//! unique so two runs at once never share a directory.
//!
//! The name is `<prefix><pid>-<label cut to 12 characters>-<8 hex of the full label's hash>`.
//! The cut label keeps a failing directory recognisable; the hash keeps two labels that share a
//! 12 character start apart. Included with `#[path]` by the test trees that nest staging
//! workspaces (no shared crate).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::path::PathBuf;

/// The most characters `short_dir` adds to the temp dir path, separator included.
#[allow(dead_code)] // the including tree asserts it; not every tree that includes this file does
pub const SHORT_DIR_NAME_BUDGET: usize = 1 + 4 + 10 + 1 + 12 + 1 + 8;

/// `<temp dir>/<prefix><pid>-<label…>-<hash>`, not yet created. `prefix` is at most 4 characters.
#[allow(dead_code)] // not every tree that includes this file nests staging workspaces
pub fn short_dir(prefix: &str, label: &str) -> PathBuf {
    assert!(
        prefix.len() <= 4,
        "prefix {prefix:?} is longer than 4 characters"
    );
    let mut hasher = DefaultHasher::new();
    label.hash(&mut hasher);
    let hash = hasher.finish() as u32;
    let cut: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect();
    std::env::temp_dir().join(format!("{prefix}{}-{cut}-{hash:08x}", std::process::id()))
}
