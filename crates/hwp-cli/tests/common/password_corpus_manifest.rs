//! Test-only validation for the owner-controlled password corpus contract.
//!
//! The helper deliberately returns redacted descriptors. It never exposes a
//! source path or serializes the resolved `Zeroizing<String>` credential.

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

const MANIFEST_SCHEMA: &str =
    include_str!("../../../../schemas/password-corpus-manifest-v1.schema.json");

pub struct RedactedDescriptor {
    pub fixture_id: String,
    pub format: String,
    pub role: String,
    pub credential_charset: String,
    pub source_path: Option<PathBuf>,
    pub credential: Option<Zeroizing<String>>,
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root is available for test validation")
}

pub fn manifest_fixture() -> Value {
    json!({
        "version": "password-corpus-manifest-v1",
        "fixtures": [
            {
                "fixture_id": "opaque-hwp5-baseline",
                "source_path": "/private/password-corpus/hwp5-baseline.hwp",
                "format": "hwp5",
                "role": "baseline",
                "credential_charset": "ascii",
                "credential_ref": "HWP_PASSWORD_HWP5_BASELINE"
            },
            {
                "fixture_id": "opaque-hwpx-baseline",
                "source_path": "/private/password-corpus/hwpx-baseline.hwpx",
                "format": "hwpx",
                "role": "baseline",
                "credential_charset": "ascii",
                "credential_ref": "HWP_PASSWORD_HWPX_BASELINE"
            },
            {
                "fixture_id": "opaque-unicode-success",
                "source_path": "/private/password-corpus/unicode-success.hwpx",
                "format": "hwpx",
                "role": "non_ascii_success",
                "credential_charset": "non_ascii",
                "credential_ref": "HWP_PASSWORD_UNICODE_SUCCESS"
            }
        ]
    })
}

pub fn invalid_manifest_cases() -> Vec<(&'static str, Value)> {
    let base = manifest_fixture();
    let mut unknown_field = base.clone();
    unknown_field["password"] = json!("must-not-be-accepted");

    let mut expected_text = base.clone();
    expected_text["fixtures"][0]["expected_text"] = json!("must-not-be-accepted");

    let mut duplicate_id = base.clone();
    duplicate_id["fixtures"][1]["fixture_id"] = base["fixtures"][0]["fixture_id"].clone();

    let mut repo_path = base.clone();
    repo_path["fixtures"][0]["source_path"] = json!(repo_root().join("fixtures/sample.hwp"));

    let mut missing_hwp5 = base.clone();
    missing_hwp5["fixtures"].as_array_mut().unwrap().remove(0);
    missing_hwp5["fixtures"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "fixture_id": "other-hwpx-baseline",
            "source_path": "/private/password-corpus/other.hwpx",
            "format": "hwpx",
            "role": "baseline",
            "credential_charset": "ascii",
            "credential_ref": "HWP_PASSWORD_OTHER"
        }));

    let mut missing_non_ascii = base.clone();
    missing_non_ascii["fixtures"]
        .as_array_mut()
        .unwrap()
        .remove(2);
    missing_non_ascii["fixtures"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "fixture_id": "second-hwp5-baseline",
            "source_path": "/private/password-corpus/second.hwp",
            "format": "hwp5",
            "role": "baseline",
            "credential_charset": "ascii",
            "credential_ref": "HWP_PASSWORD_SECOND"
        }));

    let mut non_ascii_baseline = base.clone();
    non_ascii_baseline["fixtures"][0]["credential_charset"] = json!("non_ascii");

    let mut ascii_non_ascii_success = base.clone();
    ascii_non_ascii_success["fixtures"][2]["credential_charset"] = json!("ascii");

    let mut role_reuse = base.clone();
    role_reuse["fixtures"][2]["fixture_id"] = base["fixtures"][0]["fixture_id"].clone();
    role_reuse["fixtures"][2]["source_path"] = base["fixtures"][0]["source_path"].clone();

    vec![
        ("unknown root field", unknown_field),
        ("forbidden expected text", expected_text),
        ("duplicate fixture id", duplicate_id),
        ("repository-contained source path", repo_path),
        ("missing HWP5 baseline", missing_hwp5),
        ("missing non-ASCII success", missing_non_ascii),
        ("non-ASCII baseline", non_ascii_baseline),
        ("ASCII non-ASCII-success", ascii_non_ascii_success),
        ("baseline/non-ASCII role reuse", role_reuse),
    ]
}

pub fn load_manifest_for_test<F>(
    manifest: &Value,
    repository_root: &Path,
    resolve_credential: F,
) -> Result<Vec<RedactedDescriptor>, String>
where
    F: Fn(&str) -> Option<String>,
{
    let schema: Value = serde_json::from_str(MANIFEST_SCHEMA)
        .map_err(|_| "password corpus schema is invalid".to_owned())?;
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .map_err(|_| "password corpus schema cannot be compiled".to_owned())?;
    if !validator.is_valid(manifest) {
        return Err("password corpus manifest does not satisfy the closed schema".to_owned());
    }

    let fixtures = manifest["fixtures"]
        .as_array()
        .ok_or_else(|| "password corpus fixtures are unavailable".to_owned())?;
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut baseline_formats = BTreeSet::new();
    let mut non_ascii_successes = 0usize;
    let mut descriptors = Vec::with_capacity(fixtures.len());

    for fixture in fixtures {
        let fixture_id = fixture["fixture_id"]
            .as_str()
            .ok_or_else(|| "fixture identifier is unavailable".to_owned())?;
        let source_path = PathBuf::from(
            fixture["source_path"]
                .as_str()
                .ok_or_else(|| "fixture source path is unavailable".to_owned())?,
        );
        let format = fixture["format"]
            .as_str()
            .ok_or_else(|| "fixture format is unavailable".to_owned())?;
        let role = fixture["role"]
            .as_str()
            .ok_or_else(|| "fixture role is unavailable".to_owned())?;
        let charset = fixture["credential_charset"]
            .as_str()
            .ok_or_else(|| "credential charset is unavailable".to_owned())?;
        let reference = fixture["credential_ref"]
            .as_str()
            .ok_or_else(|| "credential reference is unavailable".to_owned())?;

        if !ids.insert(fixture_id.to_owned()) || !paths.insert(source_path.clone()) {
            return Err("fixture identifiers and source paths must be distinct".to_owned());
        }
        if source_path.starts_with(repository_root) {
            return Err("fixture source paths must stay outside the repository".to_owned());
        }
        match role {
            "baseline" if charset == "ascii" => {
                baseline_formats.insert(format.to_owned());
            }
            "non_ascii_success" if charset == "non_ascii" => {
                non_ascii_successes += 1;
            }
            _ => return Err("fixture role and credential charset are incompatible".to_owned()),
        }

        let credential = resolve_credential(reference)
            .map(Zeroizing::new)
            .ok_or_else(|| "credential reference did not resolve".to_owned())?;
        descriptors.push(RedactedDescriptor {
            fixture_id: fixture_id.to_owned(),
            format: format.to_owned(),
            role: role.to_owned(),
            credential_charset: charset.to_owned(),
            source_path: None,
            credential: Some(credential),
        });
    }

    if !baseline_formats.contains("hwp5") || !baseline_formats.contains("hwpx") {
        return Err("both ASCII baseline formats are required".to_owned());
    }
    if non_ascii_successes == 0 {
        return Err("a distinct non-ASCII success fixture is required".to_owned());
    }
    Ok(descriptors)
}

pub fn discovery_would_write_evidence(manifest_path: Option<&Path>) -> bool {
    manifest_path.is_some_and(Path::is_file)
}

#[derive(Serialize)]
struct ProfileEvidence {
    version: &'static str,
    budget_profile: BudgetProfile,
    fixtures: Vec<FixtureEvidence>,
}

#[derive(Serialize)]
struct BudgetProfile {
    hwp5_stream_bytes: u64,
    hwp5_live_bytes: u64,
    hwpx_entry_bytes: u64,
    hwpx_xml_bytes: u64,
    hwpx_live_bytes: u64,
    hwpx_compression_ratio: u64,
}

#[derive(Serialize)]
struct FixtureEvidence {
    fixture_id: String,
    format: String,
    role: String,
    credential_charset: String,
    source_sha256: String,
    profile: ProfileObservation,
}

