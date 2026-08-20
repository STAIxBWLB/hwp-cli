//! `hwp skill` — manages the bundled agent skill.
//!
//! The repository source of truth under `skills/hwp/` is embedded into the
//! binary as a hand-maintained `include_str!` table (`SKILL_FILES`), and
//! `hwp skill export` writes the whole tree out verbatim (generated
//! artifacts, so existing table-listed files are silently overwritten;
//! anything else in the target directory is left untouched).
//! `--install claude-code|codex|amazon-quick` selects the conventional skill
//! directory for that client. Amazon Quick keeps the single-file contract
//! (only `SKILL.md` is written) and its profiles are resolved through
//! `~/.quickwork/profiles.json` unless `--quick-profile` supplies an ID or an
//! absolute profile directory.
//!
//! Two drift gates keep the table honest: a test that fails when a file
//! under `skills/hwp/` is missing from `SKILL_FILES` or differs in bytes,
//! and a test that enforces H2/H3 structure parity plus the line-1 language
//! link for every `*.md`/`*.ko.md` pair in the tree.
//!
//! Home-directory and profile selection are split into pure helpers for unit
//! testing. Tests use temporary directories and never touch the real `$HOME`.

use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Component, Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

use hwp_cli::cli::InstallTarget;

/// Embedded skill source of truth. Byte-identical to `skills/hwp/SKILL.md` in the repository.
pub const SKILL_MD: &str = include_str!("../../../../skills/hwp/SKILL.md");

/// One file of the embedded skill tree: path relative to `skills/hwp/` plus
/// its byte-exact contents. Entries are compile-time constants, so no runtime
/// path input ever reaches the table.
pub struct EmbeddedFile {
    pub rel: &'static str,
    pub contents: &'static str,
}

/// The embedded skill tree (D-06/D-16), hand-maintained. The
/// `embedded_table_matches_skill_tree_on_disk` test fails when a file under
/// `skills/hwp/` (excluding `claude-web/`) is missing here or differs in
/// bytes, so every added tree file must gain an entry in the same commit.
pub const SKILL_FILES: &[EmbeddedFile] = &[
    EmbeddedFile {
        rel: "SKILL.md",
        contents: include_str!("../../../../skills/hwp/SKILL.md"),
    },
    EmbeddedFile {
        rel: "SKILL.ko.md",
        contents: include_str!("../../../../skills/hwp/SKILL.ko.md"),
    },
    EmbeddedFile {
        rel: "official-documents.md",
        contents: include_str!("../../../../skills/hwp/official-documents.md"),
    },
    EmbeddedFile {
        rel: "official-documents.ko.md",
        contents: include_str!("../../../../skills/hwp/official-documents.ko.md"),
    },
    EmbeddedFile {
        rel: "references/style-patterns.md",
        contents: include_str!("../../../../skills/hwp/references/style-patterns.md"),
    },
    EmbeddedFile {
        rel: "references/style-patterns.ko.md",
        contents: include_str!("../../../../skills/hwp/references/style-patterns.ko.md"),
    },
    EmbeddedFile {
        rel: "references/korean-official-format.md",
        contents: include_str!("../../../../skills/hwp/references/korean-official-format.md"),
    },
    EmbeddedFile {
        rel: "references/korean-official-format.ko.md",
        contents: include_str!("../../../../skills/hwp/references/korean-official-format.ko.md"),
    },
];

pub fn run(
    output: Option<PathBuf>,
    install: Option<InstallTarget>,
    quick_profile: Option<PathBuf>,
) -> anyhow::Result<()> {
    match resolve_target(output, install, quick_profile.as_deref())? {
        ExportTarget::Plain(dir) => {
            let written = export(&dir)?;
            println!("{}", written.display());
        }
        ExportTarget::QuickProfile(profile) => {
            let written = export_quick(&profile)?;
            println!("{}", written.display());
            println!(
                "참고: 공문서 안내·참고 문서·템플릿 등 공문서 관련 파일은 Amazon Quick에 설치되지 않았습니다 (SKILL.md만 설치됨)."
            );
        }
    }
    Ok(())
}

