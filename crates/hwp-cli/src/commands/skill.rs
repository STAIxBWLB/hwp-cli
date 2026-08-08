//! `hwp skill` — manages the bundled agent skill.
//!
//! The repository source of truth `skills/hwp/SKILL.md` is embedded into the
//! binary with `include_str!`, and `hwp skill export` writes it out verbatim
//! (a generated artifact, so an existing file is silently overwritten).
//! `--install claude-code|codex|amazon-quick` selects the conventional skill
//! directory for that client. Amazon Quick profiles are resolved through
//! `~/.quickwork/profiles.json` unless `--quick-profile` supplies an ID or an
//! absolute profile directory.
//!
//! Home-directory and profile selection are split into pure helpers for unit
//! testing. Tests use temporary directories and never touch the real `$HOME`.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

use hwp_cli::cli::InstallTarget;

/// Embedded skill source of truth. Byte-identical to `skills/hwp/SKILL.md` in the repository.
pub const SKILL_MD: &str = include_str!("../../../../skills/hwp/SKILL.md");

pub fn run(
    output: Option<PathBuf>,
    install: Option<InstallTarget>,
    quick_profile: Option<PathBuf>,
) -> anyhow::Result<()> {
    let quick_install = matches!(install, Some(InstallTarget::AmazonQuick));
    let dir = resolve_target_dir(output, install, quick_profile.as_deref())?;
    let written = if quick_install {
        export_quick(&dir)?
    } else {
        export(&dir)?
    };
    println!("{}", written.display());
    Ok(())
}

/// Picks the output directory. Clap rejects the common conflicts first; this
/// function repeats target-specific validation for callers that bypass clap.
fn resolve_target_dir(
    output: Option<PathBuf>,
    install: Option<InstallTarget>,
    quick_profile: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    match (output, install) {
        (Some(dir), None) if quick_profile.is_none() => Ok(dir),
        (None, Some(target)) => install_dir(target, &home_dir()?, quick_profile),
        (None, None) if quick_profile.is_none() => Ok(PathBuf::from("./hwp")),
        (Some(_), Some(_)) => {
            anyhow::bail!("-o/--output 과 --install 은 동시에 쓸 수 없습니다")
        }
        (_, None) => anyhow::bail!("--quick-profile 은 --install amazon-quick 과 함께 써야 합니다"),
    }
}

/// Skill directory for an `--install` target.
fn install_dir(
    target: InstallTarget,
    home: &Path,
    quick_profile: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    match target {
        InstallTarget::ClaudeCode => {
            reject_quick_profile(quick_profile, target)?;
            Ok(home.join(".claude/skills/hwp"))
        }
        InstallTarget::Codex => {
            reject_quick_profile(quick_profile, target)?;
            Ok(home.join(".codex/skills/hwp"))
        }
        InstallTarget::AmazonQuick => {
            let profile = resolve_quick_profile(&home.join(".quickwork"), quick_profile)?;
            Ok(profile.join("skills/hwp"))
        }
    }
}