#[derive(Serialize)]
#[serde(tag = "format", rename_all = "snake_case")]
enum ProfileObservation {
    Hwp5 {
        encrypt_version: u32,
        cfb_stream_count: usize,
        cfb_stream_bytes: u64,
        validated_record_stream_count: usize,
        validated_record_streams: Vec<Hwp5ValidatedStream>,
    },
    Hwpx {
        algorithm_id: String,
        kdf_id: String,
        pbkdf2_prf: &'static str,
        start_key_id: String,
        checksum_id: String,
        checksum_scope: HwpxChecksumScope,
        cbc_padding: &'static str,
        protected_entry_count: usize,
        validated_entry_count: usize,
        protected_entry_bytes: u64,
    },
}

struct OwnerFixture {
    fixture_id: String,
    source_path: PathBuf,
    format: String,
    role: String,
    credential_charset: String,
    credential: Zeroizing<String>,
}

/// Runs the genuine owner-controlled discovery gate. The returned evidence is
/// deliberately serializable only through the closed types above: private
/// paths, credential references and credentials never cross this boundary.
pub fn run_owner_discovery_from_env() -> Result<(), String> {
    let manifest_path = owner_manifest_path(std::env::var_os("HWP_PASSWORD_CORPUS_MANIFEST"))?;
    let evidence_path = evidence_destination_from_env()?;
    run_owner_discovery(&manifest_path, &evidence_path, |reference| {
        std::env::var(reference).ok()
    })
}

#[derive(Clone, Copy)]
enum OwnerProfileDiagnosticOutcome {
    Validated,
    Hwp5Probe,
    Hwpx(HwpxProbeFailureStage),
}

impl OwnerProfileDiagnosticOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Validated => "validated",
            Self::Hwp5Probe => "hwp5_probe",
            Self::Hwpx(HwpxProbeFailureStage::CiphertextStorageProfile) => {
                "ciphertext_storage_profile"
            }
            Self::Hwpx(HwpxProbeFailureStage::KeyDerivationCipherInit) => {
                "key_derivation_cipher_init"
            }
            Self::Hwpx(HwpxProbeFailureStage::ZeroSuffix) => "zero_suffix",
            Self::Hwpx(HwpxProbeFailureStage::DeflateStream) => "deflate_stream",
            Self::Hwpx(HwpxProbeFailureStage::ChecksumScope) => "checksum_scope",
            Self::Hwpx(HwpxProbeFailureStage::DeclaredSize) => "declared_size",
            Self::Hwpx(HwpxProbeFailureStage::XmlStructure) => "xml_structure",
            Self::Hwpx(HwpxProbeFailureStage::Bounds) => "bounds",
        }
    }
}

/// Executes the owner-only diagnostic mode. The returned JSON lines are a
/// deliberately closed terminal contract: opaque fixture ID, closed format,
/// closed algorithm/KDF identifiers, and one outcome label. It neither writes
/// evidence nor exposes source paths, credential references, or file content.
pub fn run_owner_profile_diagnostic_from_env() -> Result<Vec<String>, String> {
    if std::env::var_os("HWP_PASSWORD_DISCOVERY_DIAGNOSTIC").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return Err("owner profile diagnostic is not explicitly enabled".to_owned());
    }
    let manifest_path = owner_manifest_path(std::env::var_os("HWP_PASSWORD_CORPUS_MANIFEST"))?;
    run_owner_profile_diagnostic(&manifest_path, |reference| std::env::var(reference).ok())
}

/// Parameterized test seam for the diagnostic redaction contract. Unlike the
/// discovery path, this never hashes a source or writes a receipt.
pub fn run_owner_profile_diagnostic<F>(
    manifest_path: &Path,
    resolve_credential: F,
) -> Result<Vec<String>, String>
where
    F: Fn(&str) -> Option<String>,
{
    if !manifest_path.is_absolute() || !manifest_path.is_file() {
        return Err("owner password corpus manifest is unavailable".to_owned());
    }
    let manifest_bytes = fs::read(manifest_path)
        .map_err(|_| "owner password corpus manifest cannot be read".to_owned())?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| "owner password corpus manifest is invalid".to_owned())?;
    load_manifest_for_test(&manifest, &repo_root(), |reference| {
        resolve_credential(reference)
    })?;
    let fixtures = owner_fixtures(&manifest, resolve_credential)?;

    fixtures
        .iter()
        .map(|fixture| {
            let (algorithm_id, kdf_id, checksum_scope, outcome) = match fixture.format.as_str() {
                "hwp5" => (
                    "hwp5-encrypt-version-4",
                    "hwp5-password-transform",
                    None,
                    if fixture.source_path.is_file()
                        && probe_hwp5_encrypt_version_4(&fixture.source_path, &fixture.credential)
                            .is_ok()
                    {
                        OwnerProfileDiagnosticOutcome::Validated
                    } else {
                        OwnerProfileDiagnosticOutcome::Hwp5Probe
                    },
                ),
                "hwpx" => {
                    let diagnostic =
                        diagnose_hwpx_password_profile(&fixture.source_path, &fixture.credential);
                    (
                        diagnostic.algorithm_id,
                        diagnostic.kdf_id,
                        diagnostic.checksum_scope,
                        diagnostic
                            .stage
                            .map(OwnerProfileDiagnosticOutcome::Hwpx)
                            .unwrap_or(OwnerProfileDiagnosticOutcome::Validated),
                    )
                }
                _ => return Err("owner password corpus fixture format is unsupported".to_owned()),
            };
            serde_json::to_string(&json!({
                "fixture_id": fixture.fixture_id,
                "format": fixture.format,
                "algorithm_id": algorithm_id,
                "kdf_id": kdf_id,
                "checksum_scope": checksum_scope,
                "result": outcome.label(),
            }))
            .map_err(|_| "owner profile diagnostic cannot be encoded".to_owned())
        })
        .collect()
}

pub fn owner_manifest_path(value: Option<std::ffi::OsString>) -> Result<PathBuf, String> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| "owner password corpus manifest is not configured".to_owned())
}

/// The parameterized form keeps the fail-closed path testable without reading
/// process secrets or changing the test process environment.
pub fn run_owner_discovery<F>(
    manifest_path: &Path,
    evidence_path: &Path,
    resolve_credential: F,
) -> Result<(), String>
where
    F: Fn(&str) -> Option<String>,
{
    if !manifest_path.is_absolute() || !manifest_path.is_file() {
        return Err("owner password corpus manifest is unavailable".to_owned());
    }
    if !evidence_path.is_absolute() {
        return Err("profile evidence destination must be absolute".to_owned());
    }

    // Remove a potentially stale receipt before inspecting mutable owner input.
    // Failure at any later gate therefore cannot leave a complete artifact that
    // a subsequent plan could mistake for current evidence.
    remove_existing_evidence(evidence_path)?;

    let manifest_bytes = fs::read(manifest_path)
        .map_err(|_| "owner password corpus manifest cannot be read".to_owned())?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| "owner password corpus manifest is invalid".to_owned())?;
    // Reuse the closed-schema and role validator before retaining private paths.
    load_manifest_for_test(&manifest, &repo_root(), |reference| {
        resolve_credential(reference)
    })?;
    let fixtures = owner_fixtures(&manifest, resolve_credential)?;

    let mut evidence = Vec::with_capacity(fixtures.len());
    for fixture in &fixtures {
        if !fixture.source_path.is_file() {
            return Err(owner_fixture_error(fixture, "source is unavailable"));
        }
        let source_sha256 = source_sha256(&fixture.source_path)
            .map_err(|_| owner_fixture_error(fixture, "source cannot be read"))?;
        let profile = match fixture.format.as_str() {
            "hwp5" => {
                let observation =
                    probe_hwp5_encrypt_version_4(&fixture.source_path, &fixture.credential)
                        .map_err(|_| owner_hwp5_probe_error(fixture))?;
                ProfileObservation::Hwp5 {
                    encrypt_version: observation.encrypt_version,
                    cfb_stream_count: observation.cfb_stream_count,
                    cfb_stream_bytes: observation.cfb_stream_bytes,
                    validated_record_stream_count: observation.validated_record_stream_count,
                    validated_record_streams: observation.validated_record_streams,
                }
            }
            "hwpx" => {
                let observation =
                    probe_hwpx_password_profile(&fixture.source_path, &fixture.credential)
                        .map_err(|error| owner_hwpx_probe_error(fixture, &error))?;
                profile_observation_hwpx(observation)
            }
            _ => return Err(owner_fixture_error(fixture, "format is unsupported")),
        };
        evidence.push(FixtureEvidence {
            fixture_id: fixture.fixture_id.clone(),
            format: fixture.format.clone(),
            role: fixture.role.clone(),
            credential_charset: fixture.credential_charset.clone(),
            source_sha256,
            profile,
        });
    }

    write_evidence_atomically(
        evidence_path,
        &ProfileEvidence {
            version: "password-profile-evidence-v1",
            budget_profile: BudgetProfile {
                hwp5_stream_bytes: HWP5_STREAM_BYTES,
                hwp5_live_bytes: HWP5_LIVE_BYTES,
                hwpx_entry_bytes: HWPX_ENTRY_BYTES,
                hwpx_xml_bytes: HWPX_XML_BYTES,
                hwpx_live_bytes: HWPX_LIVE_BYTES,
                hwpx_compression_ratio: HWPX_COMPRESSION_RATIO,
            },
            fixtures: evidence,
        },
    )
}

