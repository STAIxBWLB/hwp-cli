//! Test-only validation for the owner-controlled password corpus contract.
//!
//! The helper deliberately returns redacted descriptors. It never exposes a
//! source path or serializes the resolved `Zeroizing<String>` credential.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
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
