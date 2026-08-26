//! Owner-only seven-case password corpus harness.
//!
//! Genuine sources, credentials and published receipts remain outside the
//! repository. This module deliberately reports only fixed, non-sensitive
//! failure categories and never includes a path or credential in output.

#[allow(dead_code)]
#[path = "password_corpus_manifest.rs"]
mod password_corpus_manifest;
#[allow(dead_code)]
#[path = "password_receipt.rs"]
mod password_receipt;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use zeroize::{Zeroize as _, Zeroizing};

use password_corpus_manifest::{load_manifest_for_test, repo_root};
use password_receipt::{
    ExpectedFixture, PasswordDecryptionReceipt, ReceiptCase, validate_complete_run,
    validate_complete_run_for_binary, write_receipt_atomic,
};

const PROFILE_EVIDENCE_VERSION: &str = "password-profile-evidence-v1";
const HWP5_ALGORITHM: &str = "hwp5-encrypt-version-4";
const HWP5_KDF: &str = "hwp5-password-transform";

#[derive(Debug, PartialEq, Eq)]
pub enum CorpusRun {
    Skipped,
    Completed,
}

struct GenuineFixture {
    fixture_id: String,
    source_path: PathBuf,
    format: String,
    role: String,
    credential: Zeroizing<String>,
    expected: ExpectedFixture,
}

pub fn run_password_corpus_from_env() -> Result<(), String> {
    let manifest = std::env::var_os("HWP_PASSWORD_CORPUS_MANIFEST").map(PathBuf::from);
    let receipt_dir = std::env::var_os("HWP_PASSWORD_RECEIPT_DIR").map(PathBuf::from);
    match run_password_corpus(manifest, receipt_dir)? {
        CorpusRun::Skipped => {
            eprintln!("password corpus skipped: manifest is not configured");
            Ok(())
        }
        CorpusRun::Completed => Ok(()),
    }
}

pub fn run_password_corpus(
    manifest_path: Option<PathBuf>,
    receipt_dir: Option<PathBuf>,
) -> Result<CorpusRun, String> {
    let Some(manifest_path) = manifest_path else {
        return Ok(CorpusRun::Skipped);
    };
    if !manifest_path.is_absolute() || !manifest_path.is_file() {
        return Err("password corpus manifest is unavailable".into());
    }
    let manifest = read_manifest(&manifest_path)?;
    let repository_root = repo_root();
    load_manifest_for_test(&manifest, &repository_root, |reference| {
        std::env::var(reference).ok()
    })
    .map_err(|_| "password corpus manifest is incomplete".to_owned())?;

    let receipt_dir =
        receipt_dir.ok_or_else(|| "password receipt directory is not configured".to_owned())?;
    let receipt_dir = require_empty_external_receipt_dir(&receipt_dir, &repository_root)?;
    let evidence = read_profile_evidence()?;
    let fixtures = select_fixtures(&manifest, &evidence, &repository_root)?;
    let binary_sha256 = sha256_file(Path::new(env!("CARGO_BIN_EXE_hwp")))?;
    let binary_version = env!("CARGO_PKG_VERSION").to_owned();

    for fixture in &fixtures {
        let cases = if fixture.role == "baseline" {
            [
                Some(ReceiptCase::Correct),
                Some(ReceiptCase::Wrong),
                Some(ReceiptCase::Absent),
            ]
        } else {
            [Some(ReceiptCase::Correct), None, None]
        };
        for case in cases.into_iter().flatten() {
            run_case(fixture, case)?;
            let receipt = PasswordDecryptionReceipt::new(
                fixture.fixture_id.clone(),
                fixture.expected.source_sha256.clone(),
                fixture.format.clone(),
                fixture.expected.algorithm_id.clone(),
                fixture.expected.kdf_id.clone(),
                case,
                vec!["cat".into(), "convert".into(), "render".into()],
                binary_version.clone(),
                binary_sha256.clone(),
                case_result(case).into(),
                receipt_timestamp(),
            );
            write_receipt_atomic(&receipt_dir, &receipt)?;
        }
    }
    let expected = fixtures
        .iter()
        .map(|fixture| fixture.expected.clone())
        .collect::<Vec<_>>();
    validate_complete_run(&receipt_dir, &expected)?;
    Ok(CorpusRun::Completed)
}