fn evidence_destination_from_env() -> Result<PathBuf, String> {
    match std::env::var_os("HWP_PASSWORD_PROFILE_EVIDENCE") {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("profile evidence destination must be absolute".to_owned());
            }
            Ok(path)
        }
        None => Ok(repo_root()
            .join(".planning/phases/08-password-protected-input/08-PROFILE-EVIDENCE.json")),
    }
}

fn remove_existing_evidence(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("existing profile evidence cannot be cleared".to_owned()),
    }
}

fn owner_fixtures<F>(manifest: &Value, resolve_credential: F) -> Result<Vec<OwnerFixture>, String>
where
    F: Fn(&str) -> Option<String>,
{
    manifest["fixtures"]
        .as_array()
        .ok_or_else(|| "owner password corpus fixtures are unavailable".to_owned())?
        .iter()
        .map(|fixture| {
            let value = |name| {
                fixture[name]
                    .as_str()
                    .ok_or_else(|| "owner password corpus fixture is invalid".to_owned())
            };
            let credential_ref = value("credential_ref")?;
            Ok(OwnerFixture {
                fixture_id: value("fixture_id")?.to_owned(),
                source_path: PathBuf::from(value("source_path")?),
                format: value("format")?.to_owned(),
                role: value("role")?.to_owned(),
                credential_charset: value("credential_charset")?.to_owned(),
                credential: Zeroizing::new(
                    resolve_credential(credential_ref)
                        .ok_or_else(|| "owner password credential is unavailable".to_owned())?,
                ),
            })
        })
        .collect()
}

fn source_sha256(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|_| "owner password corpus source cannot be read".to_owned())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "owner password corpus source cannot be hashed".to_owned())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn write_evidence_atomically(path: &Path, evidence: &ProfileEvidence) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| "profile evidence directory is unavailable".to_owned())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "profile evidence clock is unavailable".to_owned())?
        .as_nanos();
    let temporary = parent.join(format!(
        ".password-profile-evidence-{}-{nonce}.tmp",
        std::process::id()
    ));
    let encoded = serde_json::to_vec_pretty(evidence)
        .map_err(|_| "profile evidence cannot be encoded".to_owned())?;
    let result = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "profile evidence temporary file cannot be created".to_owned())?;
        file.write_all(&encoded)
            .map_err(|_| "profile evidence cannot be written".to_owned())?;
        file.write_all(b"\n")
            .map_err(|_| "profile evidence cannot be written".to_owned())?;
        file.sync_all()
            .map_err(|_| "profile evidence cannot be synchronized".to_owned())?;
        fs::rename(&temporary, path).map_err(|_| "profile evidence cannot be published".to_owned())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub const HWP5_STREAM_BYTES: u64 = 64 * 1024 * 1024;
pub const HWP5_LIVE_BYTES: u64 = 128 * 1024 * 1024;
pub const HWPX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
pub const HWPX_XML_BYTES: u64 = 64 * 1024 * 1024;
pub const HWPX_LIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const HWPX_COMPRESSION_RATIO: u64 = 1_000;

#[derive(Clone, Copy)]
pub struct HwpxBufferSizes {
    pub ciphertext: u64,
    pub decrypted_compressed: u64,
    pub inflated: u64,
    pub parser_owned: u64,
}

/// Checks HWP5's per-stream and simultaneous-live-buffer budget without
/// allocating either buffer. The caller performs the same check before each
/// materialization from a private CFB stream.
pub fn check_hwp5_budget(stream_bytes: u64, simultaneous_live_bytes: u64) -> Result<(), String> {
    if stream_bytes > HWP5_STREAM_BYTES {
        return Err("HWP5 stream exceeds the materialized-stream budget".to_owned());
    }
    if simultaneous_live_bytes > HWP5_LIVE_BYTES {
        return Err("HWP5 simultaneous buffers exceed the aggregate budget".to_owned());
    }
    let live = stream_bytes
        .checked_add(simultaneous_live_bytes)
        .ok_or_else(|| "HWP5 buffer arithmetic overflowed".to_owned())?;
    if live > HWP5_LIVE_BYTES {
        return Err("HWP5 simultaneous buffers exceed the aggregate budget".to_owned());
    }
    Ok(())
}

