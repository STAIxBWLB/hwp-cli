//! Test-only, content-free receipt support for password corpus evidence.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

const RECEIPT_SCHEMA: &str =
    include_str!("../../../../schemas/password-decryption-receipt-v1.schema.json");
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptCase {
    Correct,
    Wrong,
    Absent,
}

impl ReceiptCase {
    fn required_result(self) -> &'static str {
        match self {
            Self::Correct => "pass",
            Self::Wrong | Self::Absent => "refused",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PasswordDecryptionReceipt {
    pub schema_version: String,
    pub fixture_id: String,
    pub source_sha256: String,
    pub format: String,
    pub algorithm_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kdf_id: Option<String>,
    pub credential_outcome: ReceiptCase,
    pub exercised_surfaces: Vec<String>,
    pub binary_version: String,
    pub binary_sha256: String,
    pub result: String,
    pub recorded_at: String,
}

impl PasswordDecryptionReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        fixture_id: String,
        source_sha256: String,
        format: String,
        algorithm_id: String,
        kdf_id: Option<String>,
        credential_outcome: ReceiptCase,
        exercised_surfaces: Vec<String>,
        binary_version: String,
        binary_sha256: String,
        result: String,
        recorded_at: String,
    ) -> Self {
        Self {
            schema_version: "password-decryption-receipt-v1".into(),
            fixture_id,
            source_sha256,
            format,
            algorithm_id,
            kdf_id,
            credential_outcome,
            exercised_surfaces,
            binary_version,
            binary_sha256,
            result,
            recorded_at,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExpectedFixture {
    pub fixture_id: String,
    pub format: String,
    pub role: String,
    pub credential_charset: String,
    pub source_sha256: String,
    pub algorithm_id: String,
    pub kdf_id: Option<String>,
}

impl ExpectedFixture {
    pub fn baseline(
        fixture_id: &str,
        format: &str,
        source_sha256: String,
        algorithm_id: &str,
        kdf_id: Option<&str>,
    ) -> Self {
        Self {
            fixture_id: fixture_id.into(),
            format: format.into(),
            role: "baseline".into(),
            credential_charset: "ascii".into(),
            source_sha256,
            algorithm_id: algorithm_id.into(),
            kdf_id: kdf_id.map(str::to_owned),
        }
    }

    pub fn non_ascii_success(
        fixture_id: &str,
        format: &str,
        source_sha256: String,
        algorithm_id: &str,
        kdf_id: Option<&str>,
    ) -> Self {
        Self {
            fixture_id: fixture_id.into(),
            format: format.into(),
            role: "non_ascii_success".into(),
            credential_charset: "non_ascii".into(),
            source_sha256,
            algorithm_id: algorithm_id.into(),
            kdf_id: kdf_id.map(str::to_owned),
        }
    }
}

pub fn validate_receipt_json(value: &Value) -> Result<(), String> {
    let schema: Value = serde_json::from_str(RECEIPT_SCHEMA)
        .map_err(|_| "password receipt schema is invalid".to_owned())?;
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .map_err(|_| "password receipt schema cannot be compiled".to_owned())?;
    if validator.is_valid(value) {
        Ok(())
    } else {
        Err("password receipt does not satisfy the closed schema".into())
    }
}

/// Publishes a schema-valid receipt without replacing an existing final file.
/// The hard-link step is an atomic no-clobber operation in the receipt directory.
pub fn write_receipt_atomic(
    receipt_dir: &Path,
    receipt: &PasswordDecryptionReceipt,
) -> Result<PathBuf, String> {
    if !receipt_dir.is_dir() {
        return Err("password receipt directory is unavailable".into());
    }
    let value = serde_json::to_value(receipt)
        .map_err(|_| "password receipt cannot be encoded".to_owned())?;
    validate_receipt_json(&value)?;
    let encoded =
        serde_json::to_vec(&value).map_err(|_| "password receipt cannot be encoded".to_owned())?;
    let name = receipt_name(&encoded);
    let destination = receipt_dir.join(name);
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = receipt_dir.join(format!(
        ".password-receipt-{}-{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "password receipt temporary file cannot be created".to_owned())?;
        file.write_all(&encoded)
            .map_err(|_| "password receipt cannot be written".to_owned())?;
        file.write_all(b"\n")
            .map_err(|_| "password receipt cannot be written".to_owned())?;
        file.sync_all()
            .map_err(|_| "password receipt cannot be synchronized".to_owned())?;
        fs::hard_link(&temporary, &destination)
            .map_err(|_| "password receipt already exists or cannot be published".to_owned())?;
        fs::remove_file(&temporary)
            .map_err(|_| "password receipt temporary file cannot be cleared".to_owned())?;
        Ok(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn validate_complete_run(
    receipt_dir: &Path,
    expected: &[ExpectedFixture],
) -> Result<(), String> {
    validate_expected_fixtures(expected)?;
    let entries = fs::read_dir(receipt_dir)
        .map_err(|_| "password receipt directory is unavailable".to_owned())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "password receipt directory cannot be inspected".to_owned())?;
    if entries.iter().any(|entry| {
        entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "json")
    }) {
        return Err("password receipt run contains an interrupted temporary file".into());
    }

    let expected_cases = expected_cases(expected);
    if entries.len() != expected_cases.len() {
        return Err("password receipt run is incomplete or contains duplicates".into());
    }
    let mut binary_identity: Option<(String, String)> = None;
    let mut seen = BTreeSet::new();
    for entry in entries {
        let bytes =
            fs::read(entry.path()).map_err(|_| "password receipt cannot be read".to_owned())?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "password receipt is not JSON".to_owned())?;
        validate_receipt_json(&value)?;
        let receipt: PasswordDecryptionReceipt = serde_json::from_value(value)
            .map_err(|_| "password receipt is malformed".to_owned())?;
        let key = receipt_key(&receipt);
        let fixture = expected
            .iter()
            .find(|fixture| fixture.fixture_id == receipt.fixture_id)
            .ok_or_else(|| "password receipt fixture is not manifest-matched".to_owned())?;
        if fixture.source_sha256 != receipt.source_sha256
            || fixture.format != receipt.format
            || fixture.algorithm_id != receipt.algorithm_id
            || fixture.kdf_id != receipt.kdf_id
            || receipt.exercised_surfaces.iter().collect::<BTreeSet<_>>()
                != BTreeSet::from([
                    &"cat".to_owned(),
                    &"convert".to_owned(),
                    &"render".to_owned(),
                ])
            || receipt.result != receipt.credential_outcome.required_result()
            || !expected_cases.contains(&key)
            || !seen.insert(key)
        {
            return Err("password receipt run contains an invalid or duplicate case".into());
        }
        match &binary_identity {
            Some(identity)
                if identity
                    != &(
                        receipt.binary_version.clone(),
                        receipt.binary_sha256.clone(),
                    ) =>
            {
                return Err("password receipt run mixes binary identities".into());
            }
            None => binary_identity = Some((receipt.binary_version, receipt.binary_sha256)),
            _ => {}
        }
    }
    if seen == expected_cases && binary_identity.is_some() {
        Ok(())
    } else {
        Err("password receipt run is incomplete".into())
    }
}

fn receipt_name(encoded: &[u8]) -> String {
    let digest = Sha256::digest(encoded);
    format!(
        "{}.json",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn receipt_key(receipt: &PasswordDecryptionReceipt) -> (String, ReceiptCase) {
    (receipt.fixture_id.clone(), receipt.credential_outcome)
}

fn expected_cases(expected: &[ExpectedFixture]) -> BTreeSet<(String, ReceiptCase)> {
    expected
        .iter()
        .flat_map(|fixture| {
            let cases = if fixture.role == "baseline" {
                vec![
                    ReceiptCase::Correct,
                    ReceiptCase::Wrong,
                    ReceiptCase::Absent,
                ]
            } else {
                vec![ReceiptCase::Correct]
            };
            cases
                .into_iter()
                .map(move |case| (fixture.fixture_id.clone(), case))
        })
        .collect()
}

fn validate_expected_fixtures(expected: &[ExpectedFixture]) -> Result<(), String> {
    if expected.len() != 3 {
        return Err("password receipt run requires exactly three selected fixtures".into());
    }
    let ids = expected
        .iter()
        .map(|fixture| fixture.fixture_id.as_str())
        .collect::<BTreeSet<_>>();
    if ids.len() != expected.len() {
        return Err("password receipt fixtures must be distinct".into());
    }
    for format in ["hwp5", "hwpx"] {
        if !expected.iter().any(|fixture| {
            fixture.role == "baseline"
                && fixture.credential_charset == "ascii"
                && fixture.format == format
        }) {
            return Err("both ASCII baseline formats are required".into());
        }
    }
    if expected
        .iter()
        .filter(|fixture| fixture.role == "baseline")
        .count()
        != 2
        || !expected.iter().any(|fixture| {
            fixture.role == "non_ascii_success" && fixture.credential_charset == "non_ascii"
        })
    {
        return Err("a distinct non-ASCII success fixture is required".into());
    }
    Ok(())
}
