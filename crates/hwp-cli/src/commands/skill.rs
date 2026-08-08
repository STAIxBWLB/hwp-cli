//! `hwp skill` — manages the bundled agent skill.
//!
//! The repository source of truth `skills/hwp/SKILL.md` is embedded into the
//! binary with `include_str!`, and `hwp skill export` writes it out verbatim
//! (a generated artifact, so an existing file is silently overwritten).
//! `--install claude-code|codex` is only a shortcut that picks the per-agent
//! conventional directory (`~/.claude/skills/hwp/`, `~/.codex/skills/hwp/`);
//! the behavior is the same.
//!
//! Home-directory resolution and target selection are split into pure functions
//! for unit testing — tests verify only the path computation without touching
//! the real `$HOME`, and file writes happen only in temporary directories.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use hwp_cli::cli::InstallTarget;

/// Embedded skill source of truth. Byte-identical to `skills/hwp/SKILL.md` in the repository.
pub const SKILL_MD: &str = include_str!("../../../../skills/hwp/SKILL.md");

pub fn run(output: Option<PathBuf>, install: Option<InstallTarget>) -> anyhow::Result<()> {
    let dir = resolve_target_dir(output, install)?;
    let written = export(&dir)?;
    println!("{}", written.display());
    Ok(())
}

/// Picks the output directory: `-o` wins, `--install` picks the agent skill directory,
/// and with neither the default is `./hwp`. Both together are rejected (clap blocks them first via `conflicts_with`).
fn resolve_target_dir(
    output: Option<PathBuf>,
    install: Option<InstallTarget>,
) -> anyhow::Result<PathBuf> {
    match (output, install) {
        (Some(dir), None) => Ok(dir),
        (None, Some(target)) => Ok(install_dir(target, &home_dir()?)),
        (None, None) => Ok(PathBuf::from("./hwp")),
        (Some(_), Some(_)) => {
            anyhow::bail!("-o/--output 과 --install 은 동시에 쓸 수 없습니다")
        }
    }
}

/// Skill directory for an `--install` target (only assembles the relative path below `home` — no IO).
fn install_dir(target: InstallTarget, home: &Path) -> PathBuf {
    let agent = match target {
        InstallTarget::ClaudeCode => ".claude",
        InstallTarget::Codex => ".codex",
    };
    home.join(agent).join("skills").join("hwp")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_bytes_match_embedded_source() {
        let dir = std::env::temp_dir().join(format!("hwp-cli-skill-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let written = export(&dir).expect("export");
        assert_eq!(
            fs::read(&written).expect("read back"),
            SKILL_MD.as_bytes(),
            "exported file must byte-match the embedded source"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_dir_resolves_under_given_home() {
        let home = Path::new("home");
        assert_eq!(
            install_dir(InstallTarget::ClaudeCode, home),
            Path::new("home/.claude/skills/hwp")
        );
        assert_eq!(
            install_dir(InstallTarget::Codex, home),
            Path::new("home/.codex/skills/hwp")
        );
    }

    #[test]
    fn target_dir_defaults_and_conflict() {
        assert_eq!(
            resolve_target_dir(Some(PathBuf::from("x")), None).unwrap(),
            PathBuf::from("x")
        );
        assert_eq!(
            resolve_target_dir(None, None).unwrap(),
            PathBuf::from("./hwp")
        );
        assert!(resolve_target_dir(Some(PathBuf::from("x")), Some(InstallTarget::Codex)).is_err());
    }
}