/// Checks every buffer that can coexist while one protected HWPX entry is
/// decrypted, inflated and handed to a parser. This is an accounting probe;
/// it must run before allocations, not after a `Vec` has been built.
pub fn check_hwpx_budget(entry_name: &str, sizes: HwpxBufferSizes) -> Result<(), String> {
    let entry_limit = if entry_name.to_ascii_lowercase().ends_with(".xml") {
        HWPX_XML_BYTES
    } else {
        HWPX_ENTRY_BYTES
    };
    if sizes.ciphertext > HWPX_ENTRY_BYTES
        || sizes.decrypted_compressed > HWPX_ENTRY_BYTES
        || sizes.inflated > entry_limit
        || sizes.parser_owned > entry_limit
    {
        return Err("HWPX entry exceeds a bounded buffer limit".to_owned());
    }
    if sizes.inflated > 0 {
        let ratio_limit = sizes
            .ciphertext
            .checked_mul(HWPX_COMPRESSION_RATIO)
            .ok_or_else(|| "HWPX compression-ratio arithmetic overflowed".to_owned())?;
        if sizes.inflated > ratio_limit {
            return Err("HWPX entry exceeds the compression-ratio budget".to_owned());
        }
    }
    let live = [
        sizes.ciphertext,
        sizes.decrypted_compressed,
        sizes.inflated,
        sizes.parser_owned,
    ]
    .into_iter()
    .try_fold(0u64, |total, size| {
        total
            .checked_add(size)
            .ok_or_else(|| "HWPX simultaneous-buffer arithmetic overflowed".to_owned())
    })?;
    if live > HWPX_LIVE_BYTES {
        return Err("HWPX simultaneous buffers exceed the aggregate budget".to_owned());
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub struct HwpxProfile {
    pub algorithm_id: String,
    pub kdf_id: String,
    pub start_key_id: String,
    pub checksum_id: String,
    pub protected_entry_count: usize,
}

const HWPX_ALGORITHM: &str = "http://www.w3.org/2001/04/xmlenc#aes256-cbc";
const HWPX_KDF: &str = "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2";
const HWPX_PBKDF2_PRF: &str = "hmac-sha1";
const HWPX_START_KEY: &str = "http://www.w3.org/2000/09/xmldsig#sha256";
const HWPX_CHECKSUM: &str = "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k";
const HWPX_CBC_PADDING: &str = "zero-to-aes-block";
const HWPX_MAX_PBKDF2_ITERATIONS: u32 = 1_000_000;
const HWPX_INTEGRITY_ERROR: &str = "HWPX credential or integrity validation failed";
const HWPX_UNSUPPORTED_PROFILE_ERROR: &str = "HWPX protected profile is unsupported";
const OWNER_FIXTURE_CREDENTIAL_OR_INTEGRITY_ERROR: &str =
    "credential or integrity validation failed";
const OWNER_FIXTURE_UNSUPPORTED_PROFILE_ERROR: &str = "protected profile is unsupported";

fn owner_fixture_error(fixture: &OwnerFixture, reason: &str) -> String {
    format!(
        "fixture {} ({}): {reason}",
        fixture.fixture_id, fixture.format
    )
}

fn owner_hwp5_probe_error(fixture: &OwnerFixture) -> String {
    owner_fixture_error(fixture, OWNER_FIXTURE_CREDENTIAL_OR_INTEGRITY_ERROR)
}

fn owner_hwpx_probe_error(fixture: &OwnerFixture, error: &str) -> String {
    let reason = if error == HWPX_UNSUPPORTED_PROFILE_ERROR {
        OWNER_FIXTURE_UNSUPPORTED_PROFILE_ERROR
    } else {
        OWNER_FIXTURE_CREDENTIAL_OR_INTEGRITY_ERROR
    };
    owner_fixture_error(fixture, reason)
}

/// Parses only the non-secret identifiers needed to decide whether a private
/// HWPX package can enter the subsequent owner-controlled probe. The schema
/// deliberately recognizes one observed-compatible profile and rejects all
/// other combinations before crypto allocation is attempted.
pub fn parse_hwpx_profile(manifest: &str) -> Result<HwpxProfile, String> {
    use base64::Engine as _;
    use quick_xml::Reader;
    use quick_xml::events::Event;

    #[derive(Default)]
    struct Entry {
        protected: bool,
        algorithm_id: Option<String>,
        kdf_id: Option<String>,
        start_key_id: Option<String>,
        checksum_id: Option<String>,
        checksum_len: Option<usize>,
        iv_len: Option<usize>,
        salt_len: Option<usize>,
        key_size: Option<usize>,
        iterations: Option<u32>,
    }

    fn local_name(name: &[u8]) -> &[u8] {
        name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
    }
    fn attr(event: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
        event
            .attributes()
            .flatten()
            .find(|attribute| local_name(attribute.key.as_ref()) == key)
            .and_then(|attribute| String::from_utf8(attribute.value.into_owned()).ok())
    }
    fn base64_len(value: Option<String>) -> Result<usize, String> {
        let value = value.ok_or_else(|| "HWPX profile metadata is incomplete".to_owned())?;
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .map(|bytes| bytes.len())
            .map_err(|_| "HWPX profile metadata is not base64".to_owned())
    }

    let mut reader = Reader::from_str(manifest);
    let mut current: Option<Entry> = None;
    let mut entries = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                match local_name(event.name().as_ref()) {
                    b"file-entry" => {
                        if current.is_some() || attr(&event, b"full-path").is_none() {
                            return Err("HWPX protected entry structure is ambiguous".to_owned());
                        }
                        current = Some(Entry::default());
                    }
                    b"encryption-data" => {
                        let entry = current
                            .as_mut()
                            .ok_or_else(|| "HWPX encryption metadata has no entry".to_owned())?;
                        entry.protected = true;
                        entry.checksum_id = attr(&event, b"checksum-type");
                        entry.checksum_len = Some(base64_len(attr(&event, b"checksum"))?);
                    }
                    b"algorithm" => {
                        let entry = current
                            .as_mut()
                            .ok_or_else(|| "HWPX algorithm metadata has no entry".to_owned())?;
                        entry.algorithm_id = attr(&event, b"algorithm-name");
                        entry.iv_len = Some(base64_len(attr(&event, b"initialisation-vector"))?);
                    }
                    b"key-derivation" => {
                        let entry = current
                            .as_mut()
                            .ok_or_else(|| "HWPX KDF metadata has no entry".to_owned())?;
                        entry.kdf_id = attr(&event, b"key-derivation-name");
                        entry.key_size =
                            attr(&event, b"key-size").and_then(|value| value.parse().ok());
                        entry.iterations =
                            attr(&event, b"iteration-count").and_then(|value| value.parse().ok());
                        entry.salt_len = Some(base64_len(attr(&event, b"salt"))?);
                    }
                    b"start-key-generation" => {
                        let entry = current
                            .as_mut()
                            .ok_or_else(|| "HWPX start-key metadata has no entry".to_owned())?;
                        entry.start_key_id = attr(&event, b"start-key-generation-name");
                    }
                    _ => {}
                }
            }
            Ok(Event::End(event)) if local_name(event.name().as_ref()) == b"file-entry" => {
                let entry = current
                    .take()
                    .ok_or_else(|| "HWPX protected entry structure is ambiguous".to_owned())?;
                if entry.protected {
                    entries.push(entry);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err("HWPX manifest XML is malformed".to_owned()),
        }
    }

    let first = entries
        .first()
        .ok_or_else(|| "HWPX manifest has no protected entries".to_owned())?;
    let profile = HwpxProfile {
        algorithm_id: first
            .algorithm_id
            .clone()
            .ok_or_else(|| "HWPX algorithm identifier is unavailable".to_owned())?,
        kdf_id: first
            .kdf_id
            .clone()
            .ok_or_else(|| "HWPX KDF identifier is unavailable".to_owned())?,
        start_key_id: first
            .start_key_id
            .clone()
            .ok_or_else(|| "HWPX start-key identifier is unavailable".to_owned())?,
        checksum_id: first
            .checksum_id
            .clone()
            .ok_or_else(|| "HWPX checksum identifier is unavailable".to_owned())?,
        protected_entry_count: entries.len(),
    };
    for entry in entries {
        if entry.algorithm_id.as_deref() != Some(HWPX_ALGORITHM)
            || entry.kdf_id.as_deref() != Some(HWPX_KDF)
            || entry.start_key_id.as_deref() != Some(HWPX_START_KEY)
            || entry.checksum_id.as_deref() != Some(HWPX_CHECKSUM)
            || entry.checksum_len != Some(32)
            || entry.iv_len != Some(16)
            || entry
                .salt_len
                .is_none_or(|length| length == 0 || length > 1024)
            || entry.key_size != Some(32)
            || entry
                .iterations
                .is_none_or(|iterations| iterations == 0 || iterations > HWPX_MAX_PBKDF2_ITERATIONS)
        {
            return Err("HWPX profile is unsupported or incomplete".to_owned());
        }
    }
    Ok(profile)
}

pub fn supported_hwpx_manifest() -> String {
    r#"<?xml version="1.0"?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><odf:file-entry full-path="Contents/section0.xml"><odf:encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="><odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="AAAAAAAAAAAAAAAAAAAAAA=="/><odf:key-derivation key-derivation-name="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2" key-size="32" iteration-count="1024" salt="AAAAAAAAAAAAAAAAAAAAAA=="/><odf:start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256"/></odf:encryption-data></odf:file-entry></odf:manifest>"#.to_owned()
}

pub fn unsupported_hwpx_manifest() -> String {
    supported_hwpx_manifest().replace("aes256-cbc", "aes128-cbc")
}

/// Applies the only HWP5 password-stream candidate admitted by the prerequisite
/// research. This stays a probe, not production reader behavior: only an
/// owner corpus can establish which streams genuinely validate.
pub fn transform_hwp5_encrypt_version_4_in_place(
    bytes: &mut [u8],
    password: &str,
) -> Result<(), String> {
    use aes::Aes128;
    use aes::cipher::{Block, BlockCipherEncrypt, KeyInit};
    use sha1::Digest as _;
    use zeroize::Zeroize as _;

    let password = password.as_bytes();
    let mut source = Zeroizing::new(Vec::with_capacity(password.len().saturating_mul(2)));
    for (index, byte) in password.iter().copied().enumerate() {
        let previous = if index == 0 {
            0xec
        } else {
            password[index - 1]
        };
        source.push(previous.rotate_left(1));
        source.push(byte);
    }
    let mut digest = sha1::Sha1::digest(&*source);
    let mut key = Zeroizing::new([0u8; 16]);
    key.copy_from_slice(&digest[..16]);
    digest.zeroize();
    let cipher = Aes128::new_from_slice(&*key).map_err(|_| "HWP5 cipher setup failed")?;
    let mut register = [0u8; 16];

    for block in bytes.chunks_mut(16) {
        let mut original = [0u8; 16];
        original[..block.len()].copy_from_slice(block);
        let mut transformed = [0u8; 16];
        for bit_index in 0..128 {
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;
            let mut keystream = Block::<Aes128>::from(register);
            cipher.encrypt_block(&mut keystream);
            let input_bit = (original[byte_index] >> (7 - bit_offset)) & 1;
            let result_bit = input_bit ^ (keystream[0] >> 7);
            for index in 0..15 {
                register[index] = (register[index] << 1) | (register[index + 1] >> 7);
            }
            register[15] = (register[15] << 1) | (input_bit & 1);
            transformed[byte_index] |= result_bit << (7 - bit_offset);
        }
        block.copy_from_slice(&transformed[..block.len()]);
    }
    Ok(())
}

/// A content-free observation from a private HWP5 file. Stream bytes never
/// cross this return boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hwp5ValidatedStream {
    /// A bounded CFB identifier, never a source filesystem path.
    pub path: String,
    pub size: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Hwp5ProbeObservation {
    pub encrypt_version: u32,
    pub cfb_stream_count: usize,
    pub cfb_stream_bytes: u64,
    pub validated_record_stream_count: usize,
    pub validated_record_streams: Vec<Hwp5ValidatedStream>,
}

/// Returns the only content-free HWP5 profile fragment that can be persisted
/// in Phase 8 evidence. In particular, it carries bounded CFB identifiers,
/// never an owner source path or transformed bytes.
pub fn serialize_hwp5_profile_observation(observation: &Hwp5ProbeObservation) -> Value {
    serde_json::to_value(ProfileObservation::Hwp5 {
        encrypt_version: observation.encrypt_version,
        cfb_stream_count: observation.cfb_stream_count,
        cfb_stream_bytes: observation.cfb_stream_bytes,
        validated_record_stream_count: observation.validated_record_stream_count,
        validated_record_streams: observation.validated_record_streams.clone(),
    })
    .expect("HWP5 profile observation is serializable")
}

/// Fail-closed, content-free classifier for the HWP5 discovery entry point.
/// It deliberately recognizes only the reference candidate, EncryptVersion 4.
pub fn check_hwp5_discovery_eligibility(header: &hwp5::FileHeader) -> Result<(), String> {
    // These are not password-encryption profiles. Refuse them before any CFB
    // stream materialization or candidate transform so they cannot become
    // password evidence by accident.
    if header.is_cert_encrypted() {
        return Err("HWP5 certificate encryption must not enter password discovery".to_owned());
    }
    if header.is_cert_drm() {
        return Err("HWP5 certificate DRM must not enter password discovery".to_owned());
    }
    if header.is_drm() {
        return Err("HWP5 DRM must not enter password discovery".to_owned());
    }
    if header.has_signature() {
        return Err("HWP5 digital signature must not enter password discovery".to_owned());
    }
    if header.is_distribution() {
        return Err("HWP5 distribution document must not enter password discovery".to_owned());
    }
    if !header.is_encrypted() {
        return Err("HWP5 is not marked encrypted".to_owned());
    }
    match header.encrypt_version {
        0 => Err("HWP5 encrypted bit + EncryptVersion=0 is internally inconsistent".to_owned()),
        4 => Ok(()),
        version => Err(format!("HWP5 EncryptVersion={version} is unsupported")),
    }
}

/// Makes the header gate mechanically precede stream access. Keeping this
/// small seam separate lets synthetic tests prove a rejected header cannot
/// reach CFB reads or the candidate transform.
pub fn after_hwp5_discovery_gate<T>(
    header: &hwp5::FileHeader,
    access_streams: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    check_hwp5_discovery_eligibility(header)?;
    access_streams()
}

fn expected_hwp5_record_tag(path: &str) -> Option<u16> {
    if path == "/DocInfo" {
        Some(hwp5::record::tag::DOCUMENT_PROPERTIES)
    } else if path.starts_with("/BodyText/Section") {
        Some(hwp5::record::tag::PARA_HEADER)
    } else if path.starts_with("/ViewText/Section") {
        Some(hwp5::record::tag::DISTRIBUTE_DOC_DATA)
    } else {
        None
    }
}

fn record_tree_contains_tag(nodes: &[hwp5::record::RecordNode], expected: u16) -> bool {
    nodes
        .iter()
        .any(|node| node.tag == expected || record_tree_contains_tag(&node.children, expected))
}

/// A strict scan alone is intentionally insufficient: a transformed stream
/// must also identify itself as the kind of record stream its CFB name claims.
pub fn validate_hwp5_record_identity(
    path: &str,
    scan: &hwp5::record::ScanResult,
) -> Result<(), String> {
    let expected = expected_hwp5_record_tag(path)
        .ok_or_else(|| "HWP5 CFB stream is not eligible for password discovery".to_owned())?;
    if record_tree_contains_tag(&scan.roots, expected) {
        Ok(())
    } else {
        Err("HWP5 candidate record identity did not validate".to_owned())
    }
}

pub fn probe_hwp5_encrypt_version_4(
    path: &Path,
    password: &str,
) -> Result<Hwp5ProbeObservation, String> {
    let mut container = hwp5::Hwp5Container::open(path)
        .map_err(|_| "HWP5 container cannot be opened for profile discovery".to_owned())?;
    let header = container.file_header().clone();
    after_hwp5_discovery_gate(&header, || {
        probe_hwp5_candidate_streams(&mut container, &header, password)
    })
}

fn probe_hwp5_candidate_streams(
    container: &mut hwp5::Hwp5Container,
    header: &hwp5::FileHeader,
    password: &str,
) -> Result<Hwp5ProbeObservation, String> {
    let streams = container.list_streams();
    let cfb_stream_bytes = streams.iter().try_fold(0u64, |total, stream| {
        total
            .checked_add(stream.size)
            .ok_or_else(|| "HWP5 CFB stream accounting overflowed".to_owned())
    })?;
    let mut validated_record_streams = Vec::new();
    for stream in streams.iter().filter(|stream| {
        stream.path == "/DocInfo"
            || stream.path.starts_with("/BodyText/")
            || stream.path.starts_with("/ViewText/")
    }) {
        check_hwp5_budget(
            stream.size,
            if header.is_compressed() {
                stream.size
            } else {
                0
            },
        )?;
        let mut payload = Zeroizing::new(
            container
                .read_stream_raw(&stream.path)
                .map_err(|_| "HWP5 protected stream cannot be read".to_owned())?,
        );
        transform_hwp5_encrypt_version_4_in_place(&mut payload, password)?;
        let decoded = if header.is_compressed() {
            hwp5::codec::decompress_bounded(&payload, "private HWP5 probe", HWP5_STREAM_BYTES)
                .map_err(|_| "HWP5 candidate transform did not validate".to_owned())?
        } else {
            payload.to_vec()
        };
        let scan = hwp5::record::scan_stream(&decoded, hwp5::record::ScanMode::Strict)
            .map_err(|_| "HWP5 candidate record shape did not validate".to_owned())?;
        if scan.record_count == 0 {
            return Err("HWP5 candidate record stream is empty".to_owned());
        }
        validate_hwp5_record_identity(&stream.path, &scan)?;
        validated_record_streams.push(Hwp5ValidatedStream {
            path: stream.path.clone(),
            size: stream.size,
        });
    }
    if validated_record_streams.is_empty() {
        return Err("HWP5 stream behavior is ambiguous".to_owned());
    }
    Ok(Hwp5ProbeObservation {
        encrypt_version: header.encrypt_version,
        cfb_stream_count: streams.len(),
        cfb_stream_bytes,
        validated_record_stream_count: validated_record_streams.len(),
        validated_record_streams,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct HwpxProbeObservation {
    pub algorithm_id: String,
    pub kdf_id: String,
    pub start_key_id: String,
    pub checksum_id: String,
    pub checksum_scope: HwpxChecksumScope,
    pub protected_entry_count: usize,
    pub validated_entry_count: usize,
    pub protected_entry_bytes: u64,
}

/// Closed, test-only failure labels for the HWPX owner-profile probe. These
/// identify a validation boundary without carrying the value that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HwpxProbeFailureStage {
    CiphertextStorageProfile,
    KeyDerivationCipherInit,
    ZeroSuffix,
    DeflateStream,
    ChecksumScope,
    DeclaredSize,
    XmlStructure,
    Bounds,
}

impl HwpxProbeFailureStage {
    pub const ALL: [Self; 8] = [
        Self::CiphertextStorageProfile,
        Self::KeyDerivationCipherInit,
        Self::ZeroSuffix,
        Self::DeflateStream,
        Self::ChecksumScope,
        Self::DeclaredSize,
        Self::XmlStructure,
        Self::Bounds,
    ];
}

/// Content-free outcome of the test-only HWPX probe diagnostic. Its two
/// identifiers are a closed vocabulary, never copied from private metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HwpxProbeDiagnostic {
    pub algorithm_id: &'static str,
    pub kdf_id: &'static str,
    pub checksum_scope: Option<HwpxChecksumScope>,
    pub stage: Option<HwpxProbeFailureStage>,
}

/// The two checksum byte scopes admitted by this owner-only diagnostic. The
/// ODF URI denotes the normative compressed scope; Hancom fixtures observed
/// by the owner diagnostic use the inflated plaintext scope instead. Both are
/// tried with the same declared SHA-256/1k value and ambiguity fails closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HwpxChecksumScope {
    CompressedUnencrypted,
    InflatedPlaintextHancom,
}

/// Selects exactly one closed checksum scope without returning or logging the
/// protected bytes. This small seam permits a synthetic ambiguity test without
/// constructing an infeasible SHA-256 collision in an encrypted package.
pub fn select_hwpx_checksum_scope(
    declared_checksum: &[u8],
    compressed_unencrypted: &[u8],
    inflated_plaintext: &[u8],
) -> Result<HwpxChecksumScope, ()> {
    let compressed_matches =
        Sha256::digest(&compressed_unencrypted[..compressed_unencrypted.len().min(1024)])
            .as_slice()
            == declared_checksum;
    let inflated_matches =
        Sha256::digest(&inflated_plaintext[..inflated_plaintext.len().min(1024)]).as_slice()
            == declared_checksum;
    match (compressed_matches, inflated_matches) {
        (true, false) => Ok(HwpxChecksumScope::CompressedUnencrypted),
        (false, true) => Ok(HwpxChecksumScope::InflatedPlaintextHancom),
        (false, false) | (true, true) => Err(()),
    }
}

/// Requires every protected entry in one package to settle on the same closed
/// checksum scope. A package that mixes the normative and observed profiles is
/// unsupported rather than being accepted by ordering or fallback.
pub fn merge_hwpx_checksum_scope(
    selected: Option<HwpxChecksumScope>,
    entry_scope: HwpxChecksumScope,
) -> Result<HwpxChecksumScope, ()> {
    match selected {
        None => Ok(entry_scope),
        Some(scope) if scope == entry_scope => Ok(entry_scope),
        Some(_) => Err(()),
    }
}

fn profile_observation_hwpx(observation: HwpxProbeObservation) -> ProfileObservation {
    ProfileObservation::Hwpx {
        algorithm_id: observation.algorithm_id,
        kdf_id: observation.kdf_id,
        pbkdf2_prf: HWPX_PBKDF2_PRF,
        start_key_id: observation.start_key_id,
        checksum_id: observation.checksum_id,
        checksum_scope: observation.checksum_scope,
        cbc_padding: HWPX_CBC_PADDING,
        protected_entry_count: observation.protected_entry_count,
        validated_entry_count: observation.validated_entry_count,
        protected_entry_bytes: observation.protected_entry_bytes,
    }
}

pub fn serialize_hwpx_profile_observation(observation: HwpxProbeObservation) -> Value {
    serde_json::to_value(profile_observation_hwpx(observation))
        .expect("closed HWPX profile evidence is serializable")
}

pub fn hwpx_integrity_error() -> &'static str {
    HWPX_INTEGRITY_ERROR
}

