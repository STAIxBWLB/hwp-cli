//! `hwp update` — 자체 업데이트.
//!
//! GitHub 릴리스에 올라간 플랫폼별 아카이브(`release.yml`의 `upload-assets` 산출물)를 받아
//! 실행 중인 바이너리를 교체한다. **새 런타임 크레이트를 들이지 않으려고** 네트워크는
//! `curl`, 압축 해제는 `tar`에 위임한다(둘 다 macOS·Linux·Windows 10 1803+ 기본 탑재).
//! 외부 명령은 전부 인자 벡터로 실행한다 — 셸 문자열 조립 없음(주입 방지).
//!
//! Homebrew 설치본은 **덮어쓰지 않고** `brew upgrade hwp`에 위임한다. Cellar 바이너리를
//! 직접 갈아끼우면 brew 매니페스트와 어긋나 다음 `brew upgrade`가 조용히 되돌린다.
//!
//! 순수 로직(버전 비교·타깃 매핑·자산 이름·설치 종류 판별·파일 교체)과 부작용(curl/tar
//! 호출)을 나눠 뒀다 — 앞쪽은 네트워크 없이 단위 테스트한다.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REPO: &str = "STAIxBWLB/hwp-cli";
const BIN: &str = "hwp";

/// 릴리스 아카이브 형식 — 타깃별로 `release.yml`이 만드는 확장자.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archive {
    TarGz,
    Zip,
}

impl Archive {
    fn ext(self) -> &'static str {
        match self {
            Archive::TarGz => "tar.gz",
            Archive::Zip => "zip",
        }
    }
}

/// 설치 경로가 말해 주는 설치 방식 — 교체 전략이 갈린다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// Homebrew Cellar 아래 — brew에 위임한다.
    Brew,
    /// 그 외(직접 내려받기·`cargo install`) — 파일을 직접 교체해도 안전하다.
    Plain,
}

/// (os, arch) → 릴리스가 실제로 게시하는 타깃 트리플. 지원 밖이면 None.
/// 매트릭스 정본은 `.github/workflows/release.yml`의 `upload-assets`다.
pub fn target_triple(os: &str, arch: &str) -> Option<(&'static str, Archive)> {
    match (os, arch) {
        ("macos", "aarch64") => Some(("aarch64-apple-darwin", Archive::TarGz)),
        ("macos", "x86_64") => Some(("x86_64-apple-darwin", Archive::TarGz)),
        ("linux", "x86_64") => Some(("x86_64-unknown-linux-gnu", Archive::TarGz)),
        ("windows", "x86_64") => Some(("x86_64-pc-windows-msvc", Archive::Zip)),
        _ => None,
    }
}

/// 자산 이름 — `release.yml`의 `archive: $bin-$tag-$target` 규칙과 대칭.
/// 예: `hwp-v0.3.0-aarch64-apple-darwin.tar.gz` + 같은 이름의 `.sha256`.
pub fn asset_names(tag: &str, triple: &str, archive: Archive) -> (String, String) {
    let stem = format!("{BIN}-{tag}-{triple}");
    (
        format!("{stem}.{}", archive.ext()),
        format!("{stem}.sha256"),
    )
}

/// 태그가 `v?X.Y.Z[-pre]` 꼴인지 확인한다. URL·파일명에 그대로 들어가므로
/// 경로 탈출(`../`)·질의 문자열 주입을 여기서 막는다.
pub fn validate_tag(tag: &str) -> Result<()> {
    let body = tag.strip_prefix('v').unwrap_or(tag);
    let (core, pre) = match body.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (body, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let core_ok = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    let pre_ok = pre.is_none_or(|p| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    });
    if core_ok && pre_ok {
        Ok(())
    } else {
        bail!("릴리스 태그 형식이 아닙니다: {tag:?} (예: v0.3.0, v0.3.0-rc1)")
    }
}

/// 버전 문자열을 (major, minor, patch, 릴리스여부)로. 파싱 실패분은 0으로 취급한다.
fn parse_version(v: &str) -> (u64, u64, u64, bool) {
    let body = v.trim().trim_start_matches('v');
    let (core, pre) = match body.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (body, None),
    };
    let mut it = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        pre.is_none(), // 프리릴리스는 같은 (X.Y.Z) 정식판보다 낮다
    )
}

