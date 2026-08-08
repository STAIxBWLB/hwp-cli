//! Contained, immutable asset snapshots for structured authoring.
//!
//! Asset paths are data, not ambient filesystem authority. This module accepts
//! only relative normal components below the spec directory, rejects links,
//! opens once, and derives validation/hash/embed bytes from the same bounded
//! handle. Errors intentionally contain no resolved filesystem path.

use std::fmt;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest as _, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetSnapshot {
    pub data: Vec<u8>,
    pub sha256: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetSnapshotErrorCode {
    InvalidPath,
    BaseUnavailable,
    Missing,
    SymlinkForbidden,
    HardlinkForbidden,
    NotRegular,
    ContainmentViolation,
    OutsideRoots,
    ChangedDuringOpen,
    LimitExceeded,
    ReadFailed,
}

impl AssetSnapshotErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPath => "invalid_path",
            Self::BaseUnavailable => "base_unavailable",
            Self::Missing => "missing",
            Self::SymlinkForbidden => "symlink_forbidden",
            Self::HardlinkForbidden => "hardlink_forbidden",
            Self::NotRegular => "not_regular",
            Self::ContainmentViolation => "containment_violation",
            Self::OutsideRoots => "outside_roots",
            Self::ChangedDuringOpen => "changed_during_open",
            Self::LimitExceeded => "limit_exceeded",
            Self::ReadFailed => "read_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetSnapshotError {
    pub code: AssetSnapshotErrorCode,
}

impl fmt::Display for AssetSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            AssetSnapshotErrorCode::InvalidPath => {
                "asset path must contain only relative normal components"
            }
            AssetSnapshotErrorCode::BaseUnavailable => "asset base directory is unavailable",
            AssetSnapshotErrorCode::Missing => "asset is unavailable",
            AssetSnapshotErrorCode::SymlinkForbidden => "asset symlinks are forbidden",
            AssetSnapshotErrorCode::HardlinkForbidden => "multiply-linked assets are forbidden",
            AssetSnapshotErrorCode::NotRegular => "asset is not a regular file",
            AssetSnapshotErrorCode::ContainmentViolation => "asset is outside the spec directory",
            AssetSnapshotErrorCode::OutsideRoots => "asset is outside the sandbox roots",
            AssetSnapshotErrorCode::ChangedDuringOpen => "asset changed while it was opened",
            AssetSnapshotErrorCode::LimitExceeded => "asset exceeds the byte limit",
            AssetSnapshotErrorCode::ReadFailed => "asset snapshot could not be read",
        })
    }
}

impl std::error::Error for AssetSnapshotError {}

pub fn validate_relative_path(path: &Path) -> Result<(), AssetSnapshotError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(error(AssetSnapshotErrorCode::InvalidPath));
    }
    Ok(())
}

pub fn read_contained(
    base_dir: &Path,
    relative_path: &Path,
    max_bytes: u64,
) -> Result<AssetSnapshot, AssetSnapshotError> {
    read_contained_impl(base_dir, relative_path, max_bytes, || {}, || {})
}

/// Defense-in-depth binding to the MCP sandbox roots. A no-op when `roots` is
/// empty (CLI/corpus callers); otherwise the canonical resolved asset must sit
/// under at least one root. Roots are expected to be canonical already.
pub fn check_under_roots(resolved: &Path, roots: &[PathBuf]) -> Result<(), AssetSnapshotError> {
    if roots.is_empty() {
        return Ok(());
    }
    let canonical =
        std::fs::canonicalize(resolved).map_err(|_| error(AssetSnapshotErrorCode::Missing))?;
    if roots.iter().any(|root| canonical.starts_with(root)) {
        Ok(())
    } else {
        Err(error(AssetSnapshotErrorCode::OutsideRoots))
    }
}

fn read_contained_impl(
    base_dir: &Path,
    relative_path: &Path,
    max_bytes: u64,
    after_parent_open: impl FnOnce(),
    after_open: impl FnOnce(),
) -> Result<AssetSnapshot, AssetSnapshotError> {
    validate_relative_path(relative_path)?;
    let file = secure_open_contained(base_dir, relative_path, after_parent_open)?;
    let opened = file
        .metadata()
        .map_err(|_| error(AssetSnapshotErrorCode::ReadFailed))?;
    if !opened.is_file() {
        return Err(error(AssetSnapshotErrorCode::NotRegular));
    }
    reject_reparse(&opened)?;
    reject_hardlink(&file, &opened)?;
    if opened.len() > max_bytes {
        return Err(error(AssetSnapshotErrorCode::LimitExceeded));
    }

    after_open();
    let mut data = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|_| error(AssetSnapshotErrorCode::ReadFailed))?;
    if data.len() as u64 > max_bytes {
        return Err(error(AssetSnapshotErrorCode::LimitExceeded));
    }
    let sha256 = Sha256::digest(&data).into();
    Ok(AssetSnapshot { data, sha256 })
}