pub fn hwpx_unsupported_profile_error() -> &'static str {
    HWPX_UNSUPPORTED_PROFILE_ERROR
}

struct HwpxEncryptedEntry {
    name: String,
    declared_plaintext_bytes: u64,
    algorithm_id: String,
    iv: Vec<u8>,
    kdf_id: String,
    salt: Vec<u8>,
    iterations: u32,
    key_size: usize,
    start_key_id: String,
    checksum_id: String,
    checksum: Vec<u8>,
}

/// Probes the narrowly admitted ODF profile without retaining decrypted
/// material. Each protected entry must decrypt, pass the declared checksum,
/// raw-inflate inside the fixed bounds and have valid XML when it declares an
/// XML name. Any deviation is deliberately reported as one content-free error.
pub fn probe_hwpx_password_profile(
    path: &Path,
    password: &str,
) -> Result<HwpxProbeObservation, String> {
    use aes::Aes256;
    use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::NoPadding};
    use pbkdf2::pbkdf2_hmac;

    let mut package = hwpx::HwpxPackage::open(path).map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
    let manifest = package
        .read_entry_string("META-INF/manifest.xml")
        .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
    let profile =
        parse_hwpx_profile(&manifest).map_err(|_| HWPX_UNSUPPORTED_PROFILE_ERROR.to_owned())?;
    let protected =
        parse_hwpx_encrypted_entries(&manifest).map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
    if protected.is_empty() {
        return Err(HWPX_INTEGRITY_ERROR.to_owned());
    }
    if protected.iter().any(|entry| {
        entry.algorithm_id != profile.algorithm_id
            || entry.kdf_id != profile.kdf_id
            || entry.start_key_id != profile.start_key_id
            || entry.checksum_id != profile.checksum_id
            || entry.iv.len() != 16
            || entry.salt.is_empty()
            || entry.salt.len() > 1024
            || entry.key_size != 32
            || entry.iterations == 0
            || entry.iterations > HWPX_MAX_PBKDF2_ITERATIONS
            || entry.checksum.len() != 32
    }) {
        return Err(HWPX_UNSUPPORTED_PROFILE_ERROR.to_owned());
    }

    let entries = package
        .entries()
        .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
    let file = fs::File::open(path).map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
    let mut raw_zip = zip::ZipArchive::new(file).map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
    let mut start_key = Zeroizing::new(Sha256::digest(password.as_bytes()).to_vec());
    let mut validated_entry_count = 0usize;
    let mut protected_entry_bytes = 0u64;
    let mut checksum_scope = None;

    for entry in &protected {
        let zip_entry = entries
            .iter()
            .find(|candidate| candidate.name == entry.name)
            .ok_or_else(|| HWPX_INTEGRITY_ERROR.to_owned())?;
        check_hwpx_budget(
            &entry.name,
            HwpxBufferSizes {
                ciphertext: zip_entry.size,
                decrypted_compressed: zip_entry.size,
                inflated: entry.declared_plaintext_bytes,
                parser_owned: 0,
            },
        )
        .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        let mut raw_entry = raw_zip
            .by_name(&entry.name)
            .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        if raw_entry.compression() != zip::CompressionMethod::Stored
            || raw_entry.size() != zip_entry.size
            || raw_entry.compressed_size() != zip_entry.size
            || zip_entry.size == 0
            || zip_entry.size % 16 != 0
        {
            return Err(HWPX_INTEGRITY_ERROR.to_owned());
        }
        let ciphertext_capacity =
            usize::try_from(zip_entry.size).map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        let mut ciphertext = Zeroizing::new(Vec::new());
        ciphertext
            .try_reserve_exact(ciphertext_capacity)
            .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        raw_entry
            .read_to_end(&mut ciphertext)
            .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        if ciphertext.len() as u64 != zip_entry.size {
            return Err(HWPX_INTEGRITY_ERROR.to_owned());
        }
        let mut key = Zeroizing::new([0u8; 32]);
        pbkdf2_hmac::<sha1::Sha1>(&start_key, &entry.salt, entry.iterations, &mut *key);
        let decryptor = cbc::Decryptor::<Aes256>::new_from_slices(&*key, &entry.iv)
            .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        let compressed = decryptor
            .decrypt_padded::<NoPadding>(&mut ciphertext)
            .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        let (plaintext, compressed_end) =
            inflate_raw_bounded(compressed, entry.declared_plaintext_bytes, &entry.name)
                .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        let suffix = &compressed[compressed_end..];
        if suffix.len() > 15 || suffix.iter().any(|byte| *byte != 0) {
            return Err(HWPX_INTEGRITY_ERROR.to_owned());
        }
        let entry_checksum_scope =
            select_hwpx_checksum_scope(&entry.checksum, &compressed[..compressed_end], &plaintext)
                .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        checksum_scope = Some(
            merge_hwpx_checksum_scope(checksum_scope, entry_checksum_scope)
                .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?,
        );
        if plaintext.len() as u64 != entry.declared_plaintext_bytes {
            return Err(HWPX_INTEGRITY_ERROR.to_owned());
        }
        if entry.name.to_ascii_lowercase().ends_with(".xml") {
            validate_xml_structure(&plaintext).map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
        }
        protected_entry_bytes = protected_entry_bytes
            .checked_add(entry.declared_plaintext_bytes)
            .ok_or_else(|| HWPX_INTEGRITY_ERROR.to_owned())?;
        if protected_entry_bytes > HWPX_LIVE_BYTES {
            return Err(HWPX_INTEGRITY_ERROR.to_owned());
        }
        validated_entry_count += 1;
    }
    use zeroize::Zeroize as _;
    start_key.zeroize();
    package
        .verify_integrity()
        .map_err(|_| HWPX_INTEGRITY_ERROR.to_owned())?;
    Ok(HwpxProbeObservation {
        algorithm_id: profile.algorithm_id,
        kdf_id: profile.kdf_id,
        start_key_id: profile.start_key_id,
        checksum_id: profile.checksum_id,
        checksum_scope: checksum_scope.ok_or_else(|| HWPX_INTEGRITY_ERROR.to_owned())?,
        protected_entry_count: protected.len(),
        validated_entry_count,
        protected_entry_bytes,
    })
}

