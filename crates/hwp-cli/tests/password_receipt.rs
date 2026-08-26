//! Contract coverage for content-free password-decryption receipts.

#[path = "common/password_receipt.rs"]
mod password_receipt;

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::thread;

use password_receipt::{
    ExpectedFixture, PasswordDecryptionReceipt, ReceiptCase, validate_complete_run,
    validate_receipt_json, write_receipt_atomic,
};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hwp-cli-password-receipt-{}-{}-{label}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha(seed: char) -> String {
    seed.to_string().repeat(64)
}

fn fixtures() -> Vec<ExpectedFixture> {
    vec![
        ExpectedFixture::baseline(
            "opaque-hwp5-baseline",
            "hwp5",
            sha('a'),
            "hwp5-encrypt-version-4",
            Some("hwp5-password-transform"),
        ),
        ExpectedFixture::baseline(
            "opaque-hwpx-baseline",
            "hwpx",
            sha('b'),
            "http://www.w3.org/2001/04/xmlenc#aes256-cbc",
            Some("urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2"),
        ),
        ExpectedFixture::non_ascii_success(
            "opaque-unicode-success",
            "hwpx",
            sha('c'),
            "http://www.w3.org/2001/04/xmlenc#aes256-cbc",
            Some("urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2"),
        ),
    ]
}

fn receipt(fixture: &ExpectedFixture, case: ReceiptCase) -> PasswordDecryptionReceipt {
    PasswordDecryptionReceipt::new(
        fixture.fixture_id.clone(),
        fixture.source_sha256.clone(),
        fixture.format.clone(),
        fixture.algorithm_id.clone(),
        fixture.kdf_id.clone(),
        case,
        vec!["cat".into(), "convert".into(), "render".into()],
        "hwp-cli-test-version".into(),
        sha('d'),
        if case == ReceiptCase::Correct {
            "pass"
        } else {
            "refused"
        }
        .into(),
        "2026-08-26T00:00:00Z".into(),
    )
}

fn publish_complete_run(dir: &std::path::Path) {
    for fixture in fixtures() {
        let cases = if fixture.role == "baseline" {
            vec![
                ReceiptCase::Correct,
                ReceiptCase::Wrong,
                ReceiptCase::Absent,
            ]
        } else {
            vec![ReceiptCase::Correct]
        };
        for case in cases {
            write_receipt_atomic(dir, &receipt(&fixture, case)).unwrap();
        }
    }
}

#[test]
fn schema_is_closed_and_content_free() {
    let value = serde_json::to_value(receipt(&fixtures()[0], ReceiptCase::Correct)).unwrap();
    validate_receipt_json(&value).unwrap();
    for forbidden in [
        "source_path",
        "credential_ref",
        "credential",
        "key",
        "payload_sample",
        "decrypted_content_sha256",
        "diagnostic",
    ] {
        let mut invalid = value.clone();
        invalid[forbidden] = serde_json::json!("must-not-be-accepted");
        assert!(
            validate_receipt_json(&invalid).is_err(),
            "{forbidden} must be rejected"
        );
    }
}

#[test]
fn complete_run_requires_exact_seven_manifest_matched_cases() {
    let dir = temp_dir("complete");
    publish_complete_run(&dir);
    validate_complete_run(&dir, &fixtures()).unwrap();
}

#[test]
fn partial_duplicate_mixed_and_interrupted_runs_are_incomplete() {
    let dir = temp_dir("partial");
    let fixture = fixtures().remove(0);
    write_receipt_atomic(&dir, &receipt(&fixture, ReceiptCase::Correct)).unwrap();
    assert!(validate_complete_run(&dir, &fixtures()).is_err());

    let dir = temp_dir("duplicate");
    publish_complete_run(&dir);
    let duplicate = receipt(&fixtures()[0], ReceiptCase::Correct);
    let path = dir.join("duplicate.json");
    fs::write(path, serde_json::to_vec(&duplicate).unwrap()).unwrap();
    assert!(validate_complete_run(&dir, &fixtures()).is_err());

    let dir = temp_dir("mixed-binary");
    publish_complete_run(&dir);
    let mut mixed = receipt(&fixtures()[0], ReceiptCase::Correct);
    mixed.binary_sha256 = sha('e');
    fs::write(dir.join("mixed.json"), serde_json::to_vec(&mixed).unwrap()).unwrap();
    assert!(validate_complete_run(&dir, &fixtures()).is_err());

    let dir = temp_dir("interrupted");
    publish_complete_run(&dir);
    fs::write(dir.join(".password-receipt-interrupted.tmp"), b"partial").unwrap();
    assert!(validate_complete_run(&dir, &fixtures()).is_err());
}

#[test]
fn classifications_formats_and_distinct_fixture_roles_are_required() {
    let dir = temp_dir("wrong-classification");
    publish_complete_run(&dir);
    let mut expected = fixtures();
    expected[0].credential_charset = "non_ascii".into();
    assert!(validate_complete_run(&dir, &expected).is_err());

    let mut expected = fixtures();
    expected[2].credential_charset = "ascii".into();
    assert!(validate_complete_run(&dir, &expected).is_err());

    let mut expected = fixtures();
    expected.retain(|fixture| fixture.format != "hwp5");
    assert!(validate_complete_run(&dir, &expected).is_err());

    let mut expected = fixtures();
    expected[2].fixture_id = expected[0].fixture_id.clone();
    assert!(validate_complete_run(&dir, &expected).is_err());
}

#[test]
fn no_clobber_races_and_independent_directories_do_not_share_state() {
    let dir = Arc::new(temp_dir("race"));
    let fixture = Arc::new(fixtures().remove(0));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let dir = Arc::clone(&dir);
        let fixture = Arc::clone(&fixture);
        workers.push(thread::spawn(move || {
            write_receipt_atomic(&dir, &receipt(&fixture, ReceiptCase::Correct))
        }));
    }
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(fs::read_dir(&*dir).unwrap().all(|entry| {
        entry
            .unwrap()
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
    }));

    let left = temp_dir("parallel-left");
    let right = temp_dir("parallel-right");
    let left_fixture = fixtures().remove(0);
    let right_fixture = fixtures().remove(1);
    let left_receipt = receipt(&left_fixture, ReceiptCase::Correct);
    let right_receipt = receipt(&right_fixture, ReceiptCase::Correct);
    let left_result = thread::spawn(move || write_receipt_atomic(&left, &left_receipt));
    let right_result = thread::spawn(move || write_receipt_atomic(&right, &right_receipt));
    assert!(left_result.join().unwrap().is_ok());
    assert!(right_result.join().unwrap().is_ok());
}

#[test]
fn receipt_case_is_serialized_as_a_closed_outcome() {
    let cases = BTreeMap::from([
        (ReceiptCase::Correct, "correct"),
        (ReceiptCase::Wrong, "wrong"),
        (ReceiptCase::Absent, "absent"),
    ]);
    for (case, expected) in cases {
        let value = serde_json::to_value(receipt(&fixtures()[0], case)).unwrap();
        assert_eq!(value["credential_outcome"], expected);
    }
}