#[cfg(unix)]
fn secure_open_contained(
    base_dir: &Path,
    relative_path: &Path,
    after_parent_open: impl FnOnce(),
) -> Result<std::fs::File, AssetSnapshotError> {
    use std::ffi::{CString, OsStr};
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    fn component_name(component: &OsStr) -> Result<CString, AssetSnapshotError> {
        CString::new(component.as_bytes()).map_err(|_| error(AssetSnapshotErrorCode::InvalidPath))
    }

    fn openat_component(
        directory: &std::fs::File,
        component: &OsStr,
        flags: i32,
    ) -> Result<std::fs::File, AssetSnapshotError> {
        let component = component_name(component)?;
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), component.as_ptr(), flags) };
        if descriptor < 0 {
            let os_error = std::io::Error::last_os_error();
            let code = match os_error.raw_os_error() {
                Some(libc::ELOOP) => AssetSnapshotErrorCode::SymlinkForbidden,
                Some(libc::ENOTDIR) => AssetSnapshotErrorCode::ContainmentViolation,
                _ => AssetSnapshotErrorCode::Missing,
            };
            return Err(error(code));
        }
        Ok(unsafe { std::fs::File::from_raw_fd(descriptor) })
    }

    let canonical_base = std::fs::canonicalize(base_dir)
        .map_err(|_| error(AssetSnapshotErrorCode::BaseUnavailable))?;
    let mut base_options = std::fs::OpenOptions::new();
    base_options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    let mut directory = base_options
        .open(canonical_base)
        .map_err(|_| error(AssetSnapshotErrorCode::BaseUnavailable))?;
    if !directory
        .metadata()
        .map_err(|_| error(AssetSnapshotErrorCode::BaseUnavailable))?
        .is_dir()
    {
        return Err(error(AssetSnapshotErrorCode::BaseUnavailable));
    }

    let components = relative_path.components().collect::<Vec<_>>();
    for component in &components[..components.len() - 1] {
        let Component::Normal(component) = component else {
            return Err(error(AssetSnapshotErrorCode::InvalidPath));
        };
        directory = openat_component(
            &directory,
            component,
            libc::O_RDONLY
                | libc::O_DIRECTORY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK,
        )?;
        let metadata = directory
            .metadata()
            .map_err(|_| error(AssetSnapshotErrorCode::ReadFailed))?;
        reject_reparse(&metadata)?;
        if !metadata.is_dir() {
            return Err(error(AssetSnapshotErrorCode::ContainmentViolation));
        }
    }
    after_parent_open();
    let Component::Normal(final_component) = components[components.len() - 1] else {
        return Err(error(AssetSnapshotErrorCode::InvalidPath));
    };
    openat_component(
        &directory,
        final_component,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
    )
}

