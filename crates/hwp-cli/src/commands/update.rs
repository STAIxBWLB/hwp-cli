//! `ahwp update`: self-update with the 1.x compatibility bridge.
//!
//! Download platform-specific GitHub release archives (`release.yml` upload-assets output)
//! and replace the resolved running executable. To avoid new runtime dependencies, delegate
//! network access to `curl` and extraction to `tar` (available on macOS/Linux/Windows 10 1803+).
//! Invoke external commands with argument vectors, without assembling shell strings.
//!
//! Delegate Homebrew installations to `brew upgrade ahwp` instead of replacing Cellar binaries.
//! Direct replacement would disagree with the manifest and be reverted by a later brew upgrade.
//!
//! Separate version/target/asset/install-kind/replacement logic from curl/tar side effects;
//! the former is tested without network access.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REPO: &str = "STAIxBWLB/hwp-cli";
const BIN: &str = "ahwp";
const LEGACY_BIN: &str = "hwp";

/// Release archive formats produced for each target by `release.yml`.
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

/// Installation kind inferred from the resolved executable path, selecting replacement policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// Inside Homebrew Cellar: delegate to brew.
    Brew,
    /// Other installations (direct download or cargo install): replace the executable directly.
    Plain,
}

/// Map (os, arch) to a published release target triple; return None for unsupported targets.
/// The canonical target matrix is upload-assets in `.github/workflows/release.yml`.
pub fn target_triple(os: &str, arch: &str) -> Option<(&'static str, Archive)> {
    match (os, arch) {
        ("macos", "aarch64") => Some(("aarch64-apple-darwin", Archive::TarGz)),
        ("macos", "x86_64") => Some(("x86_64-apple-darwin", Archive::TarGz)),
        ("linux", "x86_64") => Some(("x86_64-unknown-linux-gnu", Archive::TarGz)),
        ("linux", "aarch64") => Some(("aarch64-unknown-linux-gnu", Archive::TarGz)),
        ("windows", "x86_64") => Some(("x86_64-pc-windows-msvc", Archive::Zip)),
        _ => None,
    }
}

/// Canonical asset names matching the release workflow archive rule: $bin-$tag-$target.
/// For example, `ahwp-v1.6.0-aarch64-apple-darwin.tar.gz` and the matching `.sha256`.
pub fn asset_names(tag: &str, triple: &str, archive: Archive) -> (String, String) {
    asset_names_for(BIN, tag, triple, archive)
}

fn asset_names_for(bin: &str, tag: &str, triple: &str, archive: Archive) -> (String, String) {
    let stem = format!("{bin}-{tag}-{triple}");
    (
        format!("{stem}.{}", archive.ext()),
        format!("{stem}.sha256"),
    )
}

/// Validate the `v?X.Y.Z[-pre]` tag shape before interpolating it into URLs and file names,
/// rejecting path traversal (`../`) and query-string injection.
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

/// Parse (major, minor, patch, stable-release); invalid numeric components become zero.
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
        pre.is_none(), // Prereleases rank below a stable release with the same X.Y.Z.
    )
}

/// Whether `latest` is newer than `current`.
pub fn is_newer(current: &str, latest: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

/// Infer installation kind from the executable path. Homebrew uses links into Cellar,
/// so callers pass the canonical executable path.
pub fn install_kind(exe: &Path) -> InstallKind {
    let p = exe.to_string_lossy().replace('\\', "/");
    if p.contains("/Cellar/") || p.contains("/homebrew/") || p.contains("/linuxbrew/") {
        InstallKind::Brew
    } else {
        InstallKind::Plain
    }
}

/// Replace the executable in place, staging in the target directory for same-filesystem
/// atomic rename. Restore the backup if replacement fails.
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

    // Move the original aside before replacing it: Windows cannot overwrite a running executable,
    // and the backup also provides the recovery path on Unix.
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(target, &backup)
        .with_context(|| format!("기존 바이너리를 비켜 놓지 못했습니다: {}", target.display()))?;
    if let Err(e) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&backup, target); // Roll back.
        let _ = std::fs::remove_file(&staged);
        return Err(e).context("새 바이너리를 제자리에 놓지 못했습니다(원본 복원됨)");
    }
    // Windows may keep the running backup locked; leaving it is harmless.
    let _ = std::fs::remove_file(&backup);
    Ok(())
}

