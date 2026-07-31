//! CLI 파일 출력의 공통 트랜잭션 계층.
//!
//! writer는 최종 경로와 같은 디렉터리의 비공개 작업공간에 먼저 쓴다. 쓰기와 검증이
//! 모두 끝난 뒤에만 최종 경로를 교체하므로, 실패한 명령은 기존 출력을 훼손하지 않는다.

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use anyhow::Context as _;
use sha2::{Digest as _, Sha256};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// `destination` 옆에 실제 확장자를 유지한 임시 파일을 쓰고 검증한 뒤 게시한다.
///
/// `input`은 입력과 출력의 안전한 제자리 사용 여부 및 별칭을 검사하는 데만 쓴다.
/// 같은 일반 파일의 직접 경로(`file`, `./file`, 절대경로)는 허용하지만, 심볼릭
/// 링크나 하드 링크를 통한 별칭은 어느 디렉터리 항목을 교체해야 하는지 모호하므로 거부한다.
pub(crate) fn write_validated<T>(
    destination: &Path,
    input: Option<&Path>,
    writer: impl FnOnce(&Path) -> anyhow::Result<T>,
    verifier: impl FnOnce(&Path, &T) -> anyhow::Result<()>,
) -> anyhow::Result<T> {
    let mut staged = StagedOutput::new(destination, input)?;
    let value = writer(staged.path())?;
    staged.sync_file()?;
    verifier(staged.path(), &value)
        .with_context(|| format!("출력 검증 실패 (대상: {})", destination.display()))?;
    // verifier는 읽기 전용이라는 계약이지만, 게시 직전 fsync를 한 번 더 수행해 그
    // 계약이 실수로 깨져도 rename 전에 모든 바이트가 동기화되도록 한다.
    staged.sync_file()?;
    if let Some(warning) = staged.publish()? {
        eprintln!("경고: {warning}");
    }
    Ok(value)
}

/// Run the same private sibling staging and verification path as
/// [`write_validated`] but discard the verified stage instead of publishing it.
/// Used by dry-run commands that must prove the real writer/package path.
pub(crate) fn validate_without_publish<T>(
    destination: &Path,
    input: Option<&Path>,
    writer: impl FnOnce(&Path) -> anyhow::Result<T>,
    verifier: impl FnOnce(&Path, &T) -> anyhow::Result<()>,
) -> anyhow::Result<T> {
    let staged = StagedOutput::new(destination, input)?;
    let value = writer(staged.path())?;
    staged.sync_file()?;
    verifier(staged.path(), &value)
        .with_context(|| format!("dry-run 출력 검증 실패 (대상: {})", destination.display()))?;
    staged.sync_file()?;
    Ok(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SnapshotOutputMode {
    PlanOnly,
    ValidateOnly,
    Publish,
}

/// Copy an immutable command input into the same private transaction that owns
/// the final destination snapshot. `writer` sees only the copied input and the
/// private output path; hash, validation, and publish therefore share one
/// source snapshot and one command-start destination baseline.
pub(crate) fn write_with_private_input_snapshot<T>(
    destination: &Path,
    input: &Path,
    max_bytes: u64,
    mode: SnapshotOutputMode,
    writer: impl FnOnce(&Path, &Path, &str) -> anyhow::Result<T>,
    verifier: impl FnOnce(&Path, &T) -> anyhow::Result<()>,
) -> anyhow::Result<(String, T)> {
    let mut staged = StagedOutput::new(destination, Some(input))?;
    let mut source = fs::File::open(input)
        .with_context(|| format!("입력 snapshot을 열 수 없습니다: {}", input.display()))?;
    let opened = source
        .metadata()
        .with_context(|| format!("열린 입력 상태를 확인할 수 없습니다: {}", input.display()))?;
    let path_before = fs::symlink_metadata(input)
        .with_context(|| format!("입력 경로를 확인할 수 없습니다: {}", input.display()))?;
    if path_before.file_type().is_symlink() || !path_before.file_type().is_file() {
        anyhow::bail!(
            "입력 snapshot은 심볼릭 링크가 아닌 일반 파일만 허용합니다: {}",
            input.display()
        );
    }
    let opened_identity = FileIdentity::from_metadata(&opened, input)?;
    let path_identity = FileIdentity::from_metadata(&path_before, input)?;
    let opened_modified = opened
        .modified()
        .with_context(|| format!("입력 수정 시각을 확인할 수 없습니다: {}", input.display()))?;
    if opened_identity != path_identity
        || opened.len() != path_before.len()
        || opened_modified
            != path_before.modified().with_context(|| {
                format!("입력 수정 시각을 확인할 수 없습니다: {}", input.display())
            })?
    {
        anyhow::bail!(
            "입력 경로가 snapshot 준비 중 바뀌었습니다: {}",
            input.display()
        );
    }
    if opened.len() > max_bytes {
        anyhow::bail!(
            "입력 snapshot 크기가 제한을 초과합니다: {} > {} bytes",
            opened.len(),
            max_bytes
        );
    }

    let snapshot_path = staged.workspace.join("input.snapshot");
    let mut snapshot = fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&snapshot_path)
        .with_context(|| {
            format!(
                "비공개 입력 snapshot을 만들 수 없습니다: {}",
                snapshot_path.display()
            )
        })?;
    let mut hasher = Sha256::new();
    let mut copied = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = source
            .read(&mut buffer)
            .with_context(|| format!("입력 snapshot을 읽을 수 없습니다: {}", input.display()))?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .context("입력 snapshot 크기 overflow")?;
        if copied > max_bytes || copied > opened.len() {
            anyhow::bail!("입력 snapshot 중 파일 크기가 변경되었거나 제한을 초과했습니다");
        }
        snapshot.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
    }
    if copied != opened.len() {
        anyhow::bail!(
            "입력 snapshot 중 파일 크기가 변경되었습니다: {copied} != {} bytes",
            opened.len()
        );
    }

    let opened_after = source.metadata()?;
    let path_after = fs::symlink_metadata(input)?;
    if path_after.file_type().is_symlink()
        || !path_after.file_type().is_file()
        || FileIdentity::from_metadata(&opened_after, input)? != opened_identity
        || FileIdentity::from_metadata(&path_after, input)? != opened_identity
        || opened_after.len() != opened.len()
        || path_after.len() != opened.len()
        || opened_after.modified()? != opened_modified
        || path_after.modified()? != opened_modified
    {
        anyhow::bail!(
            "입력 경로 또는 내용이 snapshot 중 바뀌었습니다: {}",
            input.display()
        );
    }
    snapshot.flush()?;
    snapshot.sync_all()?;
    drop(snapshot);

    let hash = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let value = writer(&snapshot_path, staged.path(), &hash)?;
    if mode != SnapshotOutputMode::PlanOnly {
        staged.sync_file()?;
        verifier(staged.path(), &value).with_context(|| {
            format!(
                "snapshot 기반 출력 검증 실패 (대상: {})",
                destination.display()
            )
        })?;
        staged.sync_file()?;
    }
    if mode == SnapshotOutputMode::Publish
        && let Some(warning) = staged.publish()?
    {
        eprintln!("경고: {warning}");
    }
    Ok((hash, value))
}

/// Reject an output that aliases any immutable command input. Unlike the
/// single-source edit path, exact in-place use is not permitted.
pub(crate) fn reject_output_aliases(destination: &Path, inputs: &[&Path]) -> anyhow::Result<()> {
    let snapshot = inspect_destination(destination)?;
    for input in inputs {
        if *input == destination {
            anyhow::bail!(
                "출력이 immutable 입력과 같아 게시할 수 없습니다: {}",
                destination.display()
            );
        }
        reject_unsafe_alias(Some(input), destination, &snapshot)?;
    }
    Ok(())
}