#[cfg(windows)]
fn secure_open_contained(
    base_dir: &Path,
    relative_path: &Path,
    after_parent_open: impl FnOnce(),
) -> Result<std::fs::File, AssetSnapshotError> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    fn open_component(path: &Path) -> Result<std::fs::File, AssetSnapshotError> {
        let mut options = std::fs::OpenOptions::new();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        options
            .open(path)
            .map_err(|_| error(AssetSnapshotErrorCode::Missing))
    }

    let canonical_base = std::fs::canonicalize(base_dir)
        .map_err(|_| error(AssetSnapshotErrorCode::BaseUnavailable))?;
    let root = open_component(&canonical_base)
        .map_err(|_| error(AssetSnapshotErrorCode::BaseUnavailable))?;
    let root_metadata = root
        .metadata()
        .map_err(|_| error(AssetSnapshotErrorCode::BaseUnavailable))?;
    reject_reparse(&root_metadata)?;
    if !root_metadata.is_dir() {
        return Err(error(AssetSnapshotErrorCode::BaseUnavailable));
    }

    let components = relative_path.components().collect::<Vec<_>>();
    let mut candidate = canonical_base.clone();
    let mut retained_directories = vec![root];
    for component in &components[..components.len() - 1] {
        let Component::Normal(component) = component else {
            return Err(error(AssetSnapshotErrorCode::InvalidPath));
        };
        candidate.push(component);
        let directory = open_component(&candidate)?;
        let metadata = directory
            .metadata()
            .map_err(|_| error(AssetSnapshotErrorCode::ReadFailed))?;
        reject_reparse(&metadata)?;
        if !metadata.is_dir() {
            return Err(error(AssetSnapshotErrorCode::ContainmentViolation));
        }
        retained_directories.push(directory);
    }
    after_parent_open();
    let Component::Normal(final_component) = components[components.len() - 1] else {
        return Err(error(AssetSnapshotErrorCode::InvalidPath));
    };
    candidate.push(final_component);
    let file = open_component(&candidate)?;
    validate_opened_containment(&file, &canonical_base)?;
    drop(retained_directories);
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn secure_open_contained(
    base_dir: &Path,
    relative_path: &Path,
    after_parent_open: impl FnOnce(),
) -> Result<std::fs::File, AssetSnapshotError> {
    let base = std::fs::canonicalize(base_dir)
        .map_err(|_| error(AssetSnapshotErrorCode::BaseUnavailable))?;
    after_parent_open();
    let candidate = std::fs::canonicalize(base.join(relative_path))
        .map_err(|_| error(AssetSnapshotErrorCode::Missing))?;
    if !candidate.starts_with(&base) {
        return Err(error(AssetSnapshotErrorCode::ContainmentViolation));
    }
    std::fs::File::open(candidate).map_err(|_| error(AssetSnapshotErrorCode::Missing))
}

#[cfg(unix)]
fn reject_hardlink(
    _file: &std::fs::File,
    metadata: &std::fs::Metadata,
) -> Result<(), AssetSnapshotError> {
    use std::os::unix::fs::MetadataExt as _;
    if metadata.nlink() != 1 {
        return Err(error(AssetSnapshotErrorCode::HardlinkForbidden));
    }
    Ok(())
}

#[cfg(windows)]
fn reject_hardlink(
    file: &std::fs::File,
    _metadata: &std::fs::Metadata,
) -> Result<(), AssetSnapshotError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let loaded = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    if loaded == 0 {
        return Err(error(AssetSnapshotErrorCode::ReadFailed));
    }
    if information.nNumberOfLinks != 1 {
        return Err(error(AssetSnapshotErrorCode::HardlinkForbidden));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn reject_hardlink(
    _file: &std::fs::File,
    _metadata: &std::fs::Metadata,
) -> Result<(), AssetSnapshotError> {
    Ok(())
}

#[cfg(windows)]
fn reject_reparse(metadata: &std::fs::Metadata) -> Result<(), AssetSnapshotError> {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(error(AssetSnapshotErrorCode::SymlinkForbidden));
    }
    Ok(())
}