/// `latest`가 `current`보다 새 버전인가.
pub fn is_newer(current: &str, latest: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

/// 실행 파일 경로로 설치 방식을 판별한다. brew는 심볼릭 링크(`bin/hwp` → `Cellar/…`)를
/// 쓰므로 호출부가 canonical 경로를 넘긴다.
pub fn install_kind(exe: &Path) -> InstallKind {
    let p = exe.to_string_lossy().replace('\\', "/");
    if p.contains("/Cellar/") || p.contains("/homebrew/") || p.contains("/linuxbrew/") {
        InstallKind::Brew
    } else {
        InstallKind::Plain
    }
}

/// 새 바이너리를 제자리 교체한다. 임시 파일은 **대상과 같은 디렉터리**에 만든다
/// (rename은 같은 파일시스템 안에서만 원자적). 실패하면 백업으로 되돌린다.
pub fn replace_binary(target: &Path, new_file: &Path) -> Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("설치 경로에 상위 디렉터리가 없습니다: {}", target.display()))?;
    let staged = dir.join(format!(".{BIN}-update-{}", std::process::id()));
    let backup = dir.join(format!(".{BIN}-backup-{}", std::process::id()));

    std::fs::copy(new_file, &staged).with_context(|| {
        format!(
            "새 바이너리를 설치 위치에 놓지 못했습니다: {} (권한 확인: sudo가 필요할 수 있습니다)",
            staged.display()
        )
    })?;
    copy_exec_mode(target, &staged)?;

    // 원본을 백업으로 옮긴 뒤 새 것을 제자리로. Windows는 실행 중인 exe를 덮어쓸 수 없어
    // 이 "먼저 비켜 놓기"가 필수고, unix에서도 실패 시 복원 경로가 된다.
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(target, &backup)
        .with_context(|| format!("기존 바이너리를 비켜 놓지 못했습니다: {}", target.display()))?;
    if let Err(e) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&backup, target); // 되돌리기
        let _ = std::fs::remove_file(&staged);
        return Err(e).context("새 바이너리를 제자리에 놓지 못했습니다(원본 복원됨)");
    }
    // 실행 중인 파일이라 지우지 못할 수 있다(Windows) — 남아도 무해하다.
    let _ = std::fs::remove_file(&backup);
    Ok(())
}

#[cfg(unix)]
fn copy_exec_mode(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    // 기존 바이너리 권한을 승계하고, 없으면 0755.
    let mode = std::fs::metadata(from)
        .map(|m| m.permissions().mode())
        .unwrap_or(0o755);
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("실행 권한 설정 실패: {}", to.display()))
}

#[cfg(not(unix))]
fn copy_exec_mode(_from: &Path, _to: &Path) -> Result<()> {
    Ok(()) // Windows는 확장자로 실행 여부가 정해진다.
}

/// curl로 한 파일을 받는다. HTTPS·TLS1.2 이상만 허용하고 HTTP 오류는 실패로 만든다(-f).
fn download(url: &str, dest: &Path) -> Result<()> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--retry",
            "2",
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| anyhow!("curl 실행 실패({e}) — curl이 설치돼 있어야 합니다: {url}"))?;
    if !out.status.success() {
        bail!(
            "내려받기 실패: {url}\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// curl로 받은 본문을 문자열로 돌려준다(GitHub API 조회용).
fn fetch_text(url: &str) -> Result<String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--tlsv1.2",
            "-H",
            "Accept: application/vnd.github+json",
            url,
        ])
        .output()
        .map_err(|e| anyhow!("curl 실행 실패({e}) — curl이 설치돼 있어야 합니다"))?;
    if !out.status.success() {
        bail!(
            "릴리스 정보를 가져오지 못했습니다: {url}\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 최신 릴리스 태그(`vX.Y.Z`).
fn fetch_latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = fetch_text(&url)?;
    let v: Value = serde_json::from_str(&body).context("릴리스 JSON 파싱 실패")?;
    let tag = v
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("릴리스 응답에 tag_name이 없습니다"))?;
    validate_tag(tag)?;
    Ok(tag.to_string())
}