fn reject_quick_profile(profile: Option<&Path>, target: InstallTarget) -> anyhow::Result<()> {
    if profile.is_some() {
        anyhow::bail!(
            "--quick-profile 은 --install amazon-quick 전용입니다 (--install {} 에는 쓸 수 없습니다)",
            target.as_str()
        );
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct QuickProfiles {
    version: u32,
    entries: Vec<QuickProfileEntry>,
    last_active: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuickProfileEntry {
    id: String,
    data_path: String,
}

fn resolve_quick_profile(quick_root: &Path, profile: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(profile) = profile
        && profile.is_absolute()
    {
        return canonical_profile_dir(profile);
    }

    let registry = load_quick_profiles(quick_root)?;
    let entries = resolve_quick_entries(quick_root, &registry.entries)?;

    if let Some(profile) = profile {
        if profile.components().count() != 1 {
            anyhow::bail!(
                "상대 Quick 프로필 경로는 지원하지 않습니다: {} (프로필 ID 또는 절대 경로를 사용하세요)",
                profile.display()
            );
        }
        let id = profile.to_string_lossy();
        return entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.path.clone())
            .ok_or_else(|| anyhow::anyhow!(unknown_profile_message(&id, &entries)));
    }

    if let Some(last_active) = registry.last_active.as_deref()
        && let Some(entry) = entries.iter().find(|entry| entry.id == last_active)
    {
        return Ok(entry.path.clone());
    }

    if entries.len() == 1 {
        return Ok(entries[0].path.clone());
    }

    let available = available_profile_ids(&entries);
    anyhow::bail!(
        "활성 Amazon Quick 프로필을 결정할 수 없습니다. --quick-profile <ID_OR_ABSOLUTE_PATH>를 지정하세요. 사용 가능한 프로필: {available}"
    )
}

fn load_quick_profiles(quick_root: &Path) -> anyhow::Result<QuickProfiles> {
    let path = quick_root.join("profiles.json");
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "Amazon Quick 프로필 레지스트리를 읽을 수 없습니다: {}",
            path.display()
        )
    })?;
    let registry: QuickProfiles = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "Amazon Quick 프로필 레지스트리가 올바른 JSON이 아닙니다: {}",
            path.display()
        )
    })?;
    if registry.version != 1 {
        anyhow::bail!(
            "지원하지 않는 Amazon Quick 프로필 레지스트리 버전입니다: {} (지원 버전: 1)",
            registry.version
        );
    }
    Ok(registry)
}

#[derive(Debug)]
struct ResolvedQuickProfile {
    id: String,
    path: PathBuf,
}

fn resolve_quick_entries(
    quick_root: &Path,
    entries: &[QuickProfileEntry],
) -> anyhow::Result<Vec<ResolvedQuickProfile>> {
    let canonical_root = fs::canonicalize(quick_root).with_context(|| {
        format!(
            "Amazon Quick 데이터 디렉터리를 확인할 수 없습니다: {}",
            quick_root.display()
        )
    })?;
    let mut ids = HashSet::new();
    let mut resolved = Vec::new();

    for entry in entries {
        if entry.id.trim().is_empty() {
            anyhow::bail!("Amazon Quick 프로필 ID가 비어 있습니다");
        }
        if !ids.insert(entry.id.clone()) {
            anyhow::bail!("Amazon Quick 프로필 ID가 중복됩니다: {}", entry.id);
        }
        let relative = validate_data_path(&entry.data_path)?;
        let candidate = quick_root.join(relative);
        let canonical = match fs::canonicalize(&candidate) {
            Ok(path) if path.is_dir() => path,
            _ => continue,
        };
        if !canonical.starts_with(&canonical_root) {
            anyhow::bail!(
                "Amazon Quick 프로필 경로가 데이터 디렉터리 밖을 가리킵니다: {}",
                entry.data_path
            );
        }
        resolved.push(ResolvedQuickProfile {
            id: entry.id.clone(),
            path: canonical,
        });
    }

    Ok(resolved)
}

fn validate_data_path(data_path: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(data_path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("안전하지 않은 Amazon Quick data_path입니다: {data_path}");
    }
    Ok(path.to_path_buf())
}

fn canonical_profile_dir(path: &Path) -> anyhow::Result<PathBuf> {
    let canonical = fs::canonicalize(path).with_context(|| {
        format!(
            "Amazon Quick 프로필 디렉터리를 찾을 수 없습니다: {}",
            path.display()
        )
    })?;
    if !canonical.is_dir() {
        anyhow::bail!(
            "Amazon Quick 프로필 경로가 디렉터리가 아닙니다: {}",
            path.display()
        );
    }
    Ok(canonical)
}

fn available_profile_ids(entries: &[ResolvedQuickProfile]) -> String {
    if entries.is_empty() {
        return "(없음)".to_owned();
    }
    let mut ids: Vec<_> = entries.iter().map(|entry| entry.id.as_str()).collect();
    ids.sort_unstable();
    ids.join(", ")
}