#[cfg(unix)]
fn copy_exec_mode(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    // Inherit the executable mode, falling back to 0755 if unavailable.
    let mode = std::fs::metadata(from)
        .map(|m| m.permissions().mode())
        .unwrap_or(0o755);
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("실행 권한 설정 실패: {}", to.display()))
}

#[cfg(not(unix))]
fn copy_exec_mode(_from: &Path, _to: &Path) -> Result<()> {
    Ok(()) // Windows executable recognition uses the extension.
}

/// Download one file over HTTPS/TLS 1.2+. Only curl exit 22 with HTTP 404 means absent.
fn download(url: &str, dest: &Path) -> Result<bool> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--retry",
            "2",
            "-w",
            "%{http_code}",
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|e| anyhow!("curl 실행 실패({e}) — curl이 설치돼 있어야 합니다: {url}"))?;
    download_result(
        out.status.code(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
        url,
    )
}

/// Interpret explicit curl status fields without treating transport errors as absent assets.
fn download_result(
    exit_code: Option<i32>,
    http_status: &str,
    stderr: &str,
    url: &str,
) -> Result<bool> {
    // curl's HTTP failure exit code and explicit final response status distinguish absence
    // from authentication, rate-limit, server and transport failures. Never infer it from stderr.
    if exit_code == Some(22) && http_status.trim() == "404" {
        return Ok(false);
    }
    if exit_code != Some(0) {
        bail!(
            "내려받기 실패: {url} (HTTP {})\n{}\n네트워크와 릴리스 자산 접근 권한을 확인한 뒤 다시 실행하세요",
            http_status.trim(),
            stderr.trim()
        );
    }
    Ok(true)
}

/// Fetch a UTF-8 response body for GitHub API queries.
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

/// Fetch the latest release tag (`vX.Y.Z`).
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

/// Compare a `.sha256` asset body (hash followed by file name) with the downloaded bytes.
fn verify_sha256(file: &Path, sha_line: &str) -> Result<()> {
    let want = sha_line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if want.len() != 64 {
        bail!(
            "체크섬 자산 형식이 이상합니다: {sha_line:?}. 릴리스 체크섬 자산을 확인한 뒤 다시 실행하세요"
        );
    }
    let bytes = std::fs::read(file)
        .with_context(|| format!("내려받은 파일을 읽지 못했습니다: {}", file.display()))?;
    let got: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != want {
        bail!(
            "체크섬 불일치 — 내려받기가 손상됐거나 자산이 바뀌었습니다\n  기대: {want}\n  실제: {got}\n릴리스 체크섬과 내려받은 자산을 확인한 뒤 다시 실행하세요"
        );
    }
    Ok(())
}

/// Download and verify one release through an injectable transport. `false` means the asset
/// is absent; transport failures remain errors. Only an absent canonical archive allows the
/// legacy prefix. Missing/invalid checksums and all transport failures stop the update.
fn download_release_with(
    tag: &str,
    triple: &str,
    archive: Archive,
    base: &str,
    work: &Path,
    mut transport: impl FnMut(&str, &Path) -> Result<bool>,
) -> Result<PathBuf> {
    let (mut archive_name, mut sha_name) = asset_names(tag, triple, archive);
    let mut archive_path = work.join(&archive_name);
    if !transport(&format!("{base}/{archive_name}"), &archive_path)? {
        (archive_name, sha_name) = asset_names_for(LEGACY_BIN, tag, triple, archive);
        archive_path = work.join(&archive_name);
        if !transport(&format!("{base}/{archive_name}"), &archive_path)? {
            bail!(
                "ahwp와 hwp 릴리스 아카이브가 모두 없습니다. 해당 버전의 플랫폼 자산을 확인한 뒤 다시 실행하세요"
            );
        }
    }
    let sha_path = work.join(&sha_name);
    if !transport(&format!("{base}/{sha_name}"), &sha_path)? {
        bail!("릴리스 체크섬 자산이 없습니다. 릴리스 자산을 확인한 뒤 다시 실행하세요");
    }
    let checksum = std::fs::read_to_string(&sha_path).context(
        "내려받은 체크섬을 읽지 못했습니다. 임시 디렉터리의 권한과 여유 공간을 확인한 뒤 다시 실행하세요",
    )?;
    verify_sha256(&archive_path, &checksum)?;
    Ok(archive_path)
}

