//! MCP file authority: the `--root` sandbox and font directory resolution.
//!
//! 경로 검사를 여기 모아 두어 transport adapter들이 하나의 인가 표면을 공유한다.
//! Windows는 canonical path를 verbatim 철자로 유지하고, containment가 통과한 뒤에만
//! sandbox 호환 철자를 파생한다(아래 함수 문서 참고).

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::arg_str_opt;

/// Server context (default font directories for render/diff, `--root` file access sandbox).
pub struct Ctx {
    pub font_dirs: Vec<PathBuf>,
    /// Canonicalized allowed roots. Empty means unrestricted file access (previous behavior).
    pub roots: Vec<PathBuf>,
}

/// Canonicalize a path for sandbox authorization.
///
/// Keep Windows canonical paths in their verbatim spelling here. Lower-level
/// template and asset checks also use `std::fs::canonicalize`, so the roots in
/// `Ctx` must retain the same security identity. A sandbox-compatible spelling
/// is derived only after containment succeeds.
pub(super) fn canonicalize_mcp_path(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Derive the spelling used for downstream read-only filesystem I/O from an
/// already authorized canonical path.
fn sandbox_compatible_mcp_path(canonical: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        strip_windows_verbatim_prefix(canonical.to_path_buf())
    }
    #[cfg(not(windows))]
    {
        canonical.to_path_buf()
    }
}

/// Derive a spelling that remains ordinary even after the atomic writer adds
/// its private sibling workspace and staged filename.
pub(super) fn sandbox_compatible_mcp_write_path(canonical: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;

        // Compared with the destination, StagedOutput's longest current path adds:
        // leading dot + marker + max u32 pid + max u64 sequence + separators +
        // 32-char random token + `.tmp` + workspace separator, then either repeats
        // the destination filename or uses `destination.backup`. Certification has
        // a deeper fixed report tree, so reserve its larger relative expansion too.
        const ATOMIC_STAGING_FIXED_OVERHEAD_UTF16: usize = 82;
        const ATOMIC_STAGING_MIN_CHILD_NAME_UTF16: usize = 18;
        let file_name_units = canonical
            .file_name()
            .map(|name| name.encode_wide().count())
            .unwrap_or(0);
        let output_staging_budget = ATOMIC_STAGING_FIXED_OVERHEAD_UTF16
            .saturating_add(file_name_units.max(ATOMIC_STAGING_MIN_CHILD_NAME_UTF16));
        strip_windows_verbatim_prefix_with_budget(
            canonical.to_path_buf(),
            output_staging_budget
                .max(hwp_cli::certification::WINDOWS_CERTIFICATION_TREE_OVERHEAD_UTF16),
        )
    }
    #[cfg(not(windows))]
    {
        canonical.to_path_buf()
    }
}

#[cfg(windows)]
fn windows_ordinary_component_is_safe(component: &std::ffi::OsStr) -> bool {
    let Some(text) = component.to_str() else {
        return false;
    };
    if text.is_empty() || text.ends_with('.') || text.ends_with(' ') {
        return false;
    }
    if text.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
    }) {
        return false;
    }

    let stem = text
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return false;
    }
    for prefix in ["COM", "LPT"] {
        if stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        }) {
            return false;
        }
    }
    true
}

#[cfg(windows)]
fn windows_verbatim_components_are_ordinary_safe(path: &Path) -> bool {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(_) => {}
            Prefix::VerbatimUNC(server, share) => {
                if !windows_ordinary_component_is_safe(server)
                    || !windows_ordinary_component_is_safe(share)
                {
                    return false;
                }
            }
            _ => return false,
        },
        _ => return false,
    }
    components.all(|component| match component {
        Component::RootDir => true,
        Component::Normal(component) => windows_ordinary_component_is_safe(component),
        _ => false,
    })
}