fn unknown_profile_message(id: &str, entries: &[ResolvedQuickProfile]) -> String {
    format!(
        "Amazon Quick 프로필을 찾을 수 없습니다: {id}. 사용 가능한 프로필: {}",
        available_profile_ids(entries)
    )
}

/// Home directory: `$HOME` on unix, `$USERPROFILE` on Windows (no new crate dependency).
fn home_dir() -> anyhow::Result<PathBuf> {
    #[cfg(not(windows))]
    const VAR: &str = "HOME";
    #[cfg(windows)]
    const VAR: &str = "USERPROFILE";
    std::env::var_os(VAR)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .with_context(|| format!("홈 디렉터리 환경변수 ${VAR} 가 설정되어 있지 않습니다"))
}

/// Writes the embedded content to `dir/SKILL.md` (creating the directory if needed).
fn export(dir: &Path) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("스킬 디렉터리를 만들 수 없습니다: {}", dir.display()))?;
    let path = dir.join("SKILL.md");
    fs::write(&path, SKILL_MD)
        .with_context(|| format!("SKILL.md를 쓸 수 없습니다: {}", path.display()))?;
    Ok(path)
}

/// Writes into the canonical Quick profile without following pre-existing
/// symlinks below the profile root.
fn export_quick(dir: &Path) -> anyhow::Result<PathBuf> {
    let skills_dir = dir
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "skills"))
        .with_context(|| {
            format!(
                "Amazon Quick 스킬 경로가 올바르지 않습니다: {}",
                dir.display()
            )
        })?;
    let profile_dir = skills_dir.parent().with_context(|| {
        format!(
            "Amazon Quick 프로필 경로가 올바르지 않습니다: {}",
            dir.display()
        )
    })?;
    if dir.file_name().is_none_or(|name| name != "hwp") {
        anyhow::bail!(
            "Amazon Quick 스킬 경로가 올바르지 않습니다: {}",
            dir.display()
        );
    }
    let canonical_profile = fs::canonicalize(profile_dir).with_context(|| {
        format!(
            "Amazon Quick 프로필 디렉터리를 확인할 수 없습니다: {}",
            profile_dir.display()
        )
    })?;
    if canonical_profile != profile_dir {
        anyhow::bail!(
            "Amazon Quick 프로필 경로는 canonical 경로여야 합니다: {}",
            profile_dir.display()
        );
    }

    ensure_quick_directory(skills_dir)?;
    ensure_quick_directory(dir)?;

    let path = dir.join("SKILL.md");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!(
                "Amazon Quick SKILL.md 심볼릭 링크를 덮어쓸 수 없습니다: {}",
                path.display()
            )
        }
        Ok(metadata) if !metadata.is_file() => {
            anyhow::bail!(
                "Amazon Quick SKILL.md 경로가 일반 파일이 아닙니다: {}",
                path.display()
            )
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("SKILL.md 경로를 확인할 수 없습니다: {}", path.display())
            });
        }
    }
    fs::write(&path, SKILL_MD)
        .with_context(|| format!("SKILL.md를 쓸 수 없습니다: {}", path.display()))?;
    Ok(path)
}