/// 여러 파일을 모두 준비·검증한 뒤 하나의 복구 가능한 게시 단위로 교체한다.
///
/// PNG/SVG 다중 페이지처럼 결과 집합의 일부만 새 버전으로 보이면 안 되는 호출에 쓴다.
/// 모든 destination을 게시 직전 재확인하고 기존 파일을 먼저 backup한 뒤 새 파일을
/// 게시한다. 후반 게시 실패 시 앞서 게시한 새 파일을 제거하고 모든 기존 파일을 복원한다.
pub(crate) fn write_validated_files(
    outputs: &[(PathBuf, Vec<u8>)],
    input: Option<&Path>,
) -> anyhow::Result<Option<String>> {
    if outputs.is_empty() {
        anyhow::bail!("게시할 출력 파일이 없습니다");
    }
    let mut normalized = Vec::with_capacity(outputs.len());
    for (destination, _) in outputs {
        let absolute = std::path::absolute(destination).with_context(|| {
            format!(
                "출력 경로를 절대경로로 바꿀 수 없습니다: {}",
                destination.display()
            )
        })?;
        if normalized.contains(&absolute) {
            anyhow::bail!(
                "출력 집합에 중복 경로가 있습니다: {}",
                destination.display()
            );
        }
        normalized.push(absolute);
    }

    let mut staged = outputs
        .iter()
        .map(|(destination, _)| StagedOutput::new(destination, input))
        .collect::<anyhow::Result<Vec<_>>>()?;
    for ((_, bytes), output) in outputs.iter().zip(&staged) {
        fs::write(output.path(), bytes).with_context(|| {
            format!(
                "임시 렌더 출력을 쓸 수 없습니다: {}",
                output.path().display()
            )
        })?;
        output.sync_file()?;
        let written = fs::read(output.path()).with_context(|| {
            format!(
                "임시 렌더 출력을 검증할 수 없습니다: {}",
                output.path().display()
            )
        })?;
        if written != *bytes {
            anyhow::bail!(
                "렌더 출력 검증 중 바이트 불일치: {}",
                output.path().display()
            );
        }
    }
    publish_output_set(&mut staged, |_| Ok(()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SetPublishStep {
    Publish(usize),
    Restore(usize),
}

fn publish_output_set(
    outputs: &mut [StagedOutput],
    mut hook: impl FnMut(SetPublishStep) -> anyhow::Result<()>,
) -> anyhow::Result<Option<String>> {
    // Mutation을 시작하기 전에 집합 전체의 경쟁 여부와 staged 파일 상태를 확정한다.
    for output in outputs.iter() {
        recheck_destination(&output.destination, &output.snapshot)?;
        output.sync_file()?;
    }

    #[cfg(unix)]
    for output in outputs.iter() {
        if let DestinationSnapshot::Regular { permissions, .. } = &output.snapshot {
            fs::set_permissions(&output.staged, permissions.clone()).with_context(|| {
                format!(
                    "기존 출력 파일의 권한을 승계하지 못했습니다: {}",
                    output.destination.display()
                )
            })?;
        }
    }

    #[cfg(windows)]
    for output in outputs.iter() {
        match output.snapshot {
            DestinationSnapshot::Regular { .. } => {
                windows_copy_dacl(&output.destination, &output.staged)?;
            }
            DestinationSnapshot::Missing => {
                windows_apply_parent_default_dacl(&output.staged, &output.destination, false)?;
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    for output in outputs.iter() {
        if let DestinationSnapshot::Regular { permissions, .. } = &output.snapshot {
            fs::set_permissions(&output.staged, permissions.clone()).with_context(|| {
                format!(
                    "기존 출력 파일의 권한을 승계하지 못했습니다: {}",
                    output.destination.display()
                )
            })?;
        }
    }

    let backups = outputs
        .iter()
        .map(|output| output.workspace.join("destination.backup"))
        .collect::<Vec<_>>();
    let mut backed_up = vec![false; outputs.len()];
    let mut published = vec![false; outputs.len()];

    let result = (|| -> anyhow::Result<()> {
        for (index, output) in outputs.iter().enumerate() {
            if matches!(output.snapshot, DestinationSnapshot::Regular { .. }) {
                fs::rename(&output.destination, &backups[index]).with_context(|| {
                    format!(
                        "기존 출력 집합 항목을 백업하지 못했습니다: {}",
                        output.destination.display()
                    )
                })?;
                backed_up[index] = true;
            }
        }
        for (index, output) in outputs.iter().enumerate() {
            hook(SetPublishStep::Publish(index))?;
            fs::rename(&output.staged, &output.destination).with_context(|| {
                format!(
                    "검증된 출력 집합 항목을 게시하지 못했습니다: {}",
                    output.destination.display()
                )
            })?;
            published[index] = true;
        }
        Ok(())
    })();

    if let Err(error) = result {
        let mut recovery_errors = Vec::new();
        for index in (0..outputs.len()).rev() {
            let output = &mut outputs[index];
            if published[index]
                && let Err(remove_error) = fs::remove_file(&output.destination)
            {
                recovery_errors.push(format!(
                    "새 출력 제거 실패 {}: {remove_error}",
                    output.destination.display()
                ));
            }
            if backed_up[index] {
                let restore = hook(SetPublishStep::Restore(index)).and_then(|()| {
                    fs::rename(&backups[index], &output.destination).with_context(|| {
                        format!(
                            "기존 출력 집합 항목 복원 실패: {}",
                            output.destination.display()
                        )
                    })
                });
                if let Err(restore_error) = restore {
                    output.preserve_workspace = true;
                    recovery_errors.push(format!("{restore_error:#}"));
                }
            }
        }
        if recovery_errors.is_empty() {
            return Err(error).context("출력 집합 게시 실패 후 기존 파일을 모두 복원했습니다");
        }
        anyhow::bail!(
            "{error:#}; 출력 집합 복구가 완전하지 않아 backup을 보존했습니다: {}",
            recovery_errors.join("; ")
        );
    }

    let mut warnings = Vec::new();
    for (index, output) in outputs.iter_mut().enumerate() {
        if backed_up[index]
            && let Err(error) = fs::remove_file(&backups[index])
        {
            output.preserve_workspace = true;
            warnings.push(format!(
                "새 출력은 게시했지만 기존 파일 backup을 정리하지 못했습니다: {} ({error})",
                backups[index].display()
            ));
        }
        if let Err(error) = sync_parent_directory(&output.destination) {
            warnings.push(format!("{error:#}"));
        }
        let _ = fs::remove_dir(&output.workspace);
    }
    Ok((!warnings.is_empty()).then(|| warnings.join("; ")))
}

/// Markdown 본문과 이미지 sidecar 디렉터리를 하나의 복구 가능한 게시 단위로 쓴다.
///
/// sidecar는 최종 디렉터리와 같은 파일시스템의 비공개 작업공간에서 완성한다. 기존
/// sidecar가 있으면 작업공간으로 먼저 복제하므로 exporter의 기존 충돌 계약(동일
/// 바이트만 재사용, 다른 바이트는 거부)과 관계없는 파일 보존이 그대로 유지된다.
/// 검증 후 본문과 sidecar 양쪽 destination을 다시 확인하고, 기존 항목을 backup으로
/// 옮긴 뒤 게시한다. 두 번째 게시가 실패하면 이미 게시한 첫 번째 항목까지 되돌린다.
pub(crate) fn write_validated_with_sidecar<T>(
    destination: &Path,
    input: Option<&Path>,
    sidecar_destination: &Path,
    writer: impl FnOnce(&Path, &Path) -> anyhow::Result<T>,
    verifier: impl FnOnce(&Path, &Path, &T) -> anyhow::Result<()>,
) -> anyhow::Result<T> {
    reject_overlapping_outputs(destination, sidecar_destination, input)?;
    let mut staged = StagedOutputPair::new(destination, input, sidecar_destination)?;
    let value = writer(staged.file.path(), staged.sidecar.path())?;
    staged.file.sync_file()?;
    staged.sidecar.finish_write()?;
    verifier(staged.file.path(), staged.sidecar.path(), &value)
        .with_context(|| format!("출력 검증 실패 (대상: {})", destination.display()))?;
    staged.file.sync_file()?;
    staged.sidecar.finish_write()?;
    if let Some(warning) = staged.publish()? {
        eprintln!("경고: {warning}");
    }
    Ok(value)
}

#[derive(Clone)]
enum DestinationSnapshot {
    Missing,
    Regular {
        identity: FileIdentity,
        content: FileContentState,
        permission_state: PermissionState,
        #[cfg(not(windows))]
        permissions: fs::Permissions,
        #[cfg(windows)]
        dacl: Vec<u8>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileContentState {
    len: u64,
    modified: SystemTime,
    sha256: [u8; 32],
}

struct StagedOutput {
    destination: PathBuf,
    workspace: PathBuf,
    staged: PathBuf,
    snapshot: DestinationSnapshot,
    preserve_workspace: bool,
}

impl StagedOutput {
    fn new(destination: &Path, input: Option<&Path>) -> anyhow::Result<Self> {
        let snapshot = inspect_destination(destination)?;
        reject_unsafe_alias(input, destination, &snapshot)?;

        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file_name = destination.file_name().with_context(|| {
            format!(
                "출력 파일 이름을 확인할 수 없습니다: {}",
                destination.display()
            )
        })?;
        let display_name = file_name.to_string_lossy();

        for _ in 0..1024 {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let random = random_token()?;
            let workspace = parent.join(format!(
                ".{display_name}.hwp-output-{}-{sequence}-{random}.tmp",
                std::process::id()
            ));
            match create_private_workspace(&workspace) {
                Ok(()) => {
                    let staged = workspace.join(file_name);
                    return Ok(Self {
                        destination: destination.to_path_buf(),
                        workspace,
                        staged,
                        snapshot,
                        preserve_workspace: false,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!(
                            "출력 파일 옆에 임시 작업공간을 만들 수 없습니다: {}",
                            workspace.display()
                        )
                    });
                }
            }
        }
        anyhow::bail!(
            "충돌하지 않는 임시 작업공간을 만들 수 없습니다: {}",
            parent.display()
        )
    }

    fn path(&self) -> &Path {
        &self.staged
    }

    fn sync_file(&self) -> anyhow::Result<()> {
        let metadata = fs::symlink_metadata(&self.staged).with_context(|| {
            format!(
                "임시 출력 파일을 확인할 수 없습니다: {}",
                self.staged.display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            anyhow::bail!(
                "writer가 일반 파일이 아닌 임시 출력을 만들었습니다: {}",
                self.staged.display()
            );
        }
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.staged)
            .with_context(|| format!("임시 출력 파일을 열 수 없습니다: {}", self.staged.display()))?
            .sync_all()
            .with_context(|| format!("임시 출력 동기화 실패: {}", self.staged.display()))
    }

    // Windows에서는 아래 cfg(windows) 블록이 함수의 마지막이라 조기 return이 tail로 보인다.
    // 다른 플랫폼에서는 뒤에 cfg 블록이 더 있어 return이 필요하다.
    #[cfg_attr(windows, allow(clippy::needless_return))]
    fn publish(&mut self) -> anyhow::Result<Option<String>> {
        recheck_destination(&self.destination, &self.snapshot)?;

        #[cfg(unix)]
        if let DestinationSnapshot::Regular { permissions, .. } = &self.snapshot {
            fs::set_permissions(&self.staged, permissions.clone()).with_context(|| {
                format!(
                    "기존 출력 파일의 권한을 승계하지 못했습니다: {}",
                    self.destination.display()
                )
            })?;
        }

        #[cfg(unix)]
        fs::rename(&self.staged, &self.destination).with_context(|| {
            format!(
                "검증된 임시 출력을 최종 경로에 게시하지 못했습니다: {}",
                self.destination.display()
            )
        })?;

        #[cfg(windows)]
        {
            let warning = match self.snapshot {
                DestinationSnapshot::Regular { .. } => {
                    match windows_replace_preserving_acl(
                        &self.staged,
                        &self.destination,
                        &self.workspace.join("destination.backup"),
                    ) {
                        RecoveryState::Published { warning } => {
                            if warning.is_some() {
                                // 백업 파일을 직접 지우지 못했다면 Drop의 재귀 정리로
                                // 삭제하지 않는다. 경고에 표시한 복구 사본을 남긴다.
                                self.preserve_workspace = true;
                            }
                            warning
                        }
                        RecoveryState::FailedRestored { error } => return Err(error),
                        RecoveryState::FailedBackupPreserved { error, backup } => {
                            self.preserve_workspace = true;
                            return Err(error).with_context(|| {
                                format!("원본 백업을 보존했습니다: {}", backup.display())
                            });
                        }
                    }
                }
                DestinationSnapshot::Missing => {
                    windows_apply_parent_default_dacl(&self.staged, &self.destination, false)?;
                    fs::rename(&self.staged, &self.destination).with_context(|| {
                        format!(
                            "검증된 임시 출력을 최종 경로에 게시하지 못했습니다: {}",
                            self.destination.display()
                        )
                    })?;
                    None
                }
            };
            let _ = fs::remove_dir(&self.workspace);
            return Ok(warning);
        }

        #[cfg(not(any(unix, windows)))]
        {
            if let DestinationSnapshot::Regular { permissions, .. } = &self.snapshot {
                fs::set_permissions(&self.staged, permissions.clone()).with_context(|| {
                    format!(
                        "기존 출력 파일의 권한을 승계하지 못했습니다: {}",
                        self.destination.display()
                    )
                })?;
            }
            match publish_with_recovery(
                &RealRecoveryFs,
                &self.staged,
                &self.destination,
                &self.workspace.join("destination.backup"),
            ) {
                RecoveryState::Published { warning } => {
                    if warning.is_some() {
                        self.preserve_workspace = true;
                    }
                    let _ = fs::remove_dir(&self.workspace);
                    return Ok(warning);
                }
                RecoveryState::FailedRestored { error } => return Err(error),
                RecoveryState::FailedBackupPreserved { error, backup } => {
                    self.preserve_workspace = true;
                    return Err(error).with_context(|| {
                        format!("원본 백업을 보존했습니다: {}", backup.display())
                    });
                }
            }
        }

        #[cfg(unix)]
        {
            // rename 성공 뒤 sync 실패를 명령 실패로 바꾸면 "실패했지만 목적지는 변경됨"이라는
            // 더 위험한 계약이 된다. 게시 성공을 유지하고 호출자에게 경고만 노출한다.
            let warning = sync_parent_directory(&self.destination)
                .err()
                .map(|e| format!("{e:#}"));
            let _ = fs::remove_dir(&self.workspace);
            Ok(warning)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TreeEntryKind {
    Directory,
    File {
        len: u64,
        modified: SystemTime,
        sha256: [u8; 32],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TreeEntryState {
    relative: PathBuf,
    kind: TreeEntryKind,
    permissions: PermissionState,
    #[cfg(windows)]
    dacl: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TreeState {
    entries: Vec<TreeEntryState>,
}

impl TreeState {
    /// 복제 과정에서 수정 시각이 달라져도 실제 출력 내용/권한이 같으면 변경으로 보지 않는다.
    /// 원본 destination 재검사에서는 `Eq`를 사용해 수정 시각 변화까지 경쟁으로 감지한다.
    fn equivalent_output(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(&other.entries)
                .all(|(left, right)| {
                    left.relative == right.relative
                        && left.permissions == right.permissions
                        && match (&left.kind, &right.kind) {
                            (TreeEntryKind::Directory, TreeEntryKind::Directory) => true,
                            (
                                TreeEntryKind::File {
                                    len: left_len,
                                    sha256: left_sha256,
                                    ..
                                },
                                TreeEntryKind::File {
                                    len: right_len,
                                    sha256: right_sha256,
                                    ..
                                },
                            ) => left_len == right_len && left_sha256 == right_sha256,
                            _ => false,
                        }
                })
    }
}

#[derive(Clone)]
enum DirectorySnapshot {
    Missing,
    Directory {
        identity: FileIdentity,
        permission_state: PermissionState,
        #[cfg(not(windows))]
        permissions: fs::Permissions,
        #[cfg(windows)]
        dacl: Vec<u8>,
        tree: TreeState,
    },
}

struct StagedDirectory {
    destination: PathBuf,
    workspace: PathBuf,
    staged: PathBuf,
    snapshot: DirectorySnapshot,
    changed: bool,
    preserve_workspace: bool,
}

impl StagedDirectory {
    fn new(destination: &Path) -> anyhow::Result<Self> {
        let snapshot = inspect_directory_destination(destination)?;
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let display_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("media");
        let workspace = create_unique_workspace(parent, display_name, "hwp-media")?;
        let staged = workspace.join("tree");
        if matches!(snapshot, DirectorySnapshot::Directory { .. }) {
            copy_directory_tree(destination, &staged).with_context(|| {
                format!(
                    "기존 미디어 디렉터리를 임시 작업공간으로 복제하지 못했습니다: {}",
                    destination.display()
                )
            })?;
            recheck_directory_destination(destination, &snapshot)?;
        }
        Ok(Self {
            destination: destination.to_path_buf(),
            workspace,
            staged,
            snapshot,
            changed: false,
            preserve_workspace: false,
        })
    }

    fn path(&self) -> &Path {
        &self.staged
    }

    fn finish_write(&mut self) -> anyhow::Result<()> {
        match fs::symlink_metadata(&self.staged) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                let tree = inspect_tree(&self.staged)?;
                self.changed = match &self.snapshot {
                    DirectorySnapshot::Missing => true,
                    DirectorySnapshot::Directory { tree: original, .. } => {
                        !original.equivalent_output(&tree)
                    }
                };
                if self.changed {
                    sync_directory_tree(&self.staged)?;
                }
                Ok(())
            }
            Ok(_) => anyhow::bail!(
                "writer가 디렉터리가 아닌 미디어 sidecar를 만들었습니다: {}",
                self.staged.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.changed = false;
                Ok(())
            }
            Err(error) => Err(error).with_context(|| {
                format!(
                    "임시 미디어 sidecar를 확인할 수 없습니다: {}",
                    self.staged.display()
                )
            }),
        }
    }
}

impl Drop for StagedDirectory {
    fn drop(&mut self) {
        if !self.preserve_workspace {
            let _ = fs::remove_dir_all(&self.workspace);
        }
    }
}

struct StagedOutputPair {
    file: StagedOutput,
    sidecar: StagedDirectory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PairPublishStep {
    PublishSidecar,
    PublishFile,
    RestoreSidecar,
    RestoreFile,
}

impl StagedOutputPair {
    fn new(
        destination: &Path,
        input: Option<&Path>,
        sidecar_destination: &Path,
    ) -> anyhow::Result<Self> {
        let file = StagedOutput::new(destination, input)?;
        let sidecar = StagedDirectory::new(sidecar_destination)?;
        Ok(Self { file, sidecar })
    }

    fn publish(&mut self) -> anyhow::Result<Option<String>> {
        self.publish_with_hook(|_| Ok(()))
    }

    fn publish_with_hook(
        &mut self,
        mut hook: impl FnMut(PairPublishStep) -> anyhow::Result<()>,
    ) -> anyhow::Result<Option<String>> {
        if !self.sidecar.changed {
            return self.file.publish();
        }

        recheck_destination(&self.file.destination, &self.file.snapshot)?;
        recheck_directory_destination(&self.sidecar.destination, &self.sidecar.snapshot)?;

        #[cfg(unix)]
        if let DestinationSnapshot::Regular { permissions, .. } = &self.file.snapshot {
            fs::set_permissions(&self.file.staged, permissions.clone()).with_context(|| {
                format!(
                    "기존 출력 파일의 권한을 승계하지 못했습니다: {}",
                    self.file.destination.display()
                )
            })?;
        }

        #[cfg(windows)]
        {
            match self.file.snapshot {
                DestinationSnapshot::Regular { .. } => {
                    windows_copy_dacl(&self.file.destination, &self.file.staged)?;
                }
                DestinationSnapshot::Missing => {
                    windows_apply_parent_default_dacl(
                        &self.file.staged,
                        &self.file.destination,
                        false,
                    )?;
                }
            }
            windows_prepare_staged_tree(
                &self.sidecar.staged,
                &self.sidecar.destination,
                &self.sidecar.snapshot,
            )?;
        }

        #[cfg(not(any(unix, windows)))]
        {
            if let DestinationSnapshot::Regular { permissions, .. } = &self.file.snapshot {
                fs::set_permissions(&self.file.staged, permissions.clone()).with_context(|| {
                    format!(
                        "기존 출력 파일의 권한을 승계하지 못했습니다: {}",
                        self.file.destination.display()
                    )
                })?;
            }
            if let DirectorySnapshot::Directory { permissions, .. } = &self.sidecar.snapshot {
                fs::set_permissions(&self.sidecar.staged, permissions.clone()).with_context(
                    || {
                        format!(
                            "기존 미디어 디렉터리의 권한을 승계하지 못했습니다: {}",
                            self.sidecar.destination.display()
                        )
                    },
                )?;
            }
        }
        #[cfg(unix)]
        if let DirectorySnapshot::Directory { permissions, .. } = &self.sidecar.snapshot {
            fs::set_permissions(&self.sidecar.staged, permissions.clone()).with_context(|| {
                format!(
                    "기존 미디어 디렉터리의 권한을 승계하지 못했습니다: {}",
                    self.sidecar.destination.display()
                )
            })?;
        }

        let file_backup = self.file.workspace.join("destination.backup");
        let sidecar_backup = self.sidecar.workspace.join("destination.backup");
        let mut file_backed_up = false;
        let mut sidecar_backed_up = false;
        let mut file_published = false;
        let mut sidecar_published = false;

        let result = (|| -> anyhow::Result<()> {
            if matches!(self.sidecar.snapshot, DirectorySnapshot::Directory { .. }) {
                fs::rename(&self.sidecar.destination, &sidecar_backup).with_context(|| {
                    format!(
                        "기존 미디어 디렉터리를 안전하게 백업하지 못했습니다: {}",
                        self.sidecar.destination.display()
                    )
                })?;
                sidecar_backed_up = true;
            }
            if matches!(self.file.snapshot, DestinationSnapshot::Regular { .. }) {
                fs::rename(&self.file.destination, &file_backup).with_context(|| {
                    format!(
                        "기존 출력 파일을 안전하게 백업하지 못했습니다: {}",
                        self.file.destination.display()
                    )
                })?;
                file_backed_up = true;
            }

            hook(PairPublishStep::PublishSidecar)?;
            fs::rename(&self.sidecar.staged, &self.sidecar.destination).with_context(|| {
                format!(
                    "검증된 미디어 sidecar를 최종 경로에 게시하지 못했습니다: {}",
                    self.sidecar.destination.display()
                )
            })?;
            sidecar_published = true;

            hook(PairPublishStep::PublishFile)?;
            fs::rename(&self.file.staged, &self.file.destination).with_context(|| {
                format!(
                    "검증된 임시 출력을 최종 경로에 게시하지 못했습니다: {}",
                    self.file.destination.display()
                )
            })?;
            file_published = true;
            Ok(())
        })();

        if let Err(error) = result {
            let mut recovery_errors = Vec::new();
            if file_published && let Err(remove_error) = fs::remove_file(&self.file.destination) {
                recovery_errors.push(format!(
                    "새 출력 제거 실패 {}: {remove_error}",
                    self.file.destination.display()
                ));
            }
            if sidecar_published
                && let Err(remove_error) = fs::remove_dir_all(&self.sidecar.destination)
            {
                recovery_errors.push(format!(
                    "새 미디어 제거 실패 {}: {remove_error}",
                    self.sidecar.destination.display()
                ));
            }
            if sidecar_backed_up {
                let restore = hook(PairPublishStep::RestoreSidecar).and_then(|()| {
                    fs::rename(&sidecar_backup, &self.sidecar.destination).with_context(|| {
                        format!(
                            "기존 미디어 디렉터리 복원 실패: {}",
                            self.sidecar.destination.display()
                        )
                    })
                });
                if let Err(restore_error) = restore {
                    self.sidecar.preserve_workspace = true;
                    recovery_errors.push(format!("{restore_error:#}"));
                }
            }
            if file_backed_up {
                let restore = hook(PairPublishStep::RestoreFile).and_then(|()| {
                    fs::rename(&file_backup, &self.file.destination).with_context(|| {
                        format!(
                            "기존 출력 파일 복원 실패: {}",
                            self.file.destination.display()
                        )
                    })
                });
                if let Err(restore_error) = restore {
                    self.file.preserve_workspace = true;
                    recovery_errors.push(format!("{restore_error:#}"));
                }
            }
            if recovery_errors.is_empty() {
                return Err(error).context("게시 실패 후 기존 본문과 미디어를 복원했습니다");
            }
            anyhow::bail!(
                "{error:#}; 복구가 완전하지 않아 backup을 보존했습니다: {}",
                recovery_errors.join("; ")
            );
        }

        let mut warnings = Vec::new();
        if file_backed_up && let Err(error) = fs::remove_file(&file_backup) {
            self.file.preserve_workspace = true;
            warnings.push(format!(
                "새 출력은 게시했지만 기존 파일 backup을 정리하지 못했습니다: {} ({error})",
                file_backup.display()
            ));
        }
        if sidecar_backed_up && let Err(error) = fs::remove_dir_all(&sidecar_backup) {
            self.sidecar.preserve_workspace = true;
            warnings.push(format!(
                "새 출력은 게시했지만 기존 미디어 backup을 정리하지 못했습니다: {} ({error})",
                sidecar_backup.display()
            ));
        }
        for destination in [&self.file.destination, &self.sidecar.destination] {
            if let Err(error) = sync_parent_directory(destination) {
                warnings.push(format!("{error:#}"));
            }
        }
        let _ = fs::remove_dir(&self.file.workspace);
        let _ = fs::remove_dir(&self.sidecar.workspace);
        Ok((!warnings.is_empty()).then(|| warnings.join("; ")))
    }
}

fn reject_overlapping_outputs(
    destination: &Path,
    sidecar_destination: &Path,
    input: Option<&Path>,
) -> anyhow::Result<()> {
    let destination = std::path::absolute(destination)?;
    let sidecar = std::path::absolute(sidecar_destination)?;
    if destination == sidecar
        || destination.starts_with(&sidecar)
        || sidecar.starts_with(&destination)
    {
        anyhow::bail!(
            "본문 출력과 미디어 sidecar 경로가 겹칩니다: {} / {}",
            destination.display(),
            sidecar.display()
        );
    }
    if let Some(input) = input {
        let input = std::path::absolute(input)?;
        if input == sidecar || input.starts_with(&sidecar) {
            anyhow::bail!(
                "입력 문서가 교체할 미디어 sidecar 안에 있어 안전하게 게시할 수 없습니다: {}",
                input.display()
            );
        }
    }
    Ok(())
}

fn inspect_directory_destination(destination: &Path) -> anyhow::Result<DirectorySnapshot> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "미디어 출력 경로가 심볼릭 링크입니다. 안전한 게시를 위해 거부합니다: {}",
                    destination.display()
                );
            }
            if !metadata.file_type().is_dir() {
                anyhow::bail!(
                    "미디어 출력 경로가 디렉터리가 아닙니다: {}",
                    destination.display()
                );
            }
            Ok(DirectorySnapshot::Directory {
                identity: FileIdentity::from_metadata(&metadata, destination)?,
                permission_state: PermissionState::from_metadata(&metadata),
                #[cfg(not(windows))]
                permissions: metadata.permissions(),
                #[cfg(windows)]
                dacl: windows_security_descriptor(
                    destination,
                    windows_sys::Win32::Security::DACL_SECURITY_INFORMATION,
                )?,
                tree: inspect_tree(destination)?,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(DirectorySnapshot::Missing)
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "미디어 출력 경로를 확인할 수 없습니다: {}",
                destination.display()
            )
        }),
    }
}

fn recheck_directory_destination(
    destination: &Path,
    snapshot: &DirectorySnapshot,
) -> anyhow::Result<()> {
    let current = inspect_directory_destination(destination)?;
    match (snapshot, current) {
        (DirectorySnapshot::Missing, DirectorySnapshot::Missing) => Ok(()),
        (
            DirectorySnapshot::Directory {
                identity: expected,
                tree: expected_tree,
                permission_state: expected_permissions,
                #[cfg(windows)]
                    dacl: expected_dacl,
                ..
            },
            DirectorySnapshot::Directory {
                identity: actual,
                tree: actual_tree,
                permission_state: actual_permissions,
                #[cfg(windows)]
                    dacl: actual_dacl,
                ..
            },
        ) if expected == &actual
            && expected_tree == &actual_tree
            && expected_permissions == &actual_permissions
            && {
                #[cfg(windows)]
                {
                    expected_dacl == &actual_dacl
                }
                #[cfg(not(windows))]
                {
                    true
                }
            } =>
        {
            Ok(())
        }
        (DirectorySnapshot::Missing, DirectorySnapshot::Directory { .. }) => anyhow::bail!(
            "출력 준비 중 미디어 디렉터리가 새로 생성되어 게시를 중단합니다: {}",
            destination.display()
        ),
        (DirectorySnapshot::Directory { .. }, DirectorySnapshot::Missing) => anyhow::bail!(
            "출력 준비 중 기존 미디어 디렉터리가 사라져 게시를 중단합니다: {}",
            destination.display()
        ),
        (DirectorySnapshot::Directory { .. }, DirectorySnapshot::Directory { .. }) => {
            anyhow::bail!(
                "출력 준비 중 미디어 디렉터리 내용이 바뀌어 게시를 중단합니다: {}",
                destination.display()
            )
        }
    }
}

fn inspect_tree(root: &Path) -> anyhow::Result<TreeState> {
    fn walk(
        root: &Path,
        directory: &Path,
        entries: &mut Vec<TreeEntryState>,
    ) -> anyhow::Result<()> {
        let mut children = fs::read_dir(directory)
            .with_context(|| {
                format!(
                    "미디어 디렉터리를 읽을 수 없습니다: {}",
                    directory.display()
                )
            })?
            .collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(fs::DirEntry::file_name);
        for child in children {
            let path = child.path();
            let metadata = fs::symlink_metadata(&path)?;
            let relative = path
                .strip_prefix(root)
                .expect("순회 중인 미디어 경로는 root 아래임")
                .to_path_buf();
            let kind = if metadata.file_type().is_dir() {
                TreeEntryKind::Directory
            } else if metadata.file_type().is_file() {
                let identity = FileIdentity::from_metadata(&metadata, &path)?;
                if identity.has_multiple_links() {
                    anyhow::bail!(
                        "미디어 파일에 하드 링크 별칭이 있어 안전하게 교체할 수 없습니다: {}",
                        path.display()
                    );
                }
                let content = inspect_file_content(&path, &metadata)?;
                TreeEntryKind::File {
                    len: content.len,
                    modified: content.modified,
                    sha256: content.sha256,
                }
            } else {
                anyhow::bail!(
                    "미디어 디렉터리에 일반 파일/디렉터리가 아닌 항목이 있습니다: {}",
                    path.display()
                );
            };
            entries.push(TreeEntryState {
                relative,
                kind: kind.clone(),
                permissions: PermissionState::from_metadata(&metadata),
                #[cfg(windows)]
                dacl: windows_security_descriptor(
                    &path,
                    windows_sys::Win32::Security::DACL_SECURITY_INFORMATION,
                )?,
            });
            if matches!(kind, TreeEntryKind::Directory) {
                walk(root, &path, entries)?;
            }
        }
        Ok(())
    }

    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        anyhow::bail!("미디어 sidecar가 디렉터리가 아닙니다: {}", root.display());
    }
    let mut entries = Vec::new();
    walk(root, root, &mut entries)?;
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(TreeState { entries })
}

fn copy_directory_tree(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fn copy_children(source: &Path, destination: &Path) -> anyhow::Result<()> {
        fs::create_dir(destination)?;
        let source_permissions = fs::metadata(source)?.permissions();
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let from = entry.path();
            let to = destination.join(entry.file_name());
            let metadata = fs::symlink_metadata(&from)?;
            if metadata.file_type().is_dir() {
                copy_children(&from, &to)?;
            } else if metadata.file_type().is_file() {
                let identity = FileIdentity::from_metadata(&metadata, &from)?;
                if identity.has_multiple_links() {
                    anyhow::bail!(
                        "미디어 파일에 하드 링크 별칭이 있어 안전하게 복제할 수 없습니다: {}",
                        from.display()
                    );
                }
                fs::copy(&from, &to)?;
                fs::set_permissions(&to, metadata.permissions())?;
            } else {
                anyhow::bail!(
                    "미디어 디렉터리에 복제할 수 없는 항목이 있습니다: {}",
                    from.display()
                );
            }
        }
        fs::set_permissions(destination, source_permissions)?;
        Ok(())
    }
    copy_children(source, destination)
}

fn sync_directory_tree(root: &Path) -> anyhow::Result<()> {
    fn sync(directory: &Path) -> anyhow::Result<()> {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_dir() {
                sync(&path)?;
            } else if metadata.file_type().is_file() {
                fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)?
                    .sync_all()?;
            } else {
                anyhow::bail!("동기화할 수 없는 미디어 항목입니다: {}", path.display());
            }
        }
        #[cfg(unix)]
        fs::File::open(directory)?.sync_all()?;
        Ok(())
    }
    sync(root).with_context(|| format!("미디어 sidecar 동기화 실패: {}", root.display()))
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PermissionState(u32);

#[cfg(unix)]
impl PermissionState {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        Self(metadata.permissions().mode())
    }
}

#[cfg(not(unix))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PermissionState(bool);

#[cfg(not(unix))]
impl PermissionState {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self(metadata.permissions().readonly())
    }
}

fn create_unique_workspace(
    parent: &Path,
    display_name: &str,
    marker: &str,
) -> anyhow::Result<PathBuf> {
    for _ in 0..1024 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let random = random_token()?;
        let workspace = parent.join(format!(
            ".{display_name}.{marker}-{}-{sequence}-{random}.tmp",
            std::process::id()
        ));
        match create_private_workspace(&workspace) {
            Ok(()) => return Ok(workspace),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "출력 파일 옆에 임시 작업공간을 만들 수 없습니다: {}",
                        workspace.display()
                    )
                });
            }
        }
    }
    anyhow::bail!(
        "충돌하지 않는 임시 작업공간을 만들 수 없습니다: {}",
        parent.display()
    )
}