#[cfg(windows)]
pub(super) fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    strip_windows_verbatim_prefix_with_budget(path, 0)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix_with_budget(
    path: PathBuf,
    additional_utf16_units: usize,
) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    // Rust's Windows path layer switches ordinary absolute paths back to verbatim
    // spelling at this legacy directory-path threshold. Leave room for the NUL
    // terminator and for any downstream path expansion supplied by the caller.
    const LEGACY_MAX_PATH_UTF16: usize = 248;
    const SLASH: u16 = b'\\' as u16;
    const VERBATIM: [u16; 4] = [SLASH, SLASH, b'?' as u16, SLASH];
    const VERBATIM_UNC: [u16; 8] = [
        SLASH,
        SLASH,
        b'?' as u16,
        SLASH,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        SLASH,
    ];

    if !windows_verbatim_components_are_ordinary_safe(&path) {
        return path;
    }

    let fits_ordinary_io = |ordinary_units: usize| {
        ordinary_units
            .checked_add(additional_utf16_units)
            .and_then(|units| units.checked_add(1))
            .is_some_and(|units_with_nul| units_with_nul < LEGACY_MAX_PATH_UTF16)
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.starts_with(&VERBATIM_UNC) {
        let mut ordinary = Vec::with_capacity(wide.len() - VERBATIM_UNC.len() + 2);
        ordinary.extend_from_slice(&[SLASH, SLASH]);
        ordinary.extend_from_slice(&wide[VERBATIM_UNC.len()..]);
        if fits_ordinary_io(ordinary.len()) {
            return PathBuf::from(OsString::from_wide(&ordinary));
        }
        return path;
    }
    if wide.starts_with(&VERBATIM)
        && wide.get(5) == Some(&(b':' as u16))
        && wide
            .get(4)
            .is_some_and(|letter| matches!(*letter, 65..=90 | 97..=122))
    {
        let ordinary = &wide[VERBATIM.len()..];
        if fits_ordinary_io(ordinary.len()) {
            return PathBuf::from(OsString::from_wide(ordinary));
        }
    }
    path
}

// ---- Path sandbox (`--root`) ----

/// Checks that a canonical path sits below one of the allowed roots.
fn under_any_root(ctx: &Ctx, canonical: &Path, raw: &str) -> Result<PathBuf, String> {
    if ctx
        .roots
        .iter()
        .any(|root| canonical_path_starts_with(canonical, root))
    {
        Ok(canonical.to_path_buf())
    } else {
        Err(format!(
            "허용된 --root 밖 경로라 거부합니다: {raw} ({}으로 확인됨)",
            canonical.display()
        ))
    }
}

pub(super) fn canonical_path_starts_with(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        if path.starts_with(root) {
            return true;
        }
        let path = strip_windows_verbatim_prefix(path.to_path_buf());
        let root = strip_windows_verbatim_prefix(root.to_path_buf());
        path.starts_with(root)
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

/// Read-path validation: the path must exist (canonicalize) and the canonical result
/// must sit below a root. Empty roots pass without a check (previous behavior).
pub(super) fn checked_read_path(ctx: &Ctx, raw: &str) -> Result<PathBuf, String> {
    if ctx.roots.is_empty() {
        return Ok(PathBuf::from(raw));
    }
    let canonical = canonicalize_mcp_path(Path::new(raw))
        .map_err(|error| format!("경로를 확인할 수 없습니다: {raw} ({error})"))?;
    let authorized = under_any_root(ctx, &canonical, raw)?;
    Ok(sandbox_compatible_mcp_path(&authorized))
}

/// Write-path validation: rejects `..` components and a missing file name, then
/// canonicalizes an existing file (blocking symlink-overwrite bypasses) or, for a new
/// file, canonicalizes the parent and rejoins, before the root check.
/// Empty roots pass without a check (previous behavior).
pub(super) fn checked_write_path(ctx: &Ctx, raw: &str) -> Result<PathBuf, String> {
    if ctx.roots.is_empty() {
        return Ok(PathBuf::from(raw));
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("'..'를 포함한 출력 경로는 거부합니다: {raw}"));
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("출력 경로에 파일 이름이 없습니다: {raw}"))?;
    let resolved = if path.exists() {
        canonicalize_mcp_path(path)
            .map_err(|error| format!("출력 경로를 확인할 수 없습니다: {raw} ({error})"))?
    } else {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_parent = canonicalize_mcp_path(parent).map_err(|error| {
            format!(
                "출력 경로의 부모 디렉터리를 확인할 수 없습니다: {} ({error})",
                parent.display()
            )
        })?;
        canonical_parent.join(file_name)
    };
    let authorized = under_any_root(ctx, &resolved, raw)?;
    Ok(sandbox_compatible_mcp_write_path(&authorized))
}

pub(super) fn font_dirs_for(args: &Value, ctx: &Ctx) -> Result<Vec<PathBuf>, String> {
    let mut dirs = ctx.font_dirs.clone();
    if let Some(d) = arg_str_opt(args, "font_dir")? {
        // Per-call font_dir is subject to the sandbox check (startup --font-dir is trusted).
        dirs.push(checked_read_path(ctx, d)?);
    }
    Ok(dirs)
}