pub fn validate_existing_complete_run_from_env() -> Result<(), String> {
    let manifest_path = std::env::var_os("HWP_PASSWORD_CORPUS_MANIFEST")
        .map(PathBuf::from)
        .ok_or_else(|| "password corpus manifest is not configured".to_owned())?;
    let receipt_dir = std::env::var_os("HWP_PASSWORD_RECEIPT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "password receipt directory is not configured".to_owned())?;
    let repository_root = repo_root();
    let receipt_dir = require_external_receipt_dir(&receipt_dir, &repository_root)?;
    let manifest = read_manifest(&manifest_path)?;
    load_manifest_for_test(&manifest, &repository_root, |reference| {
        std::env::var(reference).ok()
    })
    .map_err(|_| "password corpus manifest is incomplete".to_owned())?;
    let evidence = read_profile_evidence()?;
    let fixtures = select_fixtures(&manifest, &evidence, &repository_root)?;
    let expected = fixtures
        .iter()
        .map(|fixture| fixture.expected.clone())
        .collect::<Vec<_>>();
    let binary_sha256 = sha256_file(Path::new(env!("CARGO_BIN_EXE_hwp")))?;
    validate_complete_run_for_binary(
        &receipt_dir,
        &expected,
        env!("CARGO_PKG_VERSION"),
        &binary_sha256,
    )
}

fn read_manifest(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|_| "password corpus manifest cannot be read".to_owned())?;
    serde_json::from_slice(&bytes).map_err(|_| "password corpus manifest is invalid".to_owned())
}

fn require_empty_external_receipt_dir(
    receipt_dir: &Path,
    repository_root: &Path,
) -> Result<PathBuf, String> {
    let resolved = require_external_receipt_dir(receipt_dir, repository_root)?;
    if fs::read_dir(&resolved)
        .map_err(|_| "password receipt directory cannot be inspected".to_owned())?
        .next()
        .is_some()
    {
        return Err("password receipt directory must be an empty external directory".into());
    }
    Ok(resolved)
}

pub fn require_external_receipt_dir(
    receipt_dir: &Path,
    repository_root: &Path,
) -> Result<PathBuf, String> {
    if !receipt_dir.is_absolute() || !receipt_dir.is_dir() {
        return Err("password receipt directory must be external".into());
    }
    let resolved = receipt_dir
        .canonicalize()
        .map_err(|_| "password receipt directory must be external".to_owned())?;
    let repository = repository_root
        .canonicalize()
        .map_err(|_| "password receipt directory must be external".to_owned())?;
    if resolved.starts_with(repository) {
        return Err("password receipt directory must be external".into());
    }
    Ok(resolved)
}

fn read_profile_evidence() -> Result<Value, String> {
    let path = std::env::var_os("HWP_PASSWORD_PROFILE_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            repo_root()
                .join(".planning/phases/08-password-protected-input/08-PROFILE-EVIDENCE.json")
        });
    if !path.is_absolute() || !path.is_file() {
        return Err("password profile evidence is unavailable".into());
    }
    let bytes =
        fs::read(path).map_err(|_| "password profile evidence cannot be read".to_owned())?;
    let evidence: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "password profile evidence is invalid".to_owned())?;
    if evidence.get("version").and_then(Value::as_str) != Some(PROFILE_EVIDENCE_VERSION) {
        return Err("password profile evidence version is unsupported".into());
    }
    Ok(evidence)
}