/// Where the skill gets written: a plain directory, or a resolved (canonical)
/// Amazon Quick profile directory that takes the symlink-guarded write path.
enum ExportTarget {
    Plain(PathBuf),
    QuickProfile(PathBuf),
}

/// Picks the export target. Clap rejects the common conflicts first; this
/// function repeats target-specific validation for callers that bypass clap.
fn resolve_target(
    output: Option<PathBuf>,
    install: Option<InstallTarget>,
    quick_profile: Option<&Path>,
) -> anyhow::Result<ExportTarget> {
    match (output, install) {
        (Some(dir), None) if quick_profile.is_none() => Ok(ExportTarget::Plain(dir)),
        (None, Some(target)) => install_target(target, quick_profile),
        (None, None) if quick_profile.is_none() => Ok(ExportTarget::Plain(PathBuf::from("./hwp"))),
        (Some(_), Some(_)) => {
            anyhow::bail!("-o/--output 과 --install 은 동시에 쓸 수 없습니다")
        }
        (_, None) => anyhow::bail!("--quick-profile 은 --install amazon-quick 과 함께 써야 합니다"),
    }
}

/// Export target for an `--install` choice. An absolute Quick profile needs
/// neither the registry nor `$HOME`, so it resolves before the home lookup.
fn install_target(
    target: InstallTarget,
    quick_profile: Option<&Path>,
) -> anyhow::Result<ExportTarget> {
    if matches!(target, InstallTarget::AmazonQuick)
        && let Some(profile) = quick_profile
        && profile.is_absolute()
    {
        return Ok(ExportTarget::QuickProfile(canonical_profile_dir(profile)?));
    }
    install_target_under(target, &home_dir()?, quick_profile)
}

/// Path assembly below `home` (unit-tested without touching the real `$HOME`).
fn install_target_under(
    target: InstallTarget,
    home: &Path,
    quick_profile: Option<&Path>,
) -> anyhow::Result<ExportTarget> {
    let agent = match target {
        InstallTarget::ClaudeCode => ".claude",
        InstallTarget::Codex => ".codex",
        InstallTarget::AmazonQuick => {
            let profile = resolve_quick_profile(&home.join(".quickwork"), quick_profile)?;
            return Ok(ExportTarget::QuickProfile(profile));
        }
    };
    if quick_profile.is_some() {
        anyhow::bail!("--quick-profile 은 --install amazon-quick 전용입니다");
    }
    Ok(ExportTarget::Plain(home.join(agent).join("skills/hwp")))
}

#[derive(Deserialize)]
struct QuickProfiles {
    version: u32,
    entries: Vec<QuickProfileEntry>,
    last_active: Option<String>,
}

#[derive(Deserialize)]
struct QuickProfileEntry {
    id: String,
    data_path: String,
}

/// Resolves the Amazon Quick profile directory through the registry (absolute
/// `--quick-profile` overrides are handled earlier, in `install_target`).
///
/// Registry entries are validated one at a time — the registry file is owned
/// by Amazon Quick, so one corrupt row must not break installs into other
/// profiles.
fn resolve_quick_profile(quick_root: &Path, profile: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(profile) = profile {
        let id = quick_profile_id(profile)?;
        let registry = load_quick_profiles(quick_root)?;
        let mut matches = registry.entries.iter().filter(|entry| entry.id == id);
        let entry = matches.next().ok_or_else(|| {
            anyhow::anyhow!(
                "Amazon Quick 프로필을 찾을 수 없습니다: {id}. 등록된 프로필: {}",
                profile_id_list(registry.entries.iter().map(|entry| entry.id.as_str()))
            )
        })?;
        if matches.next().is_some() {
            anyhow::bail!("Amazon Quick 프로필 ID가 중복됩니다: {id}");
        }
        return resolve_quick_entry(quick_root, entry);
    }

    let registry = load_quick_profiles(quick_root)?;
    if let Some(last_active) = registry.last_active.as_deref()
        && let Some(entry) = registry
            .entries
            .iter()
            .find(|entry| entry.id == last_active)
        && let Ok(path) = resolve_quick_entry(quick_root, entry)
    {
        return Ok(path);
    }

    let mut resolvable = Vec::new();
    for entry in &registry.entries {
        if let Ok(path) = resolve_quick_entry(quick_root, entry) {
            resolvable.push((entry.id.as_str(), path));
        }
    }
    if resolvable.len() == 1 {
        return Ok(resolvable.pop().expect("length checked").1);
    }
    anyhow::bail!(
        "활성 Amazon Quick 프로필을 결정할 수 없습니다. --quick-profile <ID_OR_ABSOLUTE_PATH>를 지정하세요. 사용 가능한 프로필: {}",
        profile_id_list(resolvable.iter().map(|(id, _)| *id))
    )
}