/// `.sha256` 자산 본문("<sha>  <파일명>")과 실제 파일 해시를 대조한다.
fn verify_sha256(file: &Path, sha_line: &str) -> Result<()> {
    let want = sha_line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if want.len() != 64 {
        bail!("체크섬 자산 형식이 이상합니다: {sha_line:?}");
    }
    let bytes = std::fs::read(file)
        .with_context(|| format!("내려받은 파일을 읽지 못했습니다: {}", file.display()))?;
    let got: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != want {
        bail!(
            "체크섬 불일치 — 내려받기가 손상됐거나 자산이 바뀌었습니다\n  기대: {want}\n  실제: {got}"
        );
    }
    Ok(())
}

/// 아카이브에서 `hwp` 실행 파일을 꺼낸다. tar는 zip도 풀 수 있어(bsdtar/Windows tar.exe)
/// 형식과 무관하게 같은 커맨드를 쓴다. 아카이브 루트에 바이너리가 있는 구조
/// (`taiki-e/upload-rust-binary-action` 산출물)를 전제한다.
fn extract(archive: &Path, dir: &Path) -> Result<PathBuf> {
    let out = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .output()
        .map_err(|e| anyhow!("tar 실행 실패({e}) — tar가 설치돼 있어야 합니다"))?;
    if !out.status.success() {
        bail!(
            "압축 해제 실패: {}\n{}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let exe = dir.join(if cfg!(windows) {
        format!("{BIN}.exe")
    } else {
        BIN.to_string()
    });
    if !exe.exists() {
        bail!(
            "아카이브에 {} 실행 파일이 없습니다: {}",
            BIN,
            archive.display()
        );
    }
    Ok(exe)
}

/// 교체된 바이너리가 실제로 기대 버전으로 실행되는지 확인한다.
fn verify_installed(exe: &Path, want: &str) -> Result<String> {
    let out = Command::new(exe)
        .arg("--version")
        .output()
        .with_context(|| format!("교체된 바이너리 실행 실패: {}", exe.display()))?;
    let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !ver.contains(want) {
        bail!("교체된 바이너리가 기대 버전이 아닙니다: {ver:?} (기대 {want})");
    }
    Ok(ver)
}

