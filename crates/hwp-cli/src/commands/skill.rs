//! `hwp skill` — 번들된 에이전트 스킬 관리.
//!
//! 저장소 정본 `skills/hwp/SKILL.md`를 `include_str!`로 바이너리에 임베드해 두고,
//! `hwp skill export`가 그대로 파일로 풀어낸다(생성물이라 기존 파일은 조용히 덮어쓴다).
//! `--install claude-code|codex`는 에이전트별 관례 디렉터리(`~/.claude/skills/hwp/`,
//! `~/.codex/skills/hwp/`)를 골라 주는 지름길일 뿐 동작은 같다.
//!
//! 홈 디렉터리 해석과 대상 결정은 순수 함수로 분리해 단위 테스트한다 — 테스트가 실제
//! `$HOME`을 만지지 않게 경로 계산만 검증하고, 파일 쓰기는 임시 디렉터리에서만 한다.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;

use hwp_cli::cli::InstallTarget;

/// 임베드된 스킬 정본. 저장소의 `skills/hwp/SKILL.md`와 바이트 단위로 같다.
pub const SKILL_MD: &str = include_str!("../../../../skills/hwp/SKILL.md");

pub fn run(output: Option<PathBuf>, install: Option<InstallTarget>) -> anyhow::Result<()> {
    let dir = resolve_target_dir(output, install)?;
    let written = export(&dir)?;
    println!("{}", written.display());
    Ok(())
}

/// 출력 디렉터리를 정한다: `-o` 우선, `--install`이면 에이전트 스킬 디렉터리,
/// 둘 다 없으면 `./hwp`. 둘 다 있으면 거부(clap이 `conflicts_with`로 먼저 막는다).
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

/// `--install` 대상의 스킬 디렉터리(`home` 아래 상대 경로 조립만 — IO 없음).
fn install_dir(target: InstallTarget, home: &Path) -> PathBuf {
    let agent = match target {
        InstallTarget::ClaudeCode => ".claude",
        InstallTarget::Codex => ".codex",
    };
    home.join(agent).join("skills").join("hwp")
}

/// 홈 디렉터리: 유닉스는 `$HOME`, Windows는 `$USERPROFILE` (새 크레이트 의존 없이).
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

/// `dir/SKILL.md`에 임베드된 내용을 쓴다(디렉터리가 없으면 만든다).
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