/// Extracts a profile ID from a relative `--quick-profile` value: exactly one
/// normal component (a trailing separator is tolerated and stripped).
fn quick_profile_id(profile: &Path) -> anyhow::Result<String> {
    let mut components = profile.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(id)), None) => Ok(id.to_string_lossy().into_owned()),
        _ => anyhow::bail!(
            "상대 Quick 프로필 경로는 지원하지 않습니다: {} (프로필 ID 또는 절대 경로를 사용하세요)",
            profile.display()
        ),
    }
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
            "Amazon Quick 프로필 레지스트리를 해석할 수 없습니다: {}",
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

/// Validates and canonicalizes a single registry entry.
fn resolve_quick_entry(quick_root: &Path, entry: &QuickProfileEntry) -> anyhow::Result<PathBuf> {
    if entry.id.trim().is_empty() {
        anyhow::bail!("Amazon Quick 프로필 ID가 비어 있습니다");
    }
    let relative = validate_data_path(&entry.data_path)?;
    let candidate = quick_root.join(relative);
    let canonical = fs::canonicalize(&candidate).with_context(|| {
        format!(
            "Amazon Quick 프로필 디렉터리가 없거나 확인할 수 없습니다: {}",
            candidate.display()
        )
    })?;
    if !canonical.is_dir() {
        anyhow::bail!(
            "Amazon Quick 프로필 경로가 디렉터리가 아닙니다: {}",
            candidate.display()
        );
    }
    let canonical_root = fs::canonicalize(quick_root).with_context(|| {
        format!(
            "Amazon Quick 데이터 디렉터리를 확인할 수 없습니다: {}",
            quick_root.display()
        )
    })?;
    if !canonical.starts_with(&canonical_root) {
        anyhow::bail!(
            "Amazon Quick 프로필 경로가 데이터 디렉터리 밖을 가리킵니다: {}",
            entry.data_path
        );
    }
    Ok(canonical)
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

fn profile_id_list<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    let mut ids: Vec<_> = ids.filter(|id| !id.trim().is_empty()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        "(없음)".to_owned()
    } else {
        ids.join(", ")
    }
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

/// Writes the embedded skill tree under `dir` (creating parent directories
/// as needed) and returns `dir`. Files in `dir` that are not listed in
/// `SKILL_FILES` are left untouched — export never deletes.
fn export(dir: &Path) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(dir)
        .with_context(|| format!("스킬 디렉터리를 만들 수 없습니다: {}", dir.display()))?;
    for file in SKILL_FILES {
        let path = dir.join(file.rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("스킬 디렉터리를 만들 수 없습니다: {}", parent.display())
            })?;
        }
        fs::write(&path, file.contents)
            .with_context(|| format!("{}를 쓸 수 없습니다: {}", file.rel, path.display()))?;
    }
    Ok(dir.to_path_buf())
}