fn select_fixtures(
    manifest: &Value,
    evidence: &Value,
    repository_root: &Path,
) -> Result<Vec<GenuineFixture>, String> {
    let fixtures = manifest
        .get("fixtures")
        .and_then(Value::as_array)
        .ok_or_else(|| "password corpus manifest is incomplete".to_owned())?;
    let evidence_fixtures = evidence
        .get("fixtures")
        .and_then(Value::as_array)
        .ok_or_else(|| "password profile evidence is incomplete".to_owned())?;
    let mut selected = Vec::new();
    for (role, format, charset) in [
        ("baseline", "hwp5", "ascii"),
        ("baseline", "hwpx", "ascii"),
        ("non_ascii_success", "hwpx", "non_ascii"),
    ] {
        let matches = fixtures
            .iter()
            .filter(|fixture| {
                fixture.get("role").and_then(Value::as_str) == Some(role)
                    && fixture.get("format").and_then(Value::as_str) == Some(format)
                    && fixture.get("credential_charset").and_then(Value::as_str) == Some(charset)
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err("password corpus manifest has invalid fixture roles".into());
        }
        selected.push(build_fixture(
            matches[0],
            evidence_fixtures,
            repository_root,
        )?);
    }
    if selected
        .iter()
        .map(|fixture| fixture.fixture_id.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != selected.len()
        || selected
            .iter()
            .map(|fixture| fixture.source_path.as_path())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != selected.len()
    {
        return Err("password corpus roles must use distinct fixtures".into());
    }
    Ok(selected)
}

fn build_fixture(
    fixture: &Value,
    evidence_fixtures: &[Value],
    repository_root: &Path,
) -> Result<GenuineFixture, String> {
    let text = |name: &str| {
        fixture
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| "password corpus manifest is incomplete".to_owned())
    };
    let fixture_id = text("fixture_id")?.to_owned();
    let source_path = PathBuf::from(text("source_path")?);
    let format = text("format")?.to_owned();
    let role = text("role")?.to_owned();
    let credential_charset = text("credential_charset")?.to_owned();
    let credential_ref = text("credential_ref")?;
    let source_path = require_external_corpus_source(&source_path, repository_root)?;
    let credential = std::env::var(credential_ref)
        .map(Zeroizing::new)
        .map_err(|_| "password corpus credential is unavailable".to_owned())?;
    let source_sha256 = sha256_file(&source_path)?;
    let evidence = evidence_fixtures
        .iter()
        .find(|entry| entry.get("fixture_id").and_then(Value::as_str) == Some(&fixture_id))
        .ok_or_else(|| "password profile evidence does not match the manifest".to_owned())?;
    if evidence.get("format").and_then(Value::as_str) != Some(format.as_str())
        || evidence.get("role").and_then(Value::as_str) != Some(role.as_str())
        || evidence.get("credential_charset").and_then(Value::as_str)
            != Some(credential_charset.as_str())
        || evidence.get("source_sha256").and_then(Value::as_str) != Some(source_sha256.as_str())
    {
        return Err("password profile evidence drifted from the owner corpus".into());
    }
    let (algorithm_id, kdf_id) = expected_profile(evidence, &format)?;
    let expected = ExpectedFixture {
        fixture_id: fixture_id.clone(),
        format: format.clone(),
        role: role.clone(),
        credential_charset: credential_charset.clone(),
        source_sha256,
        algorithm_id,
        kdf_id,
    };
    Ok(GenuineFixture {
        fixture_id,
        source_path,
        format,
        role,
        credential,
        expected,
    })
}

pub fn require_external_corpus_source(
    source_path: &Path,
    repository_root: &Path,
) -> Result<PathBuf, String> {
    if !source_path.is_absolute() || !source_path.is_file() {
        return Err("password corpus source is unavailable".into());
    }
    let resolved = source_path
        .canonicalize()
        .map_err(|_| "password corpus source is unavailable".to_owned())?;
    let repository = repository_root
        .canonicalize()
        .map_err(|_| "password corpus source is unavailable".to_owned())?;
    if resolved.starts_with(repository) {
        return Err("password corpus source is unavailable".into());
    }
    Ok(resolved)
}

fn expected_profile(evidence: &Value, format: &str) -> Result<(String, Option<String>), String> {
    let profile = evidence
        .get("profile")
        .ok_or_else(|| "password profile evidence is incomplete".to_owned())?;
    match format {
        "hwp5"
            if profile.get("format").and_then(Value::as_str) == Some("hwp5")
                && profile.get("encrypt_version").and_then(Value::as_u64) == Some(4) =>
        {
            Ok((HWP5_ALGORITHM.into(), Some(HWP5_KDF.into())))
        }
        "hwpx" if profile.get("format").and_then(Value::as_str) == Some("hwpx") => {
            let algorithm = profile
                .get("algorithm_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "password profile evidence is incomplete".to_owned())?;
            let kdf = profile
                .get("kdf_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "password profile evidence is incomplete".to_owned())?;
            Ok((algorithm.to_owned(), Some(kdf.to_owned())))
        }
        _ => Err("password profile evidence is unsupported".into()),
    }
}

fn run_case(fixture: &GenuineFixture, case: ReceiptCase) -> Result<(), String> {
    let output_dir = unique_output_dir()?;
    let result = (|| {
        let cat = run_password_command(cat_command(fixture), &fixture.credential, case)?;
        let converted = output_dir.join("converted.md");
        let convert = run_password_command(
            convert_command(fixture, &converted),
            &fixture.credential,
            case,
        )?;
        let rendered = output_dir.join("rendered.svg");
        let report = output_dir.join("rendered-report.json");
        let render = run_password_command(
            render_command(fixture, &rendered, &report),
            &fixture.credential,
            case,
        )?;
        match case {
            ReceiptCase::Correct => {
                if !cat.status.success()
                    || cat.stdout.is_empty()
                    || !convert.status.success()
                    || !non_empty_file(&converted)
                    || !render.status.success()
                    || !non_empty_file(&rendered)
                {
                    return Err(
                        "password corpus correct credential did not recover all surfaces".into(),
                    );
                }
            }
            ReceiptCase::Wrong | ReceiptCase::Absent => {
                if cat.status.success()
                    || convert.status.success()
                    || render.status.success()
                    || cat.status.code() != convert.status.code()
                    || convert.status.code() != render.status.code()
                    || converted.exists()
                    || rendered.exists()
                    || report.exists()
                {
                    return Err("password corpus refusal contract was not preserved".into());
                }
            }
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&output_dir);
    result
}

fn cat_command(fixture: &GenuineFixture) -> Command {
    let mut command = hwp();
    command.arg("cat").arg(&fixture.source_path);
    command
}

fn convert_command(fixture: &GenuineFixture, output: &Path) -> Command {
    let mut command = hwp();
    command
        .arg("convert")
        .arg(&fixture.source_path)
        .arg("-o")
        .arg(output);
    command
}

fn render_command(fixture: &GenuineFixture, output: &Path, report: &Path) -> Command {
    let mut command = hwp();
    command
        .arg("render")
        .arg(&fixture.source_path)
        .arg("-o")
        .arg(output)
        .arg("--report")
        .arg(report);
    command
}

fn run_password_command(
    mut command: Command,
    credential: &Zeroizing<String>,
    case: ReceiptCase,
) -> Result<Output, String> {
    match case {
        ReceiptCase::Absent => command
            .output()
            .map_err(|_| "password corpus command could not run".into()),
        ReceiptCase::Correct | ReceiptCase::Wrong => {
            command
                .arg("--password-stdin")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command
                .spawn()
                .map_err(|_| "password corpus command could not run".to_owned())?;
            let mut supplied = match case {
                ReceiptCase::Correct => Zeroizing::new(format!("{}\n", credential.as_str())),
                ReceiptCase::Wrong => wrong_credential(credential),
                ReceiptCase::Absent => unreachable!(),
            };
            child
                .stdin
                .take()
                .ok_or_else(|| "password corpus command stdin is unavailable".to_owned())?
                .write_all(supplied.as_bytes())
                .map_err(|_| "password corpus command stdin cannot be written".to_owned())?;
            supplied.zeroize();
            child
                .wait_with_output()
                .map_err(|_| "password corpus command did not finish".into())
        }
    }
}

fn wrong_credential(credential: &Zeroizing<String>) -> Zeroizing<String> {
    let mut wrong = Zeroizing::new(credential.to_string());
    wrong.push('\u{0001}');
    wrong.push('\n');
    wrong
}

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn unique_output_dir() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "hwp-cli-password-corpus-output-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "password corpus clock is unavailable".to_owned())?
            .as_nanos()
    ));
    fs::create_dir(&path)
        .map_err(|_| "password corpus output directory cannot be created".to_owned())?;
    Ok(path)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|_| "password corpus source cannot be hashed".to_owned())?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn non_empty_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn case_result(case: ReceiptCase) -> &'static str {
    match case {
        ReceiptCase::Correct => "pass",
        ReceiptCase::Wrong | ReceiptCase::Absent => "refused",
    }
}

fn receipt_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let days = (seconds / 86_400) as i64;
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_date_from_unix_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remainder / 3_600,
        (remainder % 3_600) / 60,
        remainder % 60
    )
}

fn civil_date_from_unix_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = (month_index + if month_index < 10 { 3 } else { -9 }) as u32;
    year += if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn password_stdin_captures_child_output() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "IFS= read -r _secret; printf captured-stdout; printf captured-stderr >&2",
        ]);
        let credential = Zeroizing::new("synthetic-secret".to_owned());

        let output = run_password_command(command, &credential, ReceiptCase::Correct).unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"captured-stdout");
        assert_eq!(output.stderr, b"captured-stderr");
    }
}