fn random_token() -> anyhow::Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("안전한 임시 작업공간 이름용 난수 생성 실패: {error}"))?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut token, "{byte:02x}").expect("String 쓰기는 실패하지 않음");
    }
    Ok(token)
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        if !self.preserve_workspace {
            let _ = fs::remove_dir_all(&self.workspace);
        }
    }
}

fn inspect_destination(destination: &Path) -> anyhow::Result<DestinationSnapshot> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                anyhow::bail!(
                    "출력 경로가 심볼릭 링크입니다. 안전한 게시를 위해 거부합니다: {}",
                    destination.display()
                );
            }
            if !file_type.is_file() {
                anyhow::bail!(
                    "출력 경로가 일반 파일이 아닙니다: {}",
                    destination.display()
                );
            }
            let identity = FileIdentity::from_metadata(&metadata, destination)?;
            if identity.has_multiple_links() {
                anyhow::bail!(
                    "출력 파일에 하드 링크 별칭이 있어 안전하게 교체할 수 없습니다: {}",
                    destination.display()
                );
            }
            let content = inspect_file_content(destination, &metadata)?;
            Ok(DestinationSnapshot::Regular {
                identity,
                content,
                permission_state: PermissionState::from_metadata(&metadata),
                #[cfg(not(windows))]
                permissions: metadata.permissions(),
                #[cfg(windows)]
                dacl: windows_security_descriptor(
                    destination,
                    windows_sys::Win32::Security::DACL_SECURITY_INFORMATION,
                )?,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DestinationSnapshot::Missing),
        Err(e) => Err(e)
            .with_context(|| format!("출력 경로를 확인할 수 없습니다: {}", destination.display())),
    }
}