/// Runs the same admitted HWPX profile checks as the public probe, but exposes
/// only a closed validation boundary for the owner-only ignored test. It never
/// returns source data, key material, checksums, or decrypted bytes.
pub fn diagnose_hwpx_password_profile(path: &Path, password: &str) -> HwpxProbeDiagnostic {
    use aes::Aes256;
    use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::NoPadding};
    use pbkdf2::pbkdf2_hmac;

    let unavailable = || HwpxProbeDiagnostic {
        algorithm_id: "unavailable",
        kdf_id: "unavailable",
        checksum_scope: None,
        stage: Some(HwpxProbeFailureStage::CiphertextStorageProfile),
    };
    let mut package = match hwpx::HwpxPackage::open(path) {
        Ok(package) => package,
        Err(_) => return unavailable(),
    };
    let manifest = match package.read_entry_string("META-INF/manifest.xml") {
        Ok(manifest) => manifest,
        Err(_) => return unavailable(),
    };
    let profile = match parse_hwpx_profile(&manifest) {
        Ok(profile) => profile,
        Err(_) => return unavailable(),
    };
    let profile_diagnostic = |stage| HwpxProbeDiagnostic {
        algorithm_id: HWPX_ALGORITHM,
        kdf_id: HWPX_KDF,
        checksum_scope: None,
        stage: Some(stage),
    };
    let protected = match parse_hwpx_encrypted_entries(&manifest) {
        Ok(entries) if !entries.is_empty() => entries,
        _ => return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile),
    };
    if protected.iter().any(|entry| {
        entry.algorithm_id != profile.algorithm_id
            || entry.kdf_id != profile.kdf_id
            || entry.start_key_id != profile.start_key_id
            || entry.checksum_id != profile.checksum_id
            || entry.iv.len() != 16
            || entry.salt.is_empty()
            || entry.salt.len() > 1024
            || entry.key_size != 32
            || entry.iterations == 0
            || entry.iterations > HWPX_MAX_PBKDF2_ITERATIONS
            || entry.checksum.len() != 32
    }) {
        return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile);
    }
    let entries = match package.entries() {
        Ok(entries) => entries,
        Err(_) => return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile),
    };
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile),
    };
    let mut raw_zip = match zip::ZipArchive::new(file) {
        Ok(zip) => zip,
        Err(_) => return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile),
    };
    let mut start_key = Zeroizing::new(Sha256::digest(password.as_bytes()).to_vec());
    let mut protected_entry_bytes = 0u64;
    let mut checksum_scope = None;

    for entry in &protected {
        let Some(zip_entry) = entries
            .iter()
            .find(|candidate| candidate.name == entry.name)
        else {
            return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile);
        };
        if check_hwpx_budget(
            &entry.name,
            HwpxBufferSizes {
                ciphertext: zip_entry.size,
                decrypted_compressed: zip_entry.size,
                inflated: entry.declared_plaintext_bytes,
                parser_owned: 0,
            },
        )
        .is_err()
        {
            return profile_diagnostic(HwpxProbeFailureStage::Bounds);
        }
        let mut raw_entry = match raw_zip.by_name(&entry.name) {
            Ok(entry) => entry,
            Err(_) => return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile),
        };
        if raw_entry.compression() != zip::CompressionMethod::Stored
            || raw_entry.size() != zip_entry.size
            || raw_entry.compressed_size() != zip_entry.size
            || zip_entry.size == 0
            || zip_entry.size % 16 != 0
        {
            return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile);
        }
        let ciphertext_capacity = match usize::try_from(zip_entry.size) {
            Ok(capacity) => capacity,
            Err(_) => return profile_diagnostic(HwpxProbeFailureStage::Bounds),
        };
        let mut ciphertext = Zeroizing::new(Vec::new());
        if ciphertext.try_reserve_exact(ciphertext_capacity).is_err() {
            return profile_diagnostic(HwpxProbeFailureStage::Bounds);
        }
        if raw_entry.read_to_end(&mut ciphertext).is_err()
            || ciphertext.len() as u64 != zip_entry.size
        {
            return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile);
        }
        let mut key = Zeroizing::new([0u8; 32]);
        pbkdf2_hmac::<sha1::Sha1>(&start_key, &entry.salt, entry.iterations, &mut *key);
        let decryptor = match cbc::Decryptor::<Aes256>::new_from_slices(&*key, &entry.iv) {
            Ok(decryptor) => decryptor,
            Err(_) => return profile_diagnostic(HwpxProbeFailureStage::KeyDerivationCipherInit),
        };
        let compressed = match decryptor.decrypt_padded::<NoPadding>(&mut ciphertext) {
            Ok(compressed) => compressed,
            Err(_) => return profile_diagnostic(HwpxProbeFailureStage::KeyDerivationCipherInit),
        };
        let (plaintext, compressed_end) =
            match inflate_raw_bounded(compressed, entry.declared_plaintext_bytes, &entry.name) {
                Ok(value) => value,
                Err(_) => return profile_diagnostic(HwpxProbeFailureStage::DeflateStream),
            };
        let suffix = &compressed[compressed_end..];
        if suffix.len() > 15 || suffix.iter().any(|byte| *byte != 0) {
            return profile_diagnostic(HwpxProbeFailureStage::ZeroSuffix);
        }
        let entry_checksum_scope = match select_hwpx_checksum_scope(
            &entry.checksum,
            &compressed[..compressed_end],
            &plaintext,
        ) {
            Ok(scope) => scope,
            Err(_) => return profile_diagnostic(HwpxProbeFailureStage::ChecksumScope),
        };
        checksum_scope = match merge_hwpx_checksum_scope(checksum_scope, entry_checksum_scope) {
            Ok(scope) => Some(scope),
            Err(_) => return profile_diagnostic(HwpxProbeFailureStage::ChecksumScope),
        };
        if plaintext.len() as u64 != entry.declared_plaintext_bytes {
            return profile_diagnostic(HwpxProbeFailureStage::DeclaredSize);
        }
        if entry.name.to_ascii_lowercase().ends_with(".xml")
            && validate_xml_structure(&plaintext).is_err()
        {
            return profile_diagnostic(HwpxProbeFailureStage::XmlStructure);
        }
        protected_entry_bytes =
            match protected_entry_bytes.checked_add(entry.declared_plaintext_bytes) {
                Some(total) if total <= HWPX_LIVE_BYTES => total,
                _ => return profile_diagnostic(HwpxProbeFailureStage::Bounds),
            };
    }
    use zeroize::Zeroize as _;
    start_key.zeroize();
    if package.verify_integrity().is_err() {
        return profile_diagnostic(HwpxProbeFailureStage::CiphertextStorageProfile);
    }
    HwpxProbeDiagnostic {
        algorithm_id: HWPX_ALGORITHM,
        kdf_id: HWPX_KDF,
        checksum_scope,
        stage: None,
    }
}