/// Writes `skills/hwp/SKILL.md` under an already-resolved canonical Quick
/// profile directory without following pre-existing symlinks below it.
fn export_quick(profile_dir: &Path) -> anyhow::Result<PathBuf> {
    let skills_dir = profile_dir.join("skills");
    ensure_quick_directory(&skills_dir)?;
    let dir = skills_dir.join("hwp");
    ensure_quick_directory(&dir)?;

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
        Ok(_) => {
            fs::remove_file(&path).with_context(|| {
                format!("기존 SKILL.md를 제거할 수 없습니다: {}", path.display())
            })?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("SKILL.md 경로를 확인할 수 없습니다: {}", path.display())
            });
        }
    }
    // create_new (O_EXCL) never follows a symlink, so a link swapped in after
    // the check above cannot redirect the write.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("SKILL.md를 쓸 수 없습니다: {}", path.display()))?;
    file.write_all(SKILL_MD.as_bytes())
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

    match fs::create_dir(path) {
        Ok(()) => {}
        // Concurrently created by another install; the re-check below still validates it.
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Amazon Quick 스킬 디렉터리를 만들 수 없습니다: {}",
                    path.display()
                )
            });
        }
    }
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
        // D-08: line 1 is the EN/KO language link; the frontmatter block
        // follows it. (Unverified assumption: Amazon Quick tolerates the
        // leading link line — if not, a later phase strips it on the Quick
        // path.)
        assert!(
            SKILL_MD.starts_with("[한국어](SKILL.ko.md) · [English](SKILL.md)\n---\nname: hwp\n"),
            "SKILL.md must open with the line-1 language link followed by the ---/name: hwp frontmatter"
        );
        assert!(SKILL_MD.contains("hwp {command} --help"));
        assert!(
            !SKILL_MD.contains('<'),
            "SKILL.md must not contain angle-bracket markup; Amazon Quick rejects it as HTML/script content"
        );
    }

    #[test]
    fn exported_tree_matches_embedded_table() {
        let dir = temp_dir("skill-export");
        let written = export(&dir).expect("export");
        assert_eq!(written, dir, "export reports the directory it wrote");
        for file in SKILL_FILES {
            assert_eq!(
                fs::read(dir.join(file.rel)).expect("read back"),
                file.contents.as_bytes(),
                "exported {} must byte-match the embedded source",
                file.rel
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_never_touches_files_outside_the_table() {
        let dir = temp_dir("skill-export-merge");
        fs::create_dir_all(dir.join("references")).expect("pre-create");
        fs::write(dir.join("keep.txt"), b"keep").expect("seed stray file");
        fs::write(dir.join("references/keep.md"), b"keep").expect("seed stray nested file");
        fs::write(dir.join("SKILL.md"), b"stale").expect("seed stale table file");

        export(&dir).expect("export");
        assert_eq!(
            fs::read(dir.join("SKILL.md")).expect("read back"),
            SKILL_MD.as_bytes(),
            "table-listed files are overwritten"
        );
        assert_eq!(
            fs::read(dir.join("keep.txt")).expect("stray file"),
            b"keep",
            "non-table files must survive an export"
        );
        assert_eq!(
            fs::read(dir.join("references/keep.md")).expect("stray nested file"),
            b"keep",
            "non-table files in table subdirectories must survive an export"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_targets_receive_the_full_tree() {
        for target in [InstallTarget::ClaudeCode, InstallTarget::Codex] {
            let home = temp_dir("skill-install-tree");
            let dir = match install_target_under(target, &home, None).expect("resolve target") {
                ExportTarget::Plain(dir) => dir,
                ExportTarget::QuickProfile(_) => {
                    panic!("claude-code/codex resolve to a plain directory")
                }
            };
            let written = export(&dir).expect("export");
            assert_eq!(written, dir);
            for file in SKILL_FILES {
                assert_eq!(
                    fs::read(dir.join(file.rel)).expect("read back"),
                    file.contents.as_bytes(),
                    "installed {} must byte-match the embedded source",
                    file.rel
                );
            }
            let _ = fs::remove_dir_all(&home);
        }
    }

    #[test]
    fn fixed_install_targets_resolve_under_given_home() {
        let home = Path::new("home");
        assert!(matches!(
            install_target_under(InstallTarget::ClaudeCode, home, None).unwrap(),
            ExportTarget::Plain(dir) if dir == Path::new("home/.claude/skills/hwp")
        ));
        assert!(matches!(
            install_target_under(InstallTarget::Codex, home, None).unwrap(),
            ExportTarget::Plain(dir) if dir == Path::new("home/.codex/skills/hwp")
        ));
        assert!(
            install_target_under(
                InstallTarget::Codex,
                home,
                Some(Path::new("enterprise-test"))
            )
            .is_err()
        );
    }

    #[test]
    fn target_defaults_and_conflicts() {
        assert!(matches!(
            resolve_target(Some(PathBuf::from("x")), None, None).unwrap(),
            ExportTarget::Plain(dir) if dir == Path::new("x")
        ));
        assert!(matches!(
            resolve_target(None, None, None).unwrap(),
            ExportTarget::Plain(dir) if dir == Path::new("./hwp")
        ));
        assert!(
            resolve_target(Some(PathBuf::from("x")), Some(InstallTarget::Codex), None).is_err()
        );
        assert!(resolve_target(None, None, Some(Path::new("profile"))).is_err());
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
            resolve_quick_profile(&quick_root, Some(Path::new("enterprise-one/"))).unwrap(),
            fs::canonicalize(&profile).unwrap(),
            "a trailing separator from shell completion must still match the ID"
        );
        assert!(matches!(
            install_target(InstallTarget::AmazonQuick, Some(&profile)).unwrap(),
            ExportTarget::QuickProfile(dir) if dir == fs::canonicalize(&profile).unwrap()
        ));
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
    fn quick_bad_registry_entry_only_breaks_its_own_profile() {
        let root = temp_dir("quick-bad-entry");
        let quick_root = root.join(".quickwork");
        let good = make_profile(&quick_root, "good");
        write_registry(
            &quick_root,
            &[("bad", "/absolute"), ("good", "profiles/good")],
            None,
        );

        assert_eq!(
            resolve_quick_profile(&quick_root, Some(Path::new("good"))).unwrap(),
            fs::canonicalize(&good).unwrap(),
            "a corrupt sibling row must not break an explicitly requested valid profile"
        );
        assert!(resolve_quick_profile(&quick_root, Some(Path::new("bad"))).is_err());
        assert_eq!(
            resolve_quick_profile(&quick_root, None).unwrap(),
            fs::canonicalize(&good).unwrap(),
            "auto-selection skips corrupt rows and picks the sole valid profile"
        );

        write_registry(
            &quick_root,
            &[("dup", "profiles/good"), ("dup", "profiles/good")],
            None,
        );
        assert!(
            resolve_quick_profile(&quick_root, Some(Path::new("dup")))
                .expect_err("duplicate id")
                .to_string()
                .contains("중복")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_missing_profile_dir_reports_directory_error() {
        let root = temp_dir("quick-missing-dir");
        let quick_root = root.join(".quickwork");
        fs::create_dir_all(&quick_root).unwrap();
        write_registry(&quick_root, &[("work", "profiles/work")], None);
        let error = resolve_quick_profile(&quick_root, Some(Path::new("work")))
            .expect_err("missing profile directory")
            .to_string();
        assert!(
            error.contains("디렉터리가 없거나"),
            "a registered profile with a missing directory must not be reported as unknown: {error}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_profile_rejects_missing_malformed_unsupported_and_traversing_registry() {
        let missing_root = temp_dir("quick-missing");
        fs::create_dir_all(&missing_root).unwrap();
        assert!(resolve_quick_profile(&missing_root, None).is_err());
        assert!(
            resolve_quick_profile(&missing_root, Some(Path::new("a/b")))
                .expect_err("relative path with missing registry")
                .to_string()
                .contains("상대 Quick 프로필"),
            "the relative-path rejection must not be masked by a registry error"
        );
        let _ = fs::remove_dir_all(missing_root);

        let malformed_root = temp_dir("quick-malformed");
        fs::create_dir_all(&malformed_root).unwrap();
        fs::write(malformed_root.join("profiles.json"), b"not json").unwrap();
        assert!(resolve_quick_profile(&malformed_root, None).is_err());
        fs::write(
            malformed_root.join("profiles.json"),
            br#"{"version":"1","entries":[]}"#,
        )
        .unwrap();
        let error = resolve_quick_profile(&malformed_root, None)
            .expect_err("schema mismatch")
            .to_string();
        assert!(error.contains("해석할 수 없습니다"), "{error}");
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
        assert!(
            resolve_quick_profile(&traversal_root, Some(Path::new("escape")))
                .expect_err("traversing data_path")
                .to_string()
                .contains("안전하지 않은")
        );
        let _ = fs::remove_dir_all(traversal_root);
    }

    #[cfg(unix)]
    #[test]
    fn quick_registry_rejects_symlinked_entry_escaping_quick_root() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("quick-escape");
        let quick_root = root.join(".quickwork");
        let outside = root.join("outside");
        fs::create_dir_all(quick_root.join("profiles")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, quick_root.join("profiles/link")).unwrap();
        write_registry(&quick_root, &[("link", "profiles/link")], None);

        assert!(
            resolve_quick_profile(&quick_root, Some(Path::new("link")))
                .expect_err("in-root symlink escaping the quick root")
                .to_string()
                .contains("밖을 가리킵니다")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_absolute_profile_does_not_require_registry_or_home() {
        let root = temp_dir("quick-absolute-no-registry");
        let profile = root.join("profile");
        fs::create_dir_all(&profile).unwrap();
        // install_target resolves an absolute profile before the $HOME lookup
        // and never opens ~/.quickwork/profiles.json.
        assert!(matches!(
            install_target(InstallTarget::AmazonQuick, Some(&profile)).unwrap(),
            ExportTarget::QuickProfile(dir) if dir == fs::canonicalize(&profile).unwrap()
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quick_export_writes_and_overwrites_regular_file() {
        let root = temp_dir("quick-export");
        let profile = root.join("profile");
        fs::create_dir_all(&profile).unwrap();
        let profile = fs::canonicalize(&profile).unwrap();

        let written = export_quick(&profile).expect("first export");
        assert_eq!(written, profile.join("skills/hwp/SKILL.md"));
        assert_eq!(fs::read(&written).unwrap(), SKILL_MD.as_bytes());

        fs::write(&written, b"stale").unwrap();
        export_quick(&profile).expect("overwrite");
        assert_eq!(fs::read(&written).unwrap(), SKILL_MD.as_bytes());
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

        symlink(&outside, canonical_profile.join("skills")).unwrap();
        assert!(export_quick(&canonical_profile).is_err());
        assert!(!outside.join("hwp/SKILL.md").exists());

        fs::remove_file(canonical_profile.join("skills")).unwrap();
        fs::create_dir_all(canonical_profile.join("skills/hwp")).unwrap();
        let outside_file = outside.join("SKILL.md");
        fs::write(&outside_file, b"keep").unwrap();
        symlink(&outside_file, canonical_profile.join("skills/hwp/SKILL.md")).unwrap();
        assert!(export_quick(&canonical_profile).is_err());
        assert_eq!(fs::read(&outside_file).unwrap(), b"keep");
        let _ = fs::remove_dir_all(root);
    }

    /// Repository root of the bundled skill tree (`crates/hwp-cli/` → repo root).
    fn skill_tree_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills/hwp")
    }

    /// Recursive walk collecting skill-relative file paths (forward-slash
    /// separated) under `root`, skipping the `claude-web/` subtree.
    fn walk_skill_tree(root: &Path) -> std::collections::BTreeSet<String> {
        let mut files = std::collections::BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read skill tree directory") {
                let path = entry.expect("directory entry").path();
                let rel = path
                    .strip_prefix(root)
                    .expect("entry under the walked root")
                    .to_path_buf();
                // claude-web/ is a repo/release artifact (web bundle
                // installer), not skill content shipped by `hwp skill export`.
                if rel.starts_with("claude-web") {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        files
    }

    /// Heading level (2 or 3) for every line starting with "## " or "### ",
    /// in document order. Plain line scan — no markdown parser.
    fn heading_levels(markdown: &str) -> Vec<u8> {
        markdown
            .lines()
            .filter_map(|line| {
                if line.starts_with("### ") {
                    Some(3)
                } else if line.starts_with("## ") {
                    Some(2)
                } else {
                    None
                }
            })
            .collect()
    }

    /// D-16 drift gate: the hand-maintained `SKILL_FILES` table must match the
    /// committed `skills/hwp/` tree exactly — set equality (a new tree file
    /// without a table entry fails here) plus byte equality per entry.
    /// `claude-web/` is excluded from both sides: it is a release/web-bundle
    /// artifact, not exported skill content (research Open Question Q2).
    #[test]
    fn embedded_table_matches_skill_tree_on_disk() {
        let root = skill_tree_root();
        let on_disk = walk_skill_tree(&root);
        let in_table: std::collections::BTreeSet<String> =
            SKILL_FILES.iter().map(|file| file.rel.to_owned()).collect();
        assert_eq!(
            on_disk, in_table,
            "\nSKILL_FILES 테이블이 skills/hwp/ 트리와 어긋남 — \
             빠진 파일은 SKILL_FILES에 항목을 추가하고, 없어진 파일은 항목을 제거하라 \
             (위 diff에 드리프트 경로가 표시됨)."
        );
        for file in SKILL_FILES {
            assert_eq!(
                fs::read(root.join(file.rel)).expect("read repo skill file"),
                file.contents.as_bytes(),
                "임베드된 {} 내용이 커밋된 파일과 다름 — include_str! 테이블 항목을 갱신하라",
                file.rel
            );
        }
    }

    /// D-17 parity gate: for every English `X.md` under `skills/hwp/` that has
    /// a Korean mirror `X.ko.md`, both files must have the identical heading
    /// structure — same H2/H3 count and order — and carry the language link on
    /// line 1. `templates/` is excluded: template bodies are Korean-only files
    /// with no mirrors by design (D-11, research Open Question Q1). Files
    /// without a mirror yet are skipped — only existing pairs are gated.
    #[test]
    fn en_ko_pairs_have_identical_structure() {
        let root = skill_tree_root();
        let files = walk_skill_tree(&root);
        for rel in files.iter().filter(|rel| {
            rel.ends_with(".md") && !rel.ends_with(".ko.md") && !rel.starts_with("templates/")
        }) {
            let ko_rel = format!("{}.ko.md", rel.strip_suffix(".md").expect(".md suffix"));
            if !root.join(&ko_rel).is_file() {
                continue;
            }
            let en = fs::read_to_string(root.join(rel)).expect("read EN file");
            let ko = fs::read_to_string(root.join(&ko_rel)).expect("read KO mirror");
            assert_eq!(
                heading_levels(&en),
                heading_levels(&ko),
                "\n{rel} 과 {ko_rel} 의 H2/H3 구조가 어긋남 — \
                 '## '/'### ' 헤딩의 개수와 순서가 같아야 함 (D-17)."
            );
            let stem = rel
                .rsplit('/')
                .next()
                .expect("file name")
                .strip_suffix(".md")
                .expect(".md suffix");
            assert!(
                en.lines()
                    .next()
                    .unwrap_or_default()
                    .contains(&format!("]({stem}.ko.md)")),
                "\n{rel} 첫 줄에 한국어 미러 링크가 필요: [한국어]({stem}.ko.md)"
            );
            assert!(
                ko.lines()
                    .next()
                    .unwrap_or_default()
                    .contains(&format!("]({stem}.md)")),
                "\n{ko_rel} 첫 줄에 영어 원문 링크가 필요: [English]({stem}.md)"
            );
        }
    }
}