fn inspect_file_content(
    path: &Path,
    path_metadata: &fs::Metadata,
) -> anyhow::Result<FileContentState> {
    let expected_identity = FileIdentity::from_metadata(path_metadata, path)?;
    let expected_len = path_metadata.len();
    let expected_modified = path_metadata
        .modified()
        .with_context(|| format!("파일 수정 시각을 확인할 수 없습니다: {}", path.display()))?;

    let mut file = fs::File::open(path)
        .with_context(|| format!("파일 내용을 검사할 수 없습니다: {}", path.display()))?;
    let before = file
        .metadata()
        .with_context(|| format!("열린 파일의 상태를 확인할 수 없습니다: {}", path.display()))?;
    let before_identity = FileIdentity::from_metadata(&before, path)?;
    let before_modified = before
        .modified()
        .with_context(|| format!("파일 수정 시각을 확인할 수 없습니다: {}", path.display()))?;
    if before_identity != expected_identity
        || before.len() != expected_len
        || before_modified != expected_modified
    {
        anyhow::bail!(
            "파일 상태가 검사 중 바뀌어 안전하게 스냅샷할 수 없습니다: {}",
            path.display()
        );
    }

    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("파일 내용을 읽을 수 없습니다: {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let after = file
        .metadata()
        .with_context(|| format!("열린 파일의 상태를 확인할 수 없습니다: {}", path.display()))?;
    let after_identity = FileIdentity::from_metadata(&after, path)?;
    let after_modified = after
        .modified()
        .with_context(|| format!("파일 수정 시각을 확인할 수 없습니다: {}", path.display()))?;
    if after_identity != expected_identity
        || after.len() != expected_len
        || after_modified != expected_modified
    {
        anyhow::bail!(
            "파일 내용이 검사 중 바뀌어 안전하게 스냅샷할 수 없습니다: {}",
            path.display()
        );
    }

    let path_after = fs::symlink_metadata(path)
        .with_context(|| format!("파일 경로를 다시 확인할 수 없습니다: {}", path.display()))?;
    if path_after.file_type().is_symlink() || !path_after.file_type().is_file() {
        anyhow::bail!(
            "파일 경로가 검사 중 일반 파일이 아닌 항목으로 바뀌었습니다: {}",
            path.display()
        );
    }
    let path_after_identity = FileIdentity::from_metadata(&path_after, path)?;
    let path_after_modified = path_after
        .modified()
        .with_context(|| format!("파일 수정 시각을 확인할 수 없습니다: {}", path.display()))?;
    if path_after_identity != expected_identity
        || path_after.len() != expected_len
        || path_after_modified != expected_modified
    {
        anyhow::bail!(
            "파일 경로 상태가 검사 중 바뀌어 안전하게 스냅샷할 수 없습니다: {}",
            path.display()
        );
    }

    Ok(FileContentState {
        len: expected_len,
        modified: expected_modified,
        sha256: hasher.finalize().into(),
    })
}

fn recheck_destination(destination: &Path, snapshot: &DestinationSnapshot) -> anyhow::Result<()> {
    let current = inspect_destination(destination)?;
    match (snapshot, current) {
        (DestinationSnapshot::Missing, DestinationSnapshot::Missing) => Ok(()),
        (
            DestinationSnapshot::Regular {
                identity: expected,
                content: expected_content,
                permission_state: expected_permissions,
                #[cfg(windows)]
                    dacl: expected_dacl,
                ..
            },
            DestinationSnapshot::Regular {
                identity: actual,
                content: actual_content,
                permission_state: actual_permissions,
                #[cfg(windows)]
                    dacl: actual_dacl,
                ..
            },
        ) if expected == &actual
            && expected_content == &actual_content
            && expected_permissions == &actual_permissions
            && {
                #[cfg(windows)]
                {
                    expected_dacl == &actual_dacl
                }
                #[cfg(not(windows))]
                {
                    true
                }
            } =>
        {
            Ok(())
        }
        (DestinationSnapshot::Missing, DestinationSnapshot::Regular { .. }) => {
            anyhow::bail!(
                "출력 준비 중 대상 경로가 새로 생성되어 게시를 중단합니다: {}",
                destination.display()
            )
        }
        (DestinationSnapshot::Regular { .. }, DestinationSnapshot::Missing) => {
            anyhow::bail!(
                "출력 준비 중 기존 대상 파일이 사라져 게시를 중단합니다: {}",
                destination.display()
            )
        }
        (
            DestinationSnapshot::Regular {
                identity: expected,
                content: expected_content,
                ..
            },
            DestinationSnapshot::Regular {
                identity: actual,
                content: actual_content,
                ..
            },
        ) if expected == &actual && expected_content == &actual_content => {
            anyhow::bail!(
                "출력 준비 중 대상 파일 권한/ACL이 바뀌어 게시를 중단합니다: {}",
                destination.display()
            )
        }
        (
            DestinationSnapshot::Regular {
                identity: expected, ..
            },
            DestinationSnapshot::Regular {
                identity: actual, ..
            },
        ) if expected == &actual => {
            anyhow::bail!(
                "출력 준비 중 대상 파일 내용이 바뀌어 게시를 중단합니다: {}",
                destination.display()
            )
        }
        (DestinationSnapshot::Regular { .. }, DestinationSnapshot::Regular { .. }) => {
            anyhow::bail!(
                "출력 준비 중 대상 파일이 다른 파일로 바뀌어 게시를 중단합니다: {}",
                destination.display()
            )
        }
    }
}

fn reject_unsafe_alias(
    input: Option<&Path>,
    destination: &Path,
    destination_snapshot: &DestinationSnapshot,
) -> anyhow::Result<()> {
    let Some(input) = input else {
        return Ok(());
    };
    if input == destination {
        let metadata = fs::symlink_metadata(input).with_context(|| {
            format!("제자리 출력 입력을 확인할 수 없습니다: {}", input.display())
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            anyhow::bail!(
                "제자리 출력은 심볼릭 링크가 아닌 일반 파일에서만 지원합니다: {}",
                input.display()
            );
        }
        if !matches!(destination_snapshot, DestinationSnapshot::Regular { .. }) {
            anyhow::bail!("제자리 출력 입력 파일이 없습니다: {}", input.display());
        }
        return Ok(());
    }

    let input_link = match fs::symlink_metadata(input) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("입력 경로를 확인할 수 없습니다: {}", input.display()));
        }
    };
    let input_followed = match fs::metadata(input) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("입력 경로를 확인할 수 없습니다: {}", input.display()));
        }
    };
    let DestinationSnapshot::Regular {
        identity: destination_identity,
        ..
    } = destination_snapshot
    else {
        return Ok(());
    };
    let input_identity = FileIdentity::from_metadata(&input_followed, input)?;
    if &input_identity == destination_identity {
        if input_link.file_type().is_symlink() {
            anyhow::bail!(
                "입력과 출력이 심볼릭 링크로 같은 파일을 가리킵니다. \
                 심볼릭 링크 별칭을 통한 제자리 출력은 지원하지 않습니다: {} -> {}",
                input.display(),
                destination.display()
            );
        }
        // destination의 링크 수가 1임은 inspect_destination에서 확인했다. 따라서
        // 같은 identity의 일반 파일은 `./file`과 절대경로처럼 같은 디렉터리 항목의
        // 다른 철자이며, 하드 링크 별칭이 아니다.
        return Ok(());
    }
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    links: u64,
}