fn ensure_quick_directory(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!(
                "Amazon Quick 스킬 경로의 심볼릭 링크를 사용할 수 없습니다: {}",
                path.display()
            )
        }
        Ok(metadata) if !metadata.is_dir() => {
            anyhow::bail!(
                "Amazon Quick 스킬 경로가 디렉터리가 아닙니다: {}",
                path.display()
            )
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Amazon Quick 스킬 경로를 확인할 수 없습니다: {}",
                    path.display()
                )
            });
        }
    }

    fs::create_dir(path).with_context(|| {
        format!(
            "Amazon Quick 스킬 디렉터리를 만들 수 없습니다: {}",
            path.display()
        )
    })?;
    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!(
            "Amazon Quick 스킬 디렉터리를 확인할 수 없습니다: {}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "Amazon Quick 스킬 디렉터리가 안전하지 않습니다: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("hwp-cli-{name}-{}-{nonce}", std::process::id()))
    }

    fn make_profile(quick_root: &Path, id: &str) -> PathBuf {
        let path = quick_root.join("profiles").join(id);
        fs::create_dir_all(&path).expect("create profile");
        path
    }

    fn write_registry(quick_root: &Path, entries: &[(&str, &str)], last_active: Option<&str>) {
        write_registry_version(quick_root, 1, entries, last_active);
    }

    fn write_registry_version(
        quick_root: &Path,
        version: u32,
        entries: &[(&str, &str)],
        last_active: Option<&str>,
    ) {
        fs::create_dir_all(quick_root).expect("create quick root");
        let entries: Vec<_> = entries
            .iter()
            .map(|(id, data_path)| serde_json::json!({ "id": id, "data_path": data_path }))
            .collect();
        fs::write(
            quick_root.join("profiles.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": version,
                "entries": entries,
                "last_active": last_active
            }))
            .expect("json"),
        )
        .expect("write registry");
    }

    #[test]
    fn embedded_skill_is_quick_publish_safe() {
        assert!(SKILL_MD.starts_with("---\nname: hwp\n"));
        assert!(SKILL_MD.contains("hwp {command} --help"));
        assert!(
            !SKILL_MD.contains('<'),
            "SKILL.md must not contain angle-bracket markup; Amazon Quick rejects it as HTML/script content"
        );
    }

    #[test]
    fn exported_bytes_match_embedded_source() {
        let dir = temp_dir("skill-export");
        let written = export(&dir).expect("export");
        assert_eq!(
            fs::read(&written).expect("read back"),
            SKILL_MD.as_bytes(),
            "exported file must byte-match the embedded source"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fixed_install_dirs_resolve_under_given_home() {
        let home = Path::new("home");
        assert_eq!(
            install_dir(InstallTarget::ClaudeCode, home, None).unwrap(),
            Path::new("home/.claude/skills/hwp")
        );
        assert_eq!(
            install_dir(InstallTarget::Codex, home, None).unwrap(),
            Path::new("home/.codex/skills/hwp")
        );
        assert!(
            install_dir(
                InstallTarget::Codex,
                home,
                Some(Path::new("enterprise-test"))
            )
            .is_err()
        );
    }

    #[test]
    fn target_dir_defaults_and_conflicts() {
        assert_eq!(
            resolve_target_dir(Some(PathBuf::from("x")), None, None).unwrap(),
            PathBuf::from("x")
        );
        assert_eq!(
            resolve_target_dir(None, None, None).unwrap(),
            PathBuf::from("./hwp")
        );
        assert!(
            resolve_target_dir(Some(PathBuf::from("x")), Some(InstallTarget::Codex), None).is_err()
        );
        assert!(resolve_target_dir(None, None, Some(Path::new("profile"))).is_err());
    }

    #[test]
    fn quick_profile_resolves_explicit_id_and_absolute_path() {
        let root = temp_dir("quick-explicit");
        let quick_root = root.join(".quickwork");
        let profile = make_profile(&quick_root, "enterprise-one");
        write_registry(
            &quick_root,
            &[("enterprise-one", "profiles/enterprise-one")],
            None,
        );

        assert_eq!(
            resolve_quick_profile(&quick_root, Some(Path::new("enterprise-one"))).unwrap(),
            fs::canonicalize(&profile).unwrap()
        );
        assert_eq!(
            resolve_quick_profile(&quick_root, Some(&profile)).unwrap(),
            fs::canonicalize(&profile).unwrap()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_profile_prefers_last_active_then_sole_valid_profile() {
        let root = temp_dir("quick-auto");
        let quick_root = root.join(".quickwork");
        let first = make_profile(&quick_root, "first");
        let second = make_profile(&quick_root, "second");
        write_registry(
            &quick_root,
            &[("first", "profiles/first"), ("second", "profiles/second")],
            Some("second"),
        );
        assert_eq!(
            resolve_quick_profile(&quick_root, None).unwrap(),
            fs::canonicalize(second).unwrap()
        );

        fs::remove_dir_all(&first).expect("remove first");
        write_registry(
            &quick_root,
            &[("first", "profiles/first"), ("second", "profiles/second")],
            Some("stale"),
        );
        assert_eq!(
            resolve_quick_profile(&quick_root, None).unwrap(),
            fs::canonicalize(quick_root.join("profiles/second")).unwrap()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_profile_rejects_ambiguity_unknown_id_and_relative_path() {
        let root = temp_dir("quick-errors");
        let quick_root = root.join(".quickwork");
        make_profile(&quick_root, "one");
        make_profile(&quick_root, "two");
        write_registry(
            &quick_root,
            &[("one", "profiles/one"), ("two", "profiles/two")],
            Some("stale"),
        );
        let error = resolve_quick_profile(&quick_root, None)
            .expect_err("ambiguous profiles")
            .to_string();
        assert!(error.contains("one, two"));
        assert!(
            resolve_quick_profile(&quick_root, Some(Path::new("missing")))
                .expect_err("unknown id")
                .to_string()
                .contains("one, two")
        );
        assert!(resolve_quick_profile(&quick_root, Some(Path::new("profiles/one"))).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_profile_rejects_missing_malformed_unsupported_and_traversing_registry() {
        let missing_root = temp_dir("quick-missing");
        fs::create_dir_all(&missing_root).unwrap();
        assert!(resolve_quick_profile(&missing_root, None).is_err());
        let _ = fs::remove_dir_all(missing_root);

        let malformed_root = temp_dir("quick-malformed");
        fs::create_dir_all(&malformed_root).unwrap();
        fs::write(malformed_root.join("profiles.json"), b"not json").unwrap();
        assert!(resolve_quick_profile(&malformed_root, None).is_err());
        let _ = fs::remove_dir_all(malformed_root);

        let unsupported_root = temp_dir("quick-unsupported");
        write_registry_version(&unsupported_root, 2, &[], None);
        let error = resolve_quick_profile(&unsupported_root, None)
            .expect_err("unsupported registry version")
            .to_string();
        assert!(error.contains("지원 버전: 1"));
        let _ = fs::remove_dir_all(unsupported_root);

        let traversal_root = temp_dir("quick-traversal");
        fs::create_dir_all(&traversal_root).unwrap();
        write_registry(&traversal_root, &[("escape", "../outside")], None);
        assert!(resolve_quick_profile(&traversal_root, None).is_err());
        let _ = fs::remove_dir_all(traversal_root);
    }

    #[test]
    fn quick_absolute_profile_does_not_require_registry() {
        let root = temp_dir("quick-absolute-no-registry");
        let profile = root.join("profile");
        fs::create_dir_all(&profile).unwrap();
        let quick_root = root.join("missing-quick-root");
        assert_eq!(
            resolve_quick_profile(&quick_root, Some(&profile)).unwrap(),
            fs::canonicalize(&profile).unwrap()
        );

        fs::create_dir_all(&quick_root).unwrap();
        fs::write(quick_root.join("profiles.json"), b"not json").unwrap();
        assert_eq!(
            resolve_quick_profile(&quick_root, Some(&profile)).unwrap(),
            fs::canonicalize(&profile).unwrap()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn quick_export_rejects_directory_and_file_symlinks() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("quick-symlink");
        let profile = root.join("profile");
        let outside = root.join("outside");
        fs::create_dir_all(&profile).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let canonical_profile = fs::canonicalize(&profile).unwrap();
        let install_dir = canonical_profile.join("skills/hwp");

        symlink(&outside, canonical_profile.join("skills")).unwrap();
        assert!(export_quick(&install_dir).is_err());
        assert!(!outside.join("hwp/SKILL.md").exists());

        fs::remove_file(canonical_profile.join("skills")).unwrap();
        fs::create_dir_all(&install_dir).unwrap();
        let outside_file = outside.join("SKILL.md");
        fs::write(&outside_file, b"keep").unwrap();
        symlink(&outside_file, install_dir.join("SKILL.md")).unwrap();
        assert!(export_quick(&install_dir).is_err());
        assert_eq!(fs::read(&outside_file).unwrap(), b"keep");
        let _ = fs::remove_dir_all(root);
    }
}