/// Extract the canonical `ahwp` executable, falling back to `hwp` for older archives.
/// Use tar for both formats (bsdtar/Windows tar.exe support ZIP), expecting root-level binaries
/// as produced by `taiki-e/upload-rust-binary-action`.
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
    for bin in [BIN, LEGACY_BIN] {
        let exe = dir.join(if cfg!(windows) {
            format!("{bin}.exe")
        } else {
            bin.to_string()
        });
        if exe.is_file() {
            return Ok(exe);
        }
    }
    bail!(
        "아카이브에 ahwp 또는 hwp 실행 파일이 없습니다: {}. 해당 플랫폼의 릴리스 아카이브를 확인한 뒤 다시 실행하세요",
        archive.display()
    )
}

/// Verify that the replaced executable reports the expected version.
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

/// `ahwp update` entry point, also reached through the legacy command name.
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
                 cargo install --git https://github.com/{REPO} agent-hwp",
                std::env::consts::OS,
                std::env::consts::ARCH,
            )
        })?;
    let (archive_name, _) = asset_names(&target_tag, triple, archive);
    let base = format!("https://github.com/{REPO}/releases/download/{target_tag}");

    let work = std::env::temp_dir().join(format!("hwp-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).context("임시 작업 디렉터리 생성 실패")?;

    eprintln!("{current} → {latest} 내려받는 중… ({archive_name})");
    let archive_path = download_release_with(&target_tag, triple, archive, &base, &work, download)?;

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

    const BRIDGE_TAG: &str = "v1.6.0";
    const BRIDGE_TARGET: &str = "x86_64-unknown-linux-gnu";
    const BRIDGE_BASE: &str = "https://example.invalid/releases/v1.6.0";
    const BRIDGE_ARCHIVE_BYTES: &[u8] = b"owner-authored updater archive sentinel";

    fn bridge_names(bin: &str) -> Vec<String> {
        vec![
            format!("{bin}-{BRIDGE_TAG}-{BRIDGE_TARGET}.tar.gz"),
            format!("{bin}-{BRIDGE_TAG}-{BRIDGE_TARGET}.sha256"),
        ]
    }

    fn bridge_response(dest: &Path, corrupt: bool) {
        if dest
            .extension()
            .is_some_and(|extension| extension == "sha256")
        {
            let digest = if corrupt {
                "0".repeat(64)
            } else {
                Sha256::digest(BRIDGE_ARCHIVE_BYTES)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            };
            std::fs::write(dest, format!("{digest}  owner-authored.tar.gz")).unwrap();
        } else {
            std::fs::write(dest, BRIDGE_ARCHIVE_BYTES).unwrap();
        }
    }

    fn bridge_dir(name: &str) -> PathBuf {
        let dir = tmp(name).with_extension("d");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bridge_http_absence_requires_explicit_404_and_curl_http_failure() {
        assert!(!download_result(Some(22), "404", "", BRIDGE_BASE).unwrap());
        assert!(download_result(Some(0), "200", "", BRIDGE_BASE).unwrap());
        for (exit_code, http_status) in [
            (Some(22), "403"),
            (Some(22), "429"),
            (Some(22), "503"),
            (Some(6), "000"),
            (Some(23), "404"),
            (None, "404"),
        ] {
            let error =
                download_result(exit_code, http_status, "404 Not Found", BRIDGE_BASE).unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("내려받기 실패") && message.contains("다시 실행"),
                "failure must explain the cause and remedy: {message}"
            );
        }
    }

    #[test]
    fn bridge_asset_names_use_the_canonical_command() {
        let (archive, checksum) = asset_names(BRIDGE_TAG, BRIDGE_TARGET, Archive::TarGz);
        assert_eq!(
            vec![archive, checksum],
            bridge_names("ahwp"),
            "bridge assets use the canonical ahwp prefix"
        );
    }

    #[test]
    fn bridge_download_prefers_canonical_assets() {
        let dir = bridge_dir("bridge-preferred");
        let mut requests = Vec::new();
        let result = download_release_with(
            BRIDGE_TAG,
            BRIDGE_TARGET,
            Archive::TarGz,
            BRIDGE_BASE,
            &dir,
            |url, dest| {
                requests.push(url.rsplit('/').next().unwrap().to_string());
                bridge_response(dest, false);
                Ok(true)
            },
        );
        assert!(result.is_ok(), "fixture download must succeed: {result:?}");
        assert_eq!(
            requests,
            bridge_names("ahwp"),
            "canonical archive/checksum must win when both names are available"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bridge_download_falls_back_only_when_canonical_archive_is_absent() {
        let dir = bridge_dir("bridge-fallback");
        let mut requests = Vec::new();
        let result = download_release_with(
            BRIDGE_TAG,
            BRIDGE_TARGET,
            Archive::TarGz,
            BRIDGE_BASE,
            &dir,
            |url, dest| {
                let name = url.rsplit('/').next().unwrap().to_string();
                let absent = name.starts_with("ahwp-");
                requests.push(name);
                if absent {
                    return Ok(false);
                }
                bridge_response(dest, false);
                Ok(true)
            },
        );
        assert!(
            result.is_ok(),
            "legacy-only release must remain installable: {result:?}"
        );
        let mut expected = vec![bridge_names("ahwp")[0].clone()];
        expected.extend(bridge_names("hwp"));
        assert_eq!(
            requests, expected,
            "legacy is tried after the absent canonical archive"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bridge_download_does_not_fallback_on_transport_failure() {
        let dir = bridge_dir("bridge-network-failure");
        let mut requests = Vec::new();
        let result = download_release_with(
            BRIDGE_TAG,
            BRIDGE_TARGET,
            Archive::TarGz,
            BRIDGE_BASE,
            &dir,
            |url, dest| {
                let name = url.rsplit('/').next().unwrap().to_string();
                let canonical = name.starts_with("ahwp-");
                requests.push(name);
                if canonical {
                    bail!("owner-authored transport failure");
                }
                bridge_response(dest, false);
                Ok(true)
            },
        );
        assert!(
            result.is_err(),
            "transport failure must not install the available legacy asset"
        );
        assert_eq!(requests, vec![bridge_names("ahwp")[0].clone()]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bridge_download_does_not_fallback_on_missing_checksum() {
        let dir = bridge_dir("bridge-missing-checksum");
        let mut requests = Vec::new();
        let result = download_release_with(
            BRIDGE_TAG,
            BRIDGE_TARGET,
            Archive::TarGz,
            BRIDGE_BASE,
            &dir,
            |url, dest| {
                let name = url.rsplit('/').next().unwrap().to_string();
                let absent = name.starts_with("ahwp-") && name.ends_with(".sha256");
                requests.push(name);
                if absent {
                    return Ok(false);
                }
                bridge_response(dest, false);
                Ok(true)
            },
        );
        assert!(
            result.is_err(),
            "missing canonical checksum must not install the available legacy asset"
        );
        assert_eq!(requests, bridge_names("ahwp"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bridge_download_does_not_fallback_on_checksum_mismatch() {
        let dir = bridge_dir("bridge-bad-checksum");
        let mut requests = Vec::new();
        let result = download_release_with(
            BRIDGE_TAG,
            BRIDGE_TARGET,
            Archive::TarGz,
            BRIDGE_BASE,
            &dir,
            |url, dest| {
                let name = url.rsplit('/').next().unwrap().to_string();
                let corrupt = name.starts_with("ahwp-") && name.ends_with(".sha256");
                requests.push(name);
                bridge_response(dest, corrupt);
                Ok(true)
            },
        );
        assert!(
            result.is_err(),
            "canonical checksum mismatch must not install the available legacy asset"
        );
        assert_eq!(requests, bridge_names("ahwp"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn bridge_extract_fixture(label: &str, canonical: bool, legacy: bool) {
        let dir = bridge_dir(label);
        let canonical_name = if cfg!(windows) { "ahwp.exe" } else { "ahwp" };
        let legacy_name = if cfg!(windows) { "hwp.exe" } else { "hwp" };
        let mut names = Vec::new();
        if canonical {
            std::fs::write(dir.join(canonical_name), b"CANONICAL").unwrap();
            names.push(canonical_name);
        }
        if legacy {
            std::fs::write(dir.join(legacy_name), b"LEGACY").unwrap();
            names.push(legacy_name);
        }
        let archive = dir.join("bridge.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&dir)
            .args(names)
            .status()
            .expect("tar is required for the updater's archive contract");
        assert!(status.success(), "synthetic archive setup must succeed");
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let result = extract(&archive, &out);
        assert!(
            result.is_ok(),
            "bridge archive must yield an executable: {result:?}"
        );
        let extracted = result.unwrap();
        let expected_name = if canonical {
            canonical_name
        } else {
            legacy_name
        };
        assert_eq!(
            extracted.file_name().unwrap(),
            expected_name,
            "new archives prefer ahwp; older hwp-only archives remain supported"
        );
        let expected: &[u8] = if canonical { b"CANONICAL" } else { b"LEGACY" };
        assert_eq!(std::fs::read(extracted).unwrap(), expected);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bridge_extract_accepts_canonical_only_archive() {
        bridge_extract_fixture("bridge-extract-new", true, false);
    }

    #[test]
    fn bridge_extract_prefers_canonical_when_both_names_exist() {
        bridge_extract_fixture("bridge-extract-both", true, true);
    }

    #[test]
    fn bridge_extract_keeps_legacy_only_archive_compatibility() {
        bridge_extract_fixture("bridge-extract-old", false, true);
    }

    #[test]
    fn 버전_비교() {
        assert!(is_newer("0.3.0", "0.4.0"));
        assert!(is_newer("0.3.0", "0.3.1"));
        assert!(is_newer("0.9.0", "1.0.0"));
        assert!(is_newer("0.3.0", "v0.4.0")); // Accept the v prefix.
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.4.0", "0.3.9"));
        // Prereleases rank below stable versions; do not mistake an RC for an upgrade.
        assert!(is_newer("0.3.0-rc1", "0.3.0"));
        assert!(!is_newer("0.3.0", "0.3.0-rc1"));
        // Lexical ordering would misorder multi-digit components ("10" < "9").
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
        // Unpublished target combinations must direct users to source installation.
        assert_eq!(
            target_triple("linux", "aarch64"),
            Some(("aarch64-unknown-linux-gnu", Archive::TarGz))
        );
        assert_eq!(target_triple("freebsd", "x86_64"), None);
    }

    /// Canonical names use ahwp for any tag; older hwp assets use the explicit fallback.
    #[test]
    fn 자산_이름() {
        let (a, s) = asset_names("v0.3.0", "aarch64-apple-darwin", Archive::TarGz);
        assert_eq!(a, "ahwp-v0.3.0-aarch64-apple-darwin.tar.gz");
        assert_eq!(s, "ahwp-v0.3.0-aarch64-apple-darwin.sha256");
        let (a, _) = asset_names("v0.3.0", "x86_64-pc-windows-msvc", Archive::Zip);
        assert_eq!(a, "ahwp-v0.3.0-x86_64-pc-windows-msvc.zip");
    }

    /// Installer and updater must share the same target/name rule; drift causes download failures
    /// that otherwise appear only after publication.
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
        // The archive-name construction must also match.
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
        // Reject traversal/query injection before tags enter URLs and file names.
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
        // No staged/backup files should remain.
        let dir = target.parent().unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".ahwp-update-") || n.starts_with(".ahwp-backup-"))
            .collect();
        assert!(leftovers.is_empty(), "잔재: {leftovers:?}");
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn 바이너리_교체_실패시_원본_보존() {
        let target = tmp("bin_keep");
        std::fs::write(&target, b"OLD").unwrap();
        // A missing replacement fails at staging and leaves the original untouched.
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

    /// Use a real tar archive to verify the root-level executable layout.
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