#[cfg(unix)]
impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata, _path: &Path) -> anyhow::Result<Self> {
        use std::os::unix::fs::MetadataExt as _;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            links: metadata.nlink(),
        })
    }

    fn has_multiple_links(&self) -> bool {
        self.links > 1
    }
}

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    volume: u32,
    index: u64,
    links: u32,
}

#[cfg(windows)]
impl FileIdentity {
    fn from_metadata(_metadata: &fs::Metadata, path: &Path) -> anyhow::Result<Self> {
        use std::os::windows::ffi::OsStrExt as _;
        use std::ptr;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            GetFileInformationByHandle, OPEN_EXISTING,
        };

        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let handle = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "Windows 파일 식별자용 핸들을 열 수 없습니다: {}",
                    path.display()
                )
            });
        }

        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        let loaded = unsafe { GetFileInformationByHandle(handle, &mut information) };
        let error = (loaded == 0).then(std::io::Error::last_os_error);
        unsafe {
            CloseHandle(handle);
        }
        if let Some(error) = error {
            return Err(error).with_context(|| {
                format!(
                    "Windows 파일 식별자를 확인할 수 없습니다: {}",
                    path.display()
                )
            });
        }
        Ok(Self {
            volume: information.dwVolumeSerialNumber,
            index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
            links: information.nNumberOfLinks,
        })
    }

    fn has_multiple_links(&self) -> bool {
        self.links > 1
    }
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    canonical_path: PathBuf,
}

#[cfg(not(any(unix, windows)))]
impl FileIdentity {
    fn from_metadata(_metadata: &fs::Metadata, path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            canonical_path: path.canonicalize()?,
        })
    }

    fn has_multiple_links(&self) -> bool {
        false
    }
}

#[cfg(unix)]
fn create_private_workspace(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

    fs::DirBuilder::new().mode(0o700).create(path)?;
    // 일부 비표준 파일시스템의 생성 모드 처리와 호출자 umask에 관계없이 정확히 0700.
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn create_private_workspace(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

    // 보호된 DACL에 object owner(OW)만 모든 권한을 부여한다. OI/CI는 작업공간 안에
    // 만드는 staged 파일과 디렉터리에도 같은 제한을 상속한다. CreateDirectoryW에
    // SECURITY_ATTRIBUTES를 직접 넘기므로 생성과 ACL 적용 사이의 공개 race가 없다.
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)\0".encode_utf16().collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let created = unsafe { CreateDirectoryW(wide_path.as_ptr(), &attributes) };
    let create_error = (created == 0).then(std::io::Error::last_os_error);
    unsafe {
        LocalFree(descriptor);
    }
    match create_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(windows)]
fn windows_security_descriptor(path: &Path, requested_information: u32) -> anyhow::Result<Vec<u8>> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::ptr;
    use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows_sys::Win32::Security::GetFileSecurityW;

    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut needed = 0_u32;
    let first = unsafe {
        GetFileSecurityW(
            wide_path.as_ptr(),
            requested_information,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if first != 0
        || std::io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
    {
        anyhow::bail!(
            "Windows 보안 설명자 크기를 확인할 수 없습니다: {} ({})",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    let mut descriptor = vec![0_u8; needed as usize];
    let loaded = unsafe {
        GetFileSecurityW(
            wide_path.as_ptr(),
            requested_information,
            descriptor.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if loaded == 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!("Windows 보안 설명자를 읽을 수 없습니다: {}", path.display())
        });
    }
    Ok(descriptor)
}

#[cfg(windows)]
fn windows_set_dacl(path: &Path, descriptor: &mut [u8]) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, SetFileSecurityW};

    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let applied = unsafe {
        SetFileSecurityW(
            wide_path.as_ptr(),
            DACL_SECURITY_INFORMATION,
            descriptor.as_mut_ptr().cast(),
        )
    };
    if applied == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("Windows 출력 ACL을 적용할 수 없습니다: {}", path.display()));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_copy_dacl(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    let mut descriptor = windows_security_descriptor(source, DACL_SECURITY_INFORMATION)?;
    windows_set_dacl(destination, &mut descriptor).with_context(|| {
        format!(
            "기존 출력의 Windows ACL을 승계하지 못했습니다: {} -> {}",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(windows)]
fn windows_apply_parent_default_dacl(
    path: &Path,
    destination: &Path,
    is_directory: bool,
) -> anyhow::Result<()> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    windows_apply_inherited_dacl(path, parent, is_directory)
}

#[cfg(windows)]
fn windows_apply_inherited_dacl(
    path: &Path,
    parent: &Path,
    is_directory: bool,
) -> anyhow::Result<()> {
    use std::ptr;
    use windows_sys::Win32::Security::{
        CreatePrivateObjectSecurityEx, DACL_SECURITY_INFORMATION, DestroyPrivateObjectSecurity,
        GENERIC_MAPPING, PSECURITY_DESCRIPTOR, SEF_DACL_AUTO_INHERIT, SetFileSecurityW,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    };

    let mut parent_descriptor = windows_security_descriptor(parent, DACL_SECURITY_INFORMATION)?;
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    let mut inherited_descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let created = unsafe {
        CreatePrivateObjectSecurityEx(
            parent_descriptor.as_mut_ptr().cast(),
            ptr::null_mut(),
            &mut inherited_descriptor,
            ptr::null(),
            if is_directory { 1 } else { 0 },
            SEF_DACL_AUTO_INHERIT,
            ptr::null_mut(),
            &mapping,
        )
    };
    if created == 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "부모 디렉터리에서 Windows 기본 ACL을 계산할 수 없습니다: {}",
                parent.display()
            )
        });
    }

    let result = (|| -> anyhow::Result<()> {
        use std::os::windows::ffi::OsStrExt as _;

        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let applied = unsafe {
            SetFileSecurityW(
                wide_path.as_ptr(),
                DACL_SECURITY_INFORMATION,
                inherited_descriptor,
            )
        };
        if applied == 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "새 출력에 부모 디렉터리의 Windows 기본 ACL을 적용할 수 없습니다: {}",
                    path.display()
                )
            });
        }
        Ok(())
    })();
    let destroyed = unsafe { DestroyPrivateObjectSecurity(&inherited_descriptor) };
    if destroyed == 0 && result.is_ok() {
        return Err(std::io::Error::last_os_error())
            .context("Windows 임시 보안 설명자를 해제할 수 없습니다");
    }
    result
}

#[cfg(windows)]
fn windows_prepare_staged_tree(
    staged: &Path,
    destination: &Path,
    snapshot: &DirectorySnapshot,
) -> anyhow::Result<()> {
    match snapshot {
        DirectorySnapshot::Directory { dacl, .. } => {
            let mut dacl = dacl.clone();
            windows_set_dacl(staged, &mut dacl).with_context(|| {
                format!(
                    "기존 미디어 루트의 Windows ACL을 승계하지 못했습니다: {}",
                    destination.display()
                )
            })?;
        }
        DirectorySnapshot::Missing => {
            windows_apply_parent_default_dacl(staged, destination, true)?;
        }
    }

    let original = match snapshot {
        DirectorySnapshot::Directory { tree, .. } => Some(tree),
        DirectorySnapshot::Missing => None,
    };
    let staged_tree = inspect_tree(staged)?;
    for entry in &staged_tree.entries {
        let staged_path = staged.join(&entry.relative);
        let is_directory = matches!(entry.kind, TreeEntryKind::Directory);
        let existing = original.and_then(|tree| {
            tree.entries
                .binary_search_by(|candidate| candidate.relative.cmp(&entry.relative))
                .ok()
                .map(|index| &tree.entries[index])
                .filter(|candidate| {
                    matches!(
                        (&candidate.kind, &entry.kind),
                        (TreeEntryKind::Directory, TreeEntryKind::Directory)
                            | (TreeEntryKind::File { .. }, TreeEntryKind::File { .. })
                    )
                })
        });
        if let Some(existing) = existing {
            let mut dacl = existing.dacl.clone();
            windows_set_dacl(&staged_path, &mut dacl).with_context(|| {
                format!(
                    "기존 미디어 항목의 Windows ACL을 승계하지 못했습니다: {}",
                    destination.join(&entry.relative).display()
                )
            })?;
        } else {
            let staged_parent = staged_path.parent().with_context(|| {
                format!(
                    "미디어 항목의 부모 경로를 확인할 수 없습니다: {}",
                    staged_path.display()
                )
            })?;
            // 부모 staged 항목은 앞선 정렬 순회에서 이미 최종 ACL을 받았다. 그
            // descriptor로 새 자식의 상속 DACL을 계산해야 private workspace의
            // owner-only ACL이 게시 후 남지 않는다.
            windows_apply_inherited_dacl(&staged_path, staged_parent, is_directory)?;
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn create_private_workspace(path: &Path) -> std::io::Result<()> {
    // 지원되는 Unix/Windows와 달리 이 fallback은 owner-only ACL을 보장하지 않는다.
    // 현재 배포 대상에는 해당하지 않으며, 새 플랫폼을 추가할 때 전용 구현이 필요하다.
    fs::create_dir(path)
}

#[cfg(unix)]
fn sync_parent_directory(destination: &Path) -> anyhow::Result<()> {
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("출력 디렉터리 동기화 실패: {}", parent.display()))
}

#[cfg(not(unix))]
fn sync_parent_directory(_destination: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(any(not(any(unix, windows)), test))]
trait RecoveryFs {
    fn exists(&self, path: &Path) -> bool;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn copy(&self, from: &Path, to: &Path) -> std::io::Result<u64>;
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
}

#[cfg(not(any(unix, windows)))]
struct RealRecoveryFs;

#[cfg(not(any(unix, windows)))]
impl RecoveryFs for RealRecoveryFs {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        fs::rename(from, to)
    }

    fn copy(&self, from: &Path, to: &Path) -> std::io::Result<u64> {
        fs::copy(from, to)
    }

    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        fs::remove_file(path)
    }
}

#[cfg(any(not(unix), test))]
enum RecoveryState {
    Published {
        warning: Option<String>,
    },
    FailedRestored {
        error: anyhow::Error,
    },
    FailedBackupPreserved {
        error: anyhow::Error,
        backup: PathBuf,
    },
}

#[cfg(windows)]
fn windows_replace_preserving_acl(
    staged: &Path,
    destination: &Path,
    backup: &Path,
) -> RecoveryState {
    use std::os::windows::ffi::OsStrExt as _;
    use std::ptr;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let staged_wide = wide(staged);
    let destination_wide = wide(destination);
    let backup_wide = wide(backup);
    let replaced = unsafe {
        // flags=0은 ACL/스트림 병합 오류를 무시하지 않는다. 성공 시 ReplaceFileW가
        // 교체 대상의 DACL을 새 파일에 원자적으로 승계하고 원본은 backup에 둔다.
        ReplaceFileW(
            destination_wide.as_ptr(),
            staged_wide.as_ptr(),
            backup_wide.as_ptr(),
            0,
            ptr::null(),
            ptr::null(),
        )
    };
    if replaced != 0 {
        let warning = fs::remove_file(backup).err().map(|error| {
            format!(
                "새 출력은 게시했지만 기존 파일 백업을 정리하지 못했습니다: {} ({error})",
                backup.display()
            )
        });
        return RecoveryState::Published { warning };
    }

    let replace_error = std::io::Error::last_os_error();
    if !backup.exists() {
        return RecoveryState::FailedRestored {
            error: anyhow::Error::new(replace_error).context(format!(
                "Windows ReplaceFileW 게시에 실패했습니다. 기존 대상과 임시 출력을 보존했습니다: {}",
                destination.display()
            )),
        };
    }

    if destination.exists()
        && let Err(remove_error) = fs::remove_file(destination)
    {
        return RecoveryState::FailedBackupPreserved {
            error: anyhow::anyhow!(
                "Windows ReplaceFileW 실패 후 대상 정리에도 실패했습니다 \
                 (게시: {replace_error}; 정리: {remove_error})"
            ),
            backup: backup.to_path_buf(),
        };
    }
    match fs::rename(backup, destination) {
        Ok(()) => RecoveryState::FailedRestored {
            error: anyhow::Error::new(replace_error).context(format!(
                "Windows ReplaceFileW 게시에 실패해 기존 파일을 복원했습니다: {}",
                destination.display()
            )),
        },
        Err(rename_restore_error) => match fs::copy(backup, destination) {
            Ok(_) => RecoveryState::FailedBackupPreserved {
                error: anyhow::anyhow!(
                    "Windows ReplaceFileW 실패 후 rename 복원에 실패해 백업을 복사로 복원했습니다 \
                     (게시: {replace_error}; rename 복원: {rename_restore_error})"
                ),
                backup: backup.to_path_buf(),
            },
            Err(copy_restore_error) => RecoveryState::FailedBackupPreserved {
                error: anyhow::anyhow!(
                    "Windows ReplaceFileW 실패 후 원본 복원도 실패했습니다 \
                     (게시: {replace_error}; rename 복원: {rename_restore_error}; \
                     copy 복원: {copy_restore_error})"
                ),
                backup: backup.to_path_buf(),
            },
        },
    }
}