/// `hwp update` 진입점.
pub fn run(check: bool, tag: Option<&str>, force: bool, json: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let exe = std::env::current_exe().context("실행 파일 경로를 찾지 못했습니다")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let kind = install_kind(&exe);

    if let Some(t) = tag {
        validate_tag(t)?;
    }
    let target_tag = match tag {
        Some(t) => t.to_string(),
        None => fetch_latest_tag()?,
    };
    let latest = target_tag.trim_start_matches('v').to_string();
    let available = is_newer(current, &latest);

    if check {
        return report(
            json,
            json!({
                "current": current, "latest": latest, "update_available": available,
                "install": if kind == InstallKind::Brew { "brew" } else { "binary" },
                "path": exe.display().to_string(),
            }),
            &if available {
                format!("현재 {current} → 최신 {latest} (업데이트 있음)")
            } else {
                format!("현재 {current} — 최신입니다")
            },
        );
    }

    if latest == current && !force {
        return report(
            json,
            json!({"current": current, "latest": latest, "updated": false, "reason": "already-latest"}),
            &format!("이미 {current} 입니다 (다시 받으려면 --force)"),
        );
    }

    if kind == InstallKind::Brew {
        if tag.is_some() {
            bail!(
                "Homebrew 설치본은 버전 고정을 지원하지 않습니다 ({}).\n\
                 특정 버전이 필요하면 릴리스 아카이브를 직접 받아 쓰세요:\n  \
                 https://github.com/{REPO}/releases",
                exe.display()
            );
        }
        eprintln!("Homebrew 설치본입니다 — brew에 위임합니다: brew upgrade {BIN}");
        let status = Command::new("brew")
            .args(["upgrade", BIN])
            .status()
            .map_err(|e| anyhow!("brew 실행 실패({e}) — 직접 `brew upgrade {BIN}`을 실행하세요"))?;
        if !status.success() {
            bail!("brew upgrade {BIN} 실패 — 위 출력을 확인하세요");
        }
        return report(
            json,
            json!({"current": current, "latest": latest, "updated": true, "via": "brew"}),
            &format!("brew로 {latest} 설치 완료"),
        );
    }

    let (triple, archive) = target_triple(std::env::consts::OS, std::env::consts::ARCH)
        .ok_or_else(|| {
            anyhow!(
                "이 플랫폼({}/{})용 사전 빌드 바이너리가 없습니다 — 소스에서 설치하세요:\n  \
                 cargo install --git https://github.com/{REPO} hwp-cli",
                std::env::consts::OS,
                std::env::consts::ARCH,
            )
        })?;
    let (archive_name, sha_name) = asset_names(&target_tag, triple, archive);
    let base = format!("https://github.com/{REPO}/releases/download/{target_tag}");

    let work = std::env::temp_dir().join(format!("hwp-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).context("임시 작업 디렉터리 생성 실패")?;

    eprintln!("{current} → {latest} 내려받는 중… ({archive_name})");
    let archive_path = work.join(&archive_name);
    download(&format!("{base}/{archive_name}"), &archive_path)?;
    let sha_path = work.join(&sha_name);
    download(&format!("{base}/{sha_name}"), &sha_path)?;
    verify_sha256(&archive_path, &std::fs::read_to_string(&sha_path)?)?;

    let new_exe = extract(&archive_path, &work)?;
    replace_binary(&exe, &new_exe)?;
    let _ = std::fs::remove_dir_all(&work);

    let ver = verify_installed(&exe, &latest)?;
    report(
        json,
        json!({
            "current": current, "latest": latest, "updated": true, "via": "binary",
            "path": exe.display().to_string(),
        }),
        &format!("{ver} 설치 완료 ({})", exe.display()),
    )
}

fn report(json: bool, value: Value, text: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&value)?)?;
    } else {
        writeln!(out, "{text}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 버전_비교() {
        assert!(is_newer("0.3.0", "0.4.0"));
        assert!(is_newer("0.3.0", "0.3.1"));
        assert!(is_newer("0.9.0", "1.0.0"));
        assert!(is_newer("0.3.0", "v0.4.0")); // v 접두 허용
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.4.0", "0.3.9"));
        // 프리릴리스는 같은 정식판보다 낮다 — rc를 최신으로 오인해 내려깔지 않게.
        assert!(is_newer("0.3.0-rc1", "0.3.0"));
        assert!(!is_newer("0.3.0", "0.3.0-rc1"));
        // 두 자리 이상 버전을 문자열로 비교하면 뒤집힌다("10" < "9").
        assert!(is_newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn 타깃_매핑() {
        assert_eq!(
            target_triple("macos", "aarch64"),
            Some(("aarch64-apple-darwin", Archive::TarGz))
        );
        assert_eq!(
            target_triple("macos", "x86_64"),
            Some(("x86_64-apple-darwin", Archive::TarGz))
        );
        assert_eq!(
            target_triple("linux", "x86_64"),
            Some(("x86_64-unknown-linux-gnu", Archive::TarGz))
        );
        assert_eq!(
            target_triple("windows", "x86_64"),
            Some(("x86_64-pc-windows-msvc", Archive::Zip))
        );
        // 릴리스가 없는 조합은 소스 설치로 안내해야 하므로 None이어야 한다.
        assert_eq!(target_triple("linux", "aarch64"), None);
        assert_eq!(target_triple("freebsd", "x86_64"), None);
    }

    /// 실제 v0.3.0 릴리스에 올라간 자산 이름과 한 글자도 다르면 안 된다.
    #[test]
    fn 자산_이름() {
        let (a, s) = asset_names("v0.3.0", "aarch64-apple-darwin", Archive::TarGz);
        assert_eq!(a, "hwp-v0.3.0-aarch64-apple-darwin.tar.gz");
        assert_eq!(s, "hwp-v0.3.0-aarch64-apple-darwin.sha256");
        let (a, _) = asset_names("v0.3.0", "x86_64-pc-windows-msvc", Archive::Zip);
        assert_eq!(a, "hwp-v0.3.0-x86_64-pc-windows-msvc.zip");
    }

    /// 설치 스크립트와 자체 업데이트가 **같은 자산 이름 규칙**을 써야 한다. 한쪽만
    /// 바뀌면 설치나 업데이트가 404로 죽는데, 그건 릴리스 후에야 드러난다.
    #[test]
    fn 설치_스크립트_타깃_동기화() {
        let script = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/install.sh"),
        )
        .expect("scripts/install.sh 없음");
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
        ] {
            let (triple, _) = target_triple(os, arch).unwrap();
            assert!(
                script.contains(triple),
                "install.sh에 {triple} 없음 (update와 자산 이름 규칙 불일치)"
            );
        }
        // 아카이브 이름 조립 규칙도 같은 모양이어야 한다.
        assert!(
            script.contains("$BIN-$TAG-$target.tar.gz"),
            "install.sh 아카이브 이름 규칙이 asset_names와 다릅니다"
        );
    }

    #[test]
    fn 태그_검증() {
        for ok in ["v0.3.0", "0.3.0", "v10.20.30", "v0.3.0-rc1", "v0.3.0-rc.1"] {
            assert!(validate_tag(ok).is_ok(), "{ok} 는 통과해야 함");
        }
        // URL·파일명에 그대로 들어가므로 경로 탈출·질의 주입을 막아야 한다.
        for bad in [
            "../../etc/passwd",
            "v0.3.0/../x",
            "v0.3",
            "latest",
            "v0.3.0?x=1",
            "v0.3.0 rc",
            "",
        ] {
            assert!(validate_tag(bad).is_err(), "{bad:?} 는 거부해야 함");
        }
    }

    #[test]
    fn 설치_방식_판별() {
        assert_eq!(
            install_kind(Path::new("/opt/homebrew/Cellar/hwp/0.3.0/bin/hwp")),
            InstallKind::Brew
        );
        assert_eq!(
            install_kind(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/hwp/0.3.0/bin/hwp"
            )),
            InstallKind::Brew
        );
        assert_eq!(
            install_kind(Path::new("/Users/x/.cargo/bin/hwp")),
            InstallKind::Plain
        );
        assert_eq!(
            install_kind(Path::new("/usr/local/bin/hwp")),
            InstallKind::Plain
        );
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hwp-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn 바이너리_교체() {
        let target = tmp("bin_old");
        let new = tmp("bin_new");
        std::fs::write(&target, b"OLD").unwrap();
        std::fs::write(&new, b"NEW").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        replace_binary(&target, &new).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&target).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "실행 권한 승계 실패: {mode:o}");
        }
        // 임시/백업 잔재가 남으면 안 된다.
        let dir = target.parent().unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".hwp-update-") || n.starts_with(".hwp-backup-"))
            .collect();
        assert!(leftovers.is_empty(), "잔재: {leftovers:?}");
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn 바이너리_교체_실패시_원본_보존() {
        let target = tmp("bin_keep");
        std::fs::write(&target, b"OLD").unwrap();
        // 없는 파일을 새 바이너리로 주면 첫 단계에서 실패해야 하고 원본은 그대로여야 한다.
        let err = replace_binary(&target, Path::new("/nonexistent/hwp")).unwrap_err();
        assert!(format!("{err:#}").contains("놓지 못했습니다"), "{err:#}");
        assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn 체크섬_대조() {
        let f = tmp("sha_target");
        std::fs::write(&f, b"hello").unwrap();
        // sha256("hello")
        let sha = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        verify_sha256(&f, &format!("{sha}  hwp-v0.3.0-x.tar.gz")).unwrap();
        assert!(verify_sha256(&f, &format!("{}  x", "0".repeat(64))).is_err());
        assert!(verify_sha256(&f, "짧음  x").is_err());
        let _ = std::fs::remove_file(&f);
    }

    /// 릴리스 아카이브 구조(루트에 바이너리) 가정을 실제 tar로 고정한다.
    #[test]
    fn 아카이브_해제() {
        if Command::new("tar").arg("--version").output().is_err() {
            eprintln!("스킵: tar 없음");
            return;
        }
        let dir = tmp("extract").with_extension("d");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "hwp.exe" } else { "hwp" };
        std::fs::write(dir.join(name), b"#!/bin/sh\necho hwp 9.9.9\n").unwrap();
        let archive = dir.join("hwp-v9.9.9-test.tar.gz");
        let ok = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&dir)
            .arg(name)
            .status()
            .unwrap()
            .success();
        assert!(ok, "테스트 아카이브 생성 실패");

        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let exe = extract(&archive, &out).unwrap();
        assert_eq!(exe.file_name().unwrap(), name);
        assert!(std::fs::read(&exe).unwrap().starts_with(b"#!/bin/sh"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