fn inflate_raw_bounded(
    compressed: &[u8],
    declared_plaintext_bytes: u64,
    entry_name: &str,
) -> Result<(Zeroizing<Vec<u8>>, usize), ()> {
    use flate2::{Decompress, FlushDecompress, Status};

    let entry_limit = if entry_name.to_ascii_lowercase().ends_with(".xml") {
        HWPX_XML_BYTES
    } else {
        HWPX_ENTRY_BYTES
    };
    let maximum = declared_plaintext_bytes.min(entry_limit);
    let mut plaintext = Zeroizing::new(Vec::new());
    let mut decompressor = Decompress::new(false);
    let mut output = [0u8; 8 * 1024];

    loop {
        let input_offset = usize::try_from(decompressor.total_in()).map_err(|_| ())?;
        if input_offset > compressed.len() {
            return Err(());
        }
        let output_before = decompressor.total_out();
        let status = decompressor
            .decompress(
                &compressed[input_offset..],
                &mut output,
                FlushDecompress::None,
            )
            .map_err(|_| ())?;
        let produced = decompressor
            .total_out()
            .checked_sub(output_before)
            .ok_or(())?;
        let next_len = (plaintext.len() as u64).checked_add(produced).ok_or(())?;
        if next_len > maximum {
            return Err(());
        }
        let produced = usize::try_from(produced).map_err(|_| ())?;
        plaintext.try_reserve(produced).map_err(|_| ())?;
        plaintext.extend_from_slice(&output[..produced]);

        if status == Status::StreamEnd {
            let compressed_end = usize::try_from(decompressor.total_in()).map_err(|_| ())?;
            return Ok((plaintext, compressed_end));
        }
        if status != Status::Ok
            || (produced == 0 && input_offset == decompressor.total_in() as usize)
        {
            return Err(());
        }
    }
}