/// 기존 파일 위 rename을 보장하지 않는 플랫폼의 명시적 복구 상태 머신.
#[cfg(any(not(any(unix, windows)), test))]
fn publish_with_recovery(
    ops: &impl RecoveryFs,
    staged: &Path,
    destination: &Path,
    backup: &Path,
) -> RecoveryState {
    if !ops.exists(destination) {
        return match ops.rename(staged, destination) {
            Ok(()) => RecoveryState::Published { warning: None },
            Err(error) => RecoveryState::FailedRestored {
                error: anyhow::Error::new(error).context(format!(
                    "검증된 임시 출력을 최종 경로에 게시하지 못했습니다: {}",
                    destination.display()
                )),
            },
        };
    }

    if let Err(error) = ops.rename(destination, backup) {
        return RecoveryState::FailedRestored {
            error: anyhow::Error::new(error).context(format!(
                "기존 출력 파일을 안전하게 백업하지 못했습니다: {}",
                destination.display()
            )),
        };
    }
    match ops.rename(staged, destination) {
        Ok(()) => {
            let warning = ops.remove_file(backup).err().map(|error| {
                format!(
                    "새 출력은 게시했지만 기존 파일 백업을 정리하지 못했습니다: {} ({error})",
                    backup.display()
                )
            });
            RecoveryState::Published { warning }
        }
        Err(publish_error) => match ops.rename(backup, destination) {
            Ok(()) => RecoveryState::FailedRestored {
                error: anyhow::Error::new(publish_error).context(format!(
                    "검증된 임시 출력을 게시하지 못해 기존 파일을 복원했습니다: {}",
                    destination.display()
                )),
            },
            Err(rename_restore_error) => match ops.copy(backup, destination) {
                Ok(_) => RecoveryState::FailedBackupPreserved {
                    error: anyhow::anyhow!(
                        "출력 게시 실패 후 rename 복원에 실패해 백업을 복사로 복원했습니다 \
                         (게시: {publish_error}; rename 복원: {rename_restore_error})"
                    ),
                    backup: backup.to_path_buf(),
                },
                Err(copy_restore_error) => RecoveryState::FailedBackupPreserved {
                    error: anyhow::anyhow!(
                        "출력 게시 실패 후 원본 복원도 실패했습니다 \
                         (게시: {publish_error}; rename 복원: {rename_restore_error}; \
                         copy 복원: {copy_restore_error})"
                    ),
                    backup: backup.to_path_buf(),
                },
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn test_dir(name: &str) -> PathBuf {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-output-test-{}-{sequence}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        dir
    }

    fn assert_no_debris(dir: &Path) {
        let leftovers: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (name.contains(".hwp-output-") || name.contains(".hwp-media-")).then_some(name)
            })
            .collect();
        assert!(leftovers.is_empty(), "임시 작업공간 잔재: {leftovers:?}");
    }

    #[test]
    fn private_input_snapshot_hash_and_operation_use_the_same_bytes() {
        let dir = test_dir("private-input-snapshot");
        let input = dir.join("reference.hwpx");
        let destination = dir.join("result.hwpx");
        fs::write(&input, b"VERSION-A").unwrap();
        let expected_hash = Sha256::digest(b"VERSION-A")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        let (hash, observed) = write_with_private_input_snapshot(
            &destination,
            &input,
            1024,
            SnapshotOutputMode::PlanOnly,
            |snapshot, _, callback_hash| {
                fs::write(&input, b"VERSION-B")?;
                assert_eq!(callback_hash, expected_hash);
                Ok(fs::read(snapshot)?)
            },
            |_, _| Ok(()),
        )
        .unwrap();

        assert_eq!(hash, expected_hash);
        assert_eq!(observed, b"VERSION-A");
        assert_eq!(fs::read(&input).unwrap(), b"VERSION-B");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshot_transaction_rejects_destination_replacement_before_publish() {
        let dir = test_dir("snapshot-destination-race");
        let input = dir.join("reference.hwpx");
        let destination = dir.join("result.hwpx");
        fs::write(&input, b"SOURCE").unwrap();
        fs::write(&destination, b"ORIGINAL").unwrap();

        let result = write_with_private_input_snapshot(
            &destination,
            &input,
            1024,
            SnapshotOutputMode::Publish,
            |_, staged, _| {
                fs::write(staged, b"GENERATED")?;
                fs::write(&destination, b"RACER")?;
                Ok(())
            },
            |_, _| Ok(()),
        );

        assert!(
            result.is_err(),
            "changed destination baseline must reject publish"
        );
        assert_eq!(fs::read(&destination).unwrap(), b"RACER");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_writer_failure_preserves_existing_destination() {
        let dir = test_dir("writer-failure");
        let destination = dir.join("result.hwpx");
        fs::write(&destination, b"ORIGINAL").unwrap();

        let result = write_validated(
            &destination,
            None,
            |staged| -> anyhow::Result<()> {
                fs::write(staged, b"PARTIAL")?;
                anyhow::bail!("강제 쓰기 실패")
            },
            |_, _| Ok(()),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verification_failure_preserves_existing_destination() {
        let dir = test_dir("verify-failure");
        let destination = dir.join("result.hwp");
        fs::write(&destination, b"ORIGINAL").unwrap();

        let result = write_validated(
            &destination,
            None,
            |staged| {
                fs::write(staged, b"COMPLETE")?;
                Ok(())
            },
            |_, _| anyhow::bail!("강제 검증 실패"),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn destination_is_rechecked_before_publish() {
        use std::os::unix::fs::symlink;

        let dir = test_dir("recheck");
        let destination = dir.join("result.hwpx");
        let other = dir.join("other.hwpx");
        fs::write(&other, b"OTHER").unwrap();

        let result = write_validated(
            &destination,
            None,
            |staged| {
                fs::write(staged, b"NEW")?;
                symlink(&other, &destination)?;
                Ok(())
            },
            |_, _| Ok(()),
        );
        assert!(result.is_err());
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&other).unwrap(), b"OTHER");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_file_content_race_is_detected_by_digest_even_with_restored_mtime() {
        let dir = test_dir("same-file-content-race");
        let destination = dir.join("result.hwpx");
        fs::write(&destination, b"ORIGINAL").unwrap();
        let original_modified = fs::metadata(&destination).unwrap().modified().unwrap();

        let result = write_validated(
            &destination,
            None,
            |staged| {
                fs::write(staged, b"NEW")?;
                fs::write(&destination, b"MUTATION")?;
                fs::OpenOptions::new()
                    .write(true)
                    .open(&destination)?
                    .set_times(fs::FileTimes::new().set_modified(original_modified))?;
                Ok(())
            },
            |_, _| Ok(()),
        );

        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("대상 파일 내용이 바뀌어"));
        assert_eq!(fs::read(&destination).unwrap(), b"MUTATION");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn directory_destination_is_rejected_before_writer() {
        let dir = test_dir("directory");
        let destination = dir.join("result.hwpx");
        fs::create_dir(&destination).unwrap();
        let writer_called = Cell::new(false);

        let result = write_validated(
            &destination,
            None,
            |_| {
                writer_called.set(true);
                Ok(())
            },
            |_, _| Ok(()),
        );
        assert!(result.is_err());
        assert!(!writer_called.get());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn workspace_is_private_and_keeps_real_suffix() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = test_dir("private-workspace");
        let destination = dir.join("result.hwpx");
        write_validated(
            &destination,
            None,
            |staged| {
                assert_eq!(staged.extension().and_then(|e| e.to_str()), Some("hwpx"));
                let workspace = staged.parent().unwrap();
                let mode = fs::metadata(workspace).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o700);
                fs::write(staged, b"NEW")?;
                Ok(())
            },
            |_, _| Ok(()),
        )
        .unwrap();
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    fn windows_dacl_sddl(path: &Path) -> String {
        use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
        use std::ptr;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
            SDDL_REVISION_1, SE_FILE_OBJECT,
        };
        use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};

        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide_path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, 0);
        let mut sddl = ptr::null_mut();
        let mut len = 0_u32;
        assert_ne!(
            unsafe {
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    SDDL_REVISION_1,
                    DACL_SECURITY_INFORMATION,
                    &mut sddl,
                    &mut len,
                )
            },
            0
        );
        let text = std::ffi::OsString::from_wide(unsafe {
            std::slice::from_raw_parts(sddl, len as usize)
        })
        .to_string_lossy()
        .into_owned();
        unsafe {
            LocalFree(sddl.cast());
            LocalFree(descriptor);
        }
        text
    }

    #[cfg(windows)]
    fn windows_set_sddl_dacl(path: &Path, sddl: &str) {
        use std::os::windows::ffi::OsStrExt as _;
        use std::ptr;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SetFileSecurityW,
        };

        let wide_sddl = sddl
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide_sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            },
            0
        );
        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        assert_ne!(
            unsafe { SetFileSecurityW(wide_path.as_ptr(), DACL_SECURITY_INFORMATION, descriptor,) },
            0
        );
        unsafe {
            LocalFree(descriptor);
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_workspace_has_one_owner_rights_ace() {
        use std::os::windows::ffi::OsStrExt as _;
        use std::ptr;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSidToSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
        };
        use windows_sys::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
            PSECURITY_DESCRIPTOR, PSID,
        };

        let dir = test_dir("windows-private-workspace");
        let workspace = dir.join("private");
        create_private_workspace(&workspace).unwrap();
        let path: Vec<u16> = workspace
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let status = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, 0);
        assert!(!dacl.is_null());
        assert_eq!(unsafe { (*dacl).AceCount }, 1, "상속 없는 owner-only DACL");

        let mut ace = ptr::null_mut();
        assert_ne!(unsafe { GetAce(dacl, 0, &mut ace) }, 0);
        let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
        let actual_sid = unsafe { ptr::addr_of_mut!((*allowed).SidStart).cast() };
        let owner_rights = "S-1-3-4\0".encode_utf16().collect::<Vec<_>>();
        let mut expected_sid: PSID = ptr::null_mut();
        assert_ne!(
            unsafe { ConvertStringSidToSidW(owner_rights.as_ptr(), &mut expected_sid) },
            0
        );
        assert_ne!(unsafe { EqualSid(actual_sid, expected_sid) }, 0);
        unsafe {
            LocalFree(expected_sid);
            LocalFree(descriptor);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_destination_dacl_survives_replace_file() {
        let dir = test_dir("windows-existing-dacl");
        let destination = dir.join("result.hwpx");
        fs::write(&destination, b"ORIGINAL").unwrap();
        windows_set_sddl_dacl(&destination, "D:P(A;;FA;;;OW)(A;;GR;;;WD)");
        let expected = windows_dacl_sddl(&destination);

        write_validated(
            &destination,
            None,
            |staged| fs::write(staged, b"NEW").map_err(Into::into),
            |_, _| Ok(()),
        )
        .unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"NEW");
        assert_eq!(windows_dacl_sddl(&destination), expected);
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_concurrent_destination_dacl_change_aborts_without_clobbering_it() {
        let dir = test_dir("windows-existing-dacl-race");
        let destination = dir.join("result.hwpx");
        fs::write(&destination, b"ORIGINAL").unwrap();
        windows_set_sddl_dacl(&destination, "D:P(A;;FA;;;OW)");

        let result = write_validated(
            &destination,
            None,
            |staged| {
                fs::write(staged, b"NEW")?;
                windows_set_sddl_dacl(&destination, "D:P(A;;FA;;;OW)(A;;GR;;;WD)");
                Ok(())
            },
            |_, _| Ok(()),
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("권한/ACL"));
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL");
        assert_eq!(
            windows_dacl_sddl(&destination),
            windows_dacl_sddl(&{
                let probe = dir.join("probe.hwpx");
                fs::write(&probe, b"PROBE").unwrap();
                windows_set_sddl_dacl(&probe, "D:P(A;;FA;;;OW)(A;;GR;;;WD)");
                probe
            })
        );
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_new_destination_uses_parent_default_dacl() {
        let dir = test_dir("windows-parent-dacl");
        let ordinary = dir.join("ordinary.hwpx");
        let destination = dir.join("result.hwpx");
        fs::write(&ordinary, b"ORDINARY").unwrap();

        write_validated(
            &destination,
            None,
            |staged| fs::write(staged, b"NEW").map_err(Into::into),
            |_, _| Ok(()),
        )
        .unwrap();

        assert_eq!(
            windows_dacl_sddl(&destination),
            windows_dacl_sddl(&ordinary),
            "새 출력은 private staging DACL이 아니라 부모의 기본 ACL을 상속해야 함"
        );
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_paired_outputs_preserve_existing_file_and_directory_dacls() {
        let dir = test_dir("windows-pair-dacl");
        let destination = dir.join("report.md");
        let media = dir.join("report.media");
        fs::write(&destination, b"ORIGINAL BODY").unwrap();
        fs::create_dir(&media).unwrap();
        let image1 = media.join("image1.png");
        let nested = media.join("nested");
        fs::write(&image1, b"ORIGINAL IMAGE").unwrap();
        fs::create_dir(&nested).unwrap();
        let nested_existing = nested.join("existing.bin");
        fs::write(&nested_existing, b"EXISTING").unwrap();
        windows_set_sddl_dacl(&destination, "D:P(A;;FA;;;OW)(A;;GR;;;WD)");
        windows_set_sddl_dacl(&media, "D:P(A;OICI;FA;;;OW)(A;OICI;GR;;;WD)");
        windows_set_sddl_dacl(&image1, "D:P(A;;FA;;;OW)(A;;GR;;;WD)");
        windows_set_sddl_dacl(&nested, "D:P(A;OICI;FA;;;OW)(A;OICI;GR;;;WD)");
        windows_set_sddl_dacl(&nested_existing, "D:P(A;;FA;;;OW)(A;;GR;;;WD)");
        let inherited_probe = media.join("inherited-probe.bin");
        fs::write(&inherited_probe, b"PROBE").unwrap();
        let nested_probe = nested.join("inherited-probe.bin");
        fs::write(&nested_probe, b"PROBE").unwrap();
        let expected_file = windows_dacl_sddl(&destination);
        let expected_directory = windows_dacl_sddl(&media);
        let expected_image = windows_dacl_sddl(&image1);
        let expected_nested = windows_dacl_sddl(&nested);
        let expected_nested_existing = windows_dacl_sddl(&nested_existing);
        let expected_new = windows_dacl_sddl(&inherited_probe);
        let expected_nested_new = windows_dacl_sddl(&nested_probe);

        write_validated_with_sidecar(
            &destination,
            None,
            &media,
            |staged, staged_media| {
                fs::write(staged, b"NEW BODY")?;
                fs::write(staged_media.join("image2.png"), b"NEW IMAGE")?;
                fs::write(staged_media.join("nested/new.bin"), b"NEW NESTED")?;
                Ok(())
            },
            |_, _, _| Ok(()),
        )
        .unwrap();

        assert_eq!(windows_dacl_sddl(&destination), expected_file);
        assert_eq!(windows_dacl_sddl(&media), expected_directory);
        assert_eq!(windows_dacl_sddl(&image1), expected_image);
        assert_eq!(windows_dacl_sddl(&nested), expected_nested);
        assert_eq!(
            windows_dacl_sddl(&nested_existing),
            expected_nested_existing
        );
        assert_eq!(windows_dacl_sddl(&media.join("image2.png")), expected_new);
        assert_eq!(
            windows_dacl_sddl(&media.join("nested/new.bin")),
            expected_nested_new
        );
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_new_paired_tree_uses_final_parent_inheritance_for_all_entries() {
        let dir = test_dir("windows-new-pair-tree-dacl");
        let ordinary = dir.join("ordinary.media");
        fs::create_dir(&ordinary).unwrap();
        fs::create_dir(ordinary.join("nested")).unwrap();
        fs::write(ordinary.join("root.bin"), b"ROOT").unwrap();
        fs::write(ordinary.join("nested/child.bin"), b"CHILD").unwrap();

        let destination = dir.join("report.md");
        let media = dir.join("report.media");
        write_validated_with_sidecar(
            &destination,
            None,
            &media,
            |staged, staged_media| {
                fs::write(staged, b"BODY")?;
                fs::create_dir_all(staged_media.join("nested"))?;
                fs::write(staged_media.join("root.bin"), b"ROOT")?;
                fs::write(staged_media.join("nested/child.bin"), b"CHILD")?;
                Ok(())
            },
            |_, _, _| Ok(()),
        )
        .unwrap();

        assert_eq!(windows_dacl_sddl(&media), windows_dacl_sddl(&ordinary));
        assert_eq!(
            windows_dacl_sddl(&media.join("root.bin")),
            windows_dacl_sddl(&ordinary.join("root.bin"))
        );
        assert_eq!(
            windows_dacl_sddl(&media.join("nested")),
            windows_dacl_sddl(&ordinary.join("nested"))
        );
        assert_eq!(
            windows_dacl_sddl(&media.join("nested/child.bin")),
            windows_dacl_sddl(&ordinary.join("nested/child.bin"))
        );
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn successful_publish_preserves_destination_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = test_dir("mode");
        let destination = dir.join("result.hwpx");
        fs::write(&destination, b"ORIGINAL").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();

        write_validated(
            &destination,
            None,
            |staged| {
                fs::write(staged, b"NEW")?;
                Ok(())
            },
            |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"NEW");
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_destination_mode_change_aborts_without_clobbering_it() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = test_dir("mode-race");
        let destination = dir.join("result.hwpx");
        fs::write(&destination, b"ORIGINAL").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();

        let result = write_validated(
            &destination,
            None,
            |staged| {
                fs::write(staged, b"NEW")?;
                fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))?;
                Ok(())
            },
            |_, _| Ok(()),
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("권한/ACL"));
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL");
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    fn staged_file_set(destinations: &[PathBuf]) -> Vec<StagedOutput> {
        destinations
            .iter()
            .enumerate()
            .map(|(index, destination)| {
                let staged = StagedOutput::new(destination, None).unwrap();
                fs::write(staged.path(), format!("NEW-{index}")).unwrap();
                staged.sync_file().unwrap();
                staged
            })
            .collect()
    }

    #[test]
    fn output_set_late_failure_restores_all_existing_files() {
        let dir = test_dir("set-existing-rollback");
        let destinations = [dir.join("page-1.png"), dir.join("page-2.png")];
        fs::write(&destinations[0], b"OLD-0").unwrap();
        fs::write(&destinations[1], b"OLD-1").unwrap();
        let mut staged = staged_file_set(&destinations);

        let result = publish_output_set(&mut staged, |step| {
            if step == SetPublishStep::Publish(1) {
                anyhow::bail!("강제 두 번째 게시 실패");
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&destinations[0]).unwrap(), b"OLD-0");
        assert_eq!(fs::read(&destinations[1]).unwrap(), b"OLD-1");
        drop(staged);
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn output_set_late_failure_removes_all_new_files() {
        let dir = test_dir("set-missing-rollback");
        let destinations = [dir.join("page-1.svg"), dir.join("page-2.svg")];
        let mut staged = staged_file_set(&destinations);

        let result = publish_output_set(&mut staged, |step| {
            if step == SetPublishStep::Publish(1) {
                anyhow::bail!("강제 두 번째 게시 실패");
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(!destinations[0].exists());
        assert!(!destinations[1].exists());
        drop(staged);
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn output_set_restore_failure_preserves_backup() {
        let dir = test_dir("set-restore-backup");
        let destinations = [dir.join("page-1.png"), dir.join("page-2.png")];
        fs::write(&destinations[0], b"OLD-0").unwrap();
        fs::write(&destinations[1], b"OLD-1").unwrap();
        let mut staged = staged_file_set(&destinations);
        let backup = staged[0].workspace.join("destination.backup");

        let result = publish_output_set(&mut staged, |step| match step {
            SetPublishStep::Publish(1) => anyhow::bail!("강제 두 번째 게시 실패"),
            SetPublishStep::Restore(0) => anyhow::bail!("강제 첫 번째 복원 실패"),
            _ => Ok(()),
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&backup).unwrap(), b"OLD-0");
        assert_eq!(fs::read(&destinations[1]).unwrap(), b"OLD-1");
        staged[0].preserve_workspace = false;
        drop(staged);
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn output_set_preflight_rejects_duplicate_and_race_before_mutation() {
        let dir = test_dir("set-preflight");
        let first = dir.join("page-1.png");
        let second = dir.join("page-2.png");
        fs::write(&first, b"OLD-0").unwrap();
        fs::write(&second, b"OLD-1").unwrap();

        let duplicate = vec![
            (first.clone(), b"NEW-A".to_vec()),
            (first.clone(), b"NEW-B".to_vec()),
        ];
        assert!(write_validated_files(&duplicate, None).is_err());
        assert_eq!(fs::read(&first).unwrap(), b"OLD-0");

        let destinations = [first.clone(), second.clone()];
        let mut staged = staged_file_set(&destinations);
        fs::write(&second, b"RACER").unwrap();
        assert!(publish_output_set(&mut staged, |_| Ok(())).is_err());
        assert_eq!(fs::read(&first).unwrap(), b"OLD-0");
        assert_eq!(fs::read(&second).unwrap(), b"RACER");
        drop(staged);
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn output_set_preflight_rejects_symlink_without_touching_other_outputs() {
        use std::os::unix::fs::symlink;

        let dir = test_dir("set-symlink");
        let first = dir.join("page-1.png");
        let target = dir.join("target.png");
        let second = dir.join("page-2.png");
        fs::write(&first, b"OLD-0").unwrap();
        fs::write(&target, b"TARGET").unwrap();
        symlink(&target, &second).unwrap();
        let outputs = vec![
            (first.clone(), b"NEW-0".to_vec()),
            (second, b"NEW-1".to_vec()),
        ];
        assert!(write_validated_files(&outputs, None).is_err());
        assert_eq!(fs::read(&first).unwrap(), b"OLD-0");
        assert_eq!(fs::read(&target).unwrap(), b"TARGET");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sidecar_extraction_failure_preserves_body_and_existing_media() {
        let dir = test_dir("sidecar-writer-failure");
        let destination = dir.join("report.md");
        let media = dir.join("report.media");
        fs::write(&destination, b"ORIGINAL BODY").unwrap();
        fs::create_dir(&media).unwrap();
        fs::write(media.join("image1.png"), b"ORIGINAL IMAGE").unwrap();

        let result = write_validated_with_sidecar(
            &destination,
            None,
            &media,
            |staged, staged_media| -> anyhow::Result<()> {
                fs::write(staged, b"NEW BODY")?;
                fs::write(staged_media.join("image2.png"), b"PARTIAL IMAGE")?;
                anyhow::bail!("강제 미디어 추출 실패")
            },
            |_, _, _| Ok(()),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL BODY");
        assert_eq!(
            fs::read(media.join("image1.png")).unwrap(),
            b"ORIGINAL IMAGE"
        );
        assert!(!media.join("image2.png").exists());
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sidecar_destination_race_aborts_both_outputs() {
        let dir = test_dir("sidecar-race");
        let destination = dir.join("report.md");
        let media = dir.join("report.media");
        fs::write(&destination, b"ORIGINAL BODY").unwrap();
        fs::create_dir(&media).unwrap();
        fs::write(media.join("image1.png"), b"ORIGINAL IMAGE").unwrap();

        let result = write_validated_with_sidecar(
            &destination,
            None,
            &media,
            |staged, staged_media| {
                fs::write(staged, b"NEW BODY")?;
                fs::write(staged_media.join("image2.png"), b"NEW IMAGE")?;
                fs::write(media.join("image1.png"), b"RACER IMAGE")?;
                Ok(())
            },
            |_, _, _| Ok(()),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL BODY");
        assert_eq!(fs::read(media.join("image1.png")).unwrap(), b"RACER IMAGE");
        assert!(!media.join("image2.png").exists());
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sidecar_same_file_content_race_is_detected_by_digest() {
        let dir = test_dir("sidecar-same-file-race");
        let destination = dir.join("report.md");
        let media = dir.join("report.media");
        let image = media.join("image1.png");
        fs::write(&destination, b"ORIGINAL BODY").unwrap();
        fs::create_dir(&media).unwrap();
        fs::write(&image, b"ORIGINAL").unwrap();
        let original_modified = fs::metadata(&image).unwrap().modified().unwrap();

        let result = write_validated_with_sidecar(
            &destination,
            None,
            &media,
            |staged, staged_media| {
                fs::write(staged, b"NEW BODY")?;
                fs::write(staged_media.join("image2.png"), b"NEW IMAGE")?;
                fs::write(&image, b"MUTATION")?;
                fs::OpenOptions::new()
                    .write(true)
                    .open(&image)?
                    .set_times(fs::FileTimes::new().set_modified(original_modified))?;
                Ok(())
            },
            |_, _, _| Ok(()),
        );

        assert!(format!("{:#}", result.unwrap_err()).contains("미디어 디렉터리 내용이 바뀌어"));
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL BODY");
        assert_eq!(fs::read(&image).unwrap(), b"MUTATION");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_root_mode_race_aborts_both_outputs() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = test_dir("sidecar-root-mode-race");
        let destination = dir.join("report.md");
        let media = dir.join("report.media");
        fs::write(&destination, b"ORIGINAL BODY").unwrap();
        fs::create_dir(&media).unwrap();
        fs::write(media.join("image1.png"), b"ORIGINAL IMAGE").unwrap();
        fs::set_permissions(&media, fs::Permissions::from_mode(0o750)).unwrap();

        let result = write_validated_with_sidecar(
            &destination,
            None,
            &media,
            |staged, staged_media| {
                fs::write(staged, b"NEW BODY")?;
                fs::write(staged_media.join("image2.png"), b"NEW IMAGE")?;
                fs::set_permissions(&media, fs::Permissions::from_mode(0o700))?;
                Ok(())
            },
            |_, _, _| Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL BODY");
        assert!(!media.join("image2.png").exists());
        assert_eq!(
            fs::metadata(&media).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sidecar_publish_supports_new_and_existing_destinations() {
        let dir = test_dir("sidecar-success");
        let destination = dir.join("report.md");
        let media = dir.join("report.media");

        write_validated_with_sidecar(
            &destination,
            None,
            &media,
            |staged, staged_media| {
                fs::write(staged, b"FIRST BODY")?;
                fs::create_dir(staged_media)?;
                fs::write(staged_media.join("image1.png"), b"FIRST IMAGE")?;
                Ok(())
            },
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"FIRST BODY");
        assert_eq!(fs::read(media.join("image1.png")).unwrap(), b"FIRST IMAGE");

        fs::write(media.join("keep.txt"), b"KEEP").unwrap();
        write_validated_with_sidecar(
            &destination,
            None,
            &media,
            |staged, staged_media| {
                fs::write(staged, b"SECOND BODY")?;
                // 기존 tree를 복제하므로 동일 미디어와 관계없는 파일을 그대로 유지한다.
                assert_eq!(fs::read(staged_media.join("image1.png"))?, b"FIRST IMAGE");
                fs::write(staged_media.join("image2.png"), b"SECOND IMAGE")?;
                Ok(())
            },
            |_, _, _| Ok(()),
        )
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"SECOND BODY");
        assert_eq!(fs::read(media.join("image1.png")).unwrap(), b"FIRST IMAGE");
        assert_eq!(fs::read(media.join("image2.png")).unwrap(), b"SECOND IMAGE");
        assert_eq!(fs::read(media.join("keep.txt")).unwrap(), b"KEEP");
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn late_body_publish_failure_rolls_back_existing_media() {
        let dir = test_dir("sidecar-late-failure");
        let destination = dir.join("report.md");
        let media = dir.join("report.media");
        fs::write(&destination, b"ORIGINAL BODY").unwrap();
        fs::create_dir(&media).unwrap();
        fs::write(media.join("image1.png"), b"ORIGINAL IMAGE").unwrap();

        let mut staged = StagedOutputPair::new(&destination, None, &media).unwrap();
        fs::write(staged.file.path(), b"NEW BODY").unwrap();
        fs::write(staged.sidecar.path().join("image2.png"), b"NEW IMAGE").unwrap();
        staged.file.sync_file().unwrap();
        staged.sidecar.finish_write().unwrap();
        let result = staged.publish_with_hook(|step| {
            if step == PairPublishStep::PublishFile {
                anyhow::bail!("강제 본문 게시 실패");
            }
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL BODY");
        assert_eq!(
            fs::read(media.join("image1.png")).unwrap(),
            b"ORIGINAL IMAGE"
        );
        assert!(!media.join("image2.png").exists());
        drop(staged);
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_sidecar_restore_retains_recovery_backup() {
        let dir = test_dir("sidecar-backup-retained");
        let destination = dir.join("report.md");
        let media = dir.join("report.media");
        fs::write(&destination, b"ORIGINAL BODY").unwrap();
        fs::create_dir(&media).unwrap();
        fs::write(media.join("image1.png"), b"ORIGINAL IMAGE").unwrap();

        let mut staged = StagedOutputPair::new(&destination, None, &media).unwrap();
        let sidecar_workspace = staged.sidecar.workspace.clone();
        let backup = sidecar_workspace.join("destination.backup");
        fs::write(staged.file.path(), b"NEW BODY").unwrap();
        fs::write(staged.sidecar.path().join("image2.png"), b"NEW IMAGE").unwrap();
        staged.file.sync_file().unwrap();
        staged.sidecar.finish_write().unwrap();
        let result = staged.publish_with_hook(|step| match step {
            PairPublishStep::PublishFile => anyhow::bail!("강제 본문 게시 실패"),
            PairPublishStep::RestoreSidecar => anyhow::bail!("강제 미디어 복원 실패"),
            _ => Ok(()),
        });

        assert!(result.is_err());
        assert!(backup.is_dir(), "복구용 기존 미디어 backup 보존");
        assert_eq!(
            fs::read(backup.join("image1.png")).unwrap(),
            b"ORIGINAL IMAGE"
        );
        assert_eq!(fs::read(&destination).unwrap(), b"ORIGINAL BODY");
        staged.sidecar.preserve_workspace = false;
        drop(staged);
        assert_no_debris(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_hardlink_and_special_destinations_are_rejected() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let dir = test_dir("unsafe-targets");
        let original = dir.join("original.hwpx");
        fs::write(&original, b"ORIGINAL").unwrap();

        let symlink_path = dir.join("symlink.hwpx");
        symlink(&original, &symlink_path).unwrap();
        assert!(
            write_validated(
                &symlink_path,
                None,
                |staged| fs::write(staged, b"NEW").map_err(Into::into),
                |_, _| Ok(())
            )
            .is_err()
        );

        let hardlink_path = dir.join("hardlink.hwpx");
        fs::hard_link(&original, &hardlink_path).unwrap();
        assert!(
            write_validated(
                &hardlink_path,
                None,
                |staged| fs::write(staged, b"NEW").map_err(Into::into),
                |_, _| Ok(())
            )
            .is_err()
        );

        let socket_path = dir.join("socket.hwpx");
        let _listener = UnixListener::bind(&socket_path).unwrap();
        assert!(
            write_validated(
                &socket_path,
                None,
                |staged| fs::write(staged, b"NEW").map_err(Into::into),
                |_, _| Ok(())
            )
            .is_err()
        );
        assert_eq!(fs::read(&original).unwrap(), b"ORIGINAL");
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn aliased_input_is_rejected_but_exact_in_place_is_allowed() {
        let dir = test_dir("aliases");
        let document = dir.join("document.hwpx");
        fs::write(&document, b"ORIGINAL").unwrap();

        write_validated(
            &document,
            Some(&document),
            |staged| fs::write(staged, b"NEW").map_err(Into::into),
            |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(fs::read(&document).unwrap(), b"NEW");

        let lexical_alias = dir.join(".").join("document.hwpx");
        write_validated(
            &document,
            Some(&lexical_alias),
            |staged| fs::write(staged, b"NEWER").map_err(Into::into),
            |_, _| Ok(()),
        )
        .unwrap();
        assert_eq!(fs::read(&document).unwrap(), b"NEWER");

        let alias = dir.join("alias.hwpx");
        fs::hard_link(&document, &alias).unwrap();
        assert!(
            write_validated(
                &alias,
                Some(&document),
                |staged| fs::write(staged, b"BAD").map_err(Into::into),
                |_, _| Ok(())
            )
            .is_err()
        );
        assert_eq!(fs::read(&document).unwrap(), b"NEWER");
        fs::remove_dir_all(dir).unwrap();
    }

    struct FaultFs {
        rename_calls: Cell<usize>,
        fail_rename_calls: Vec<usize>,
        copy_fails: bool,
        remove_fails: bool,
        removed: RefCell<Vec<PathBuf>>,
    }

    impl RecoveryFs for FaultFs {
        fn exists(&self, _path: &Path) -> bool {
            true
        }

        fn rename(&self, _from: &Path, _to: &Path) -> std::io::Result<()> {
            let call = self.rename_calls.get() + 1;
            self.rename_calls.set(call);
            if self.fail_rename_calls.contains(&call) {
                Err(std::io::Error::other(format!("rename {call} 실패")))
            } else {
                Ok(())
            }
        }

        fn copy(&self, _from: &Path, _to: &Path) -> std::io::Result<u64> {
            if self.copy_fails {
                Err(std::io::Error::other("copy 실패"))
            } else {
                Ok(8)
            }
        }

        fn remove_file(&self, path: &Path) -> std::io::Result<()> {
            self.removed.borrow_mut().push(path.to_path_buf());
            if self.remove_fails {
                Err(std::io::Error::other("remove 실패"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn non_unix_recovery_preserves_backup_when_restore_rename_fails() {
        let ops = FaultFs {
            rename_calls: Cell::new(0),
            // 1: destination->backup 성공, 2: publish 실패, 3: restore rename 실패.
            fail_rename_calls: vec![2, 3],
            copy_fails: false,
            remove_fails: false,
            removed: RefCell::new(Vec::new()),
        };
        let backup = Path::new("/work/destination.backup");
        let state = publish_with_recovery(
            &ops,
            Path::new("/work/staged.hwpx"),
            Path::new("/work/result.hwpx"),
            backup,
        );
        let RecoveryState::FailedBackupPreserved {
            error,
            backup: retained,
        } = state
        else {
            panic!("백업 보존 상태여야 함");
        };
        assert!(error.to_string().contains("복사로 복원"));
        assert_eq!(retained, backup);
        assert!(
            ops.removed.borrow().is_empty(),
            "복구 rename 실패 후 백업을 삭제하면 안 됨"
        );
    }

    #[test]
    fn non_unix_recovery_reports_restored_state() {
        let ops = FaultFs {
            rename_calls: Cell::new(0),
            // 1: backup 성공, 2: publish 실패, 3: restore 성공.
            fail_rename_calls: vec![2],
            copy_fails: false,
            remove_fails: false,
            removed: RefCell::new(Vec::new()),
        };
        let state = publish_with_recovery(
            &ops,
            Path::new("/work/staged.hwpx"),
            Path::new("/work/result.hwpx"),
            Path::new("/work/destination.backup"),
        );
        let RecoveryState::FailedRestored { error } = state else {
            panic!("복원 완료 실패 상태여야 함");
        };
        assert!(error.to_string().contains("기존 파일을 복원"));

        let success_ops = FaultFs {
            rename_calls: Cell::new(0),
            fail_rename_calls: Vec::new(),
            copy_fails: false,
            remove_fails: false,
            removed: RefCell::new(Vec::new()),
        };
        let success = publish_with_recovery(
            &success_ops,
            Path::new("/work/staged.hwpx"),
            Path::new("/work/result.hwpx"),
            Path::new("/work/destination.backup"),
        );
        let RecoveryState::Published { warning } = success else {
            panic!("게시 성공 상태여야 함");
        };
        assert!(warning.is_none());

        let cleanup_failure_ops = FaultFs {
            rename_calls: Cell::new(0),
            fail_rename_calls: Vec::new(),
            copy_fails: false,
            remove_fails: true,
            removed: RefCell::new(Vec::new()),
        };
        let cleanup_failure = publish_with_recovery(
            &cleanup_failure_ops,
            Path::new("/work/staged.hwpx"),
            Path::new("/work/result.hwpx"),
            Path::new("/work/destination.backup"),
        );
        let RecoveryState::Published { warning } = cleanup_failure else {
            panic!("백업 정리 실패도 게시 자체는 성공이어야 함");
        };
        assert!(warning.is_some());
    }
}
