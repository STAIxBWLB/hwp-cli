//! A test temp path that is removed when the test passes (#349).
//!
//! The name carries the process id, so two test runs at once do not collide. Dropping the value
//! removes the file or directory at the path; after a panic it is kept for debugging. Included
//! with `#[path]` by the test trees whose `tmp()` helper hands out many paths (no shared crate).

use std::path::{Path, PathBuf};

pub struct TempPath(PathBuf);

impl TempPath {
    /// `<temp dir>/<prefix>-<pid>-<name>`.
    pub fn new(prefix: &str, name: &str) -> Self {
        Self(std::env::temp_dir().join(format!("{prefix}-{}-{name}", std::process::id())))
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_file(&self.0).or_else(|_| std::fs::remove_dir_all(&self.0));
        }
    }
}

impl std::ops::Deref for TempPath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}