fn parse_hwpx_encrypted_entries(manifest: &str) -> Result<Vec<HwpxEncryptedEntry>, String> {
    use base64::Engine as _;
    use quick_xml::Reader;
    use quick_xml::events::Event;

    #[derive(Default)]
    struct CurrentEntry {
        name: Option<String>,
        size: Option<u64>,
        protected: bool,
        algorithm_id: Option<String>,
        iv: Option<Vec<u8>>,
        kdf_id: Option<String>,
        salt: Option<Vec<u8>>,
        iterations: Option<u32>,
        key_size: Option<usize>,
        start_key_id: Option<String>,
        checksum_id: Option<String>,
        checksum: Option<Vec<u8>>,
    }

    fn local_name(name: &[u8]) -> &[u8] {
        name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
    }
    fn attr(event: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
        event
            .attributes()
            .flatten()
            .find(|attribute| local_name(attribute.key.as_ref()) == key)
            .and_then(|attribute| String::from_utf8(attribute.value.into_owned()).ok())
    }
    fn base64(value: Option<String>) -> Result<Vec<u8>, String> {
        base64::engine::general_purpose::STANDARD
            .decode(value.ok_or_else(|| "HWPX profile metadata is incomplete".to_owned())?)
            .map_err(|_| "HWPX profile metadata is invalid".to_owned())
    }

    let mut reader = Reader::from_str(manifest);
    let mut current: Option<CurrentEntry> = None;
    let mut protected = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                match local_name(event.name().as_ref()) {
                    b"file-entry" => {
                        if current.is_some() {
                            return Err("HWPX protected entry structure is ambiguous".to_owned());
                        }
                        current = Some(CurrentEntry {
                            name: attr(&event, b"full-path"),
                            size: attr(&event, b"size").and_then(|value| value.parse().ok()),
                            ..CurrentEntry::default()
                        });
                    }
                    b"encryption-data" => {
                        let entry = current
                            .as_mut()
                            .ok_or_else(|| "HWPX encryption metadata has no entry".to_owned())?;
                        entry.protected = true;
                        entry.checksum_id = attr(&event, b"checksum-type");
                        entry.checksum = Some(base64(attr(&event, b"checksum"))?);
                    }
                    b"algorithm" => {
                        let entry = current
                            .as_mut()
                            .ok_or_else(|| "HWPX algorithm metadata has no entry".to_owned())?;
                        entry.algorithm_id = attr(&event, b"algorithm-name");
                        entry.iv = Some(base64(attr(&event, b"initialisation-vector"))?);
                    }
                    b"key-derivation" => {
                        let entry = current
                            .as_mut()
                            .ok_or_else(|| "HWPX KDF metadata has no entry".to_owned())?;
                        entry.kdf_id = attr(&event, b"key-derivation-name");
                        entry.salt = Some(base64(attr(&event, b"salt"))?);
                        entry.iterations =
                            attr(&event, b"iteration-count").and_then(|value| value.parse().ok());
                        entry.key_size =
                            attr(&event, b"key-size").and_then(|value| value.parse().ok());
                    }
                    b"start-key-generation" => {
                        let entry = current
                            .as_mut()
                            .ok_or_else(|| "HWPX start-key metadata has no entry".to_owned())?;
                        entry.start_key_id = attr(&event, b"start-key-generation-name");
                    }
                    _ => {}
                }
            }
            Ok(Event::End(event)) if local_name(event.name().as_ref()) == b"file-entry" => {
                let entry = current
                    .take()
                    .ok_or_else(|| "HWPX protected entry structure is ambiguous".to_owned())?;
                if entry.protected {
                    protected.push(HwpxEncryptedEntry {
                        name: entry
                            .name
                            .ok_or_else(|| "HWPX protected entry name is unavailable".to_owned())?,
                        declared_plaintext_bytes: entry
                            .size
                            .ok_or_else(|| "HWPX protected entry size is unavailable".to_owned())?,
                        algorithm_id: entry
                            .algorithm_id
                            .ok_or_else(|| "HWPX algorithm identifier is unavailable".to_owned())?,
                        iv: entry.iv.ok_or_else(|| {
                            "HWPX initialization vector is unavailable".to_owned()
                        })?,
                        kdf_id: entry
                            .kdf_id
                            .ok_or_else(|| "HWPX KDF identifier is unavailable".to_owned())?,
                        salt: entry
                            .salt
                            .ok_or_else(|| "HWPX salt is unavailable".to_owned())?,
                        iterations: entry
                            .iterations
                            .ok_or_else(|| "HWPX iteration count is unavailable".to_owned())?,
                        key_size: entry
                            .key_size
                            .ok_or_else(|| "HWPX key size is unavailable".to_owned())?,
                        start_key_id: entry
                            .start_key_id
                            .ok_or_else(|| "HWPX start-key identifier is unavailable".to_owned())?,
                        checksum_id: entry
                            .checksum_id
                            .ok_or_else(|| "HWPX checksum identifier is unavailable".to_owned())?,
                        checksum: entry
                            .checksum
                            .ok_or_else(|| "HWPX checksum is unavailable".to_owned())?,
                    });
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err("HWPX manifest XML is malformed".to_owned()),
        }
    }
    if current.is_some() {
        return Err("HWPX protected entry structure is ambiguous".to_owned());
    }
    Ok(protected)
}

fn validate_xml_structure(bytes: &[u8]) -> Result<(), String> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_reader(bytes);
    let mut depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| "HWPX protected XML structure did not validate".to_owned())?;
            }
            Ok(Event::End(_)) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| "HWPX protected XML structure did not validate".to_owned())?;
            }
            Ok(Event::Eof) if depth == 0 => return Ok(()),
            Ok(Event::Eof) => {
                return Err("HWPX protected XML structure did not validate".to_owned());
            }
            Ok(_) => {}
            Err(_) => return Err("HWPX protected XML structure did not validate".to_owned()),
        }
    }
}