#[cfg(not(windows))]
fn reject_reparse(metadata: &std::fs::Metadata) -> Result<(), AssetSnapshotError> {
    if metadata.file_type().is_symlink() {
        return Err(error(AssetSnapshotErrorCode::SymlinkForbidden));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_opened_containment(
    file: &std::fs::File,
    canonical_base: &Path,
) -> Result<(), AssetSnapshotError> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_DOS,
        )
    };
    if length == 0 || length as usize >= buffer.len() {
        return Err(error(AssetSnapshotErrorCode::ReadFailed));
    }
    let opened = std::path::PathBuf::from(OsString::from_wide(&buffer[..length as usize]));
    if !windows_path_is_within(&opened, canonical_base) {
        return Err(error(AssetSnapshotErrorCode::ContainmentViolation));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_path_is_within(candidate: &Path, base: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        let value = path.as_os_str().to_string_lossy();
        let value = value.strip_prefix(r"\\?\UNC\").map_or_else(
            || value.strip_prefix(r"\\?\").unwrap_or(&value).to_string(),
            |suffix| format!(r"\\{suffix}"),
        );
        value.trim_end_matches(['\\', '/']).to_lowercase()
    }

    let candidate = normalized(candidate);
    let base = normalized(base);
    candidate == base
        || candidate
            .strip_prefix(&base)
            .is_some_and(|suffix| suffix.starts_with(['\\', '/']))
}

fn error(code: AssetSnapshotErrorCode) -> AssetSnapshotError {
    AssetSnapshotError { code }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_contract_rejects_ambient_authority() {
        for path in ["", ".", "a/../b", "../secret", "/etc/passwd"] {
            assert!(validate_relative_path(Path::new(path)).is_err(), "{path}");
        }
        assert!(validate_relative_path(Path::new("assets/image.png")).is_ok());
    }

    #[test]
    fn bounded_read_rejects_content_larger_than_the_limit() {
        let root = test_root("oversize");
        std::fs::write(root.join("large.bin"), b"12345").unwrap();

        let failure = read_contained(&root, Path::new("large.bin"), 4).unwrap_err();
        assert_eq!(failure.code, AssetSnapshotErrorCode::LimitExceeded);

        cleanup_root(&root);
    }

    #[test]
    fn roots_check_binds_resolved_assets_to_sandbox_roots() {
        let parent = test_root("roots-parent");
        let sandbox = parent.join("sandbox");
        std::fs::create_dir(&sandbox).unwrap();
        let inside = sandbox.join("asset.bin");
        let outside = parent.join("outside.bin");
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();

        check_under_roots(&outside, &[]).unwrap();
        let roots = vec![std::fs::canonicalize(&sandbox).unwrap()];
        check_under_roots(&inside, &roots).unwrap();
        let failure = check_under_roots(&outside, &roots).unwrap_err();
        assert_eq!(failure.code, AssetSnapshotErrorCode::OutsideRoots);
        assert!(!failure.to_string().contains(&parent.display().to_string()));

        cleanup_root(&parent);
    }

    #[cfg(unix)]
    #[test]
    fn pathname_swap_after_open_cannot_change_snapshot_bytes() {
        let root = test_root("swap");
        let asset = root.join("asset.bin");
        let replacement = root.join("replacement.bin");
        let opened_bytes = b"opened-before-swap";
        std::fs::write(&asset, opened_bytes).unwrap();
        std::fs::write(&replacement, b"replacement-canary").unwrap();

        let snapshot = read_contained_impl(
            &root,
            Path::new("asset.bin"),
            64,
            || {},
            || {
                std::fs::rename(&asset, root.join("opened.bin")).unwrap();
                std::fs::rename(&replacement, &asset).unwrap();
            },
        )
        .unwrap();

        assert_eq!(snapshot.data, opened_bytes);
        assert_ne!(snapshot.data, std::fs::read(&asset).unwrap());
        cleanup_root(&root);
    }

    #[cfg(unix)]
    #[test]
    fn parent_swap_to_outside_symlink_cannot_escape_open_directory() {
        use std::os::unix::fs::symlink;

        let root = test_root("parent-swap");
        let inside = root.join("assets");
        let outside = test_root("outside-canary");
        std::fs::create_dir(&inside).unwrap();
        std::fs::write(inside.join("image.bin"), b"contained-bytes").unwrap();
        std::fs::write(outside.join("image.bin"), b"outside-canary").unwrap();

        let snapshot = read_contained_impl(
            &root,
            Path::new("assets/image.bin"),
            64,
            || {
                std::fs::rename(&inside, root.join("opened-assets")).unwrap();
                symlink(&outside, &inside).unwrap();
            },
            || {},
        )
        .unwrap();

        assert_eq!(snapshot.data, b"contained-bytes");
        assert_ne!(snapshot.data, b"outside-canary");
        cleanup_root(&root);
        cleanup_root(&outside);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_hardlink_are_rejected_without_path_leak() {
        use std::os::unix::fs::symlink;

        let root = test_root("links");
        let target = root.join("target.bin");
        std::fs::write(&target, b"canary").unwrap();
        symlink(&target, root.join("link.bin")).unwrap();
        let symlink_error = read_contained(&root, Path::new("link.bin"), 64).unwrap_err();
        assert_eq!(symlink_error.code, AssetSnapshotErrorCode::SymlinkForbidden);
        assert!(
            !symlink_error
                .to_string()
                .contains(&root.display().to_string())
        );

        std::fs::hard_link(&target, root.join("hard.bin")).unwrap();
        let hardlink_error = read_contained(&root, Path::new("hard.bin"), 64).unwrap_err();
        assert_eq!(
            hardlink_error.code,
            AssetSnapshotErrorCode::HardlinkForbidden
        );

        cleanup_root(&root);
    }

    fn test_root(suffix: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "hwp-asset-snapshot-{}-{}-{suffix}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn cleanup_root(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }
}
