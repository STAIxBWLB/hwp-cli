//! Synthetic contract coverage for the private Phase 8 password corpus manifest.
//!
//! Genuine document paths and credentials remain outside the repository. This
//! suite exercises only opaque identifiers and synthetic absolute paths.

#[path = "common/password_corpus_manifest.rs"]
mod password_corpus_manifest;

use password_corpus_manifest::{
    check_hwp5_budget, check_hwpx_budget, discovery_would_write_evidence, load_manifest_for_test,
    manifest_fixture, parse_hwpx_profile, repo_root, HwpxBufferSizes,
};

#[test]
fn manifest_contract_accepts_a_redacted_synthetic_matrix() {
    let manifest = manifest_fixture();
    let descriptors = load_manifest_for_test(&manifest, &repo_root(), |reference| {
        Some(format!("secret-for-{reference}"))
    })
    .expect("the synthetic manifest should satisfy the closed contract");

    assert_eq!(descriptors.len(), 3);
    assert!(descriptors.iter().any(|entry| {
        entry.fixture_id == "opaque-hwp5-baseline"
            && entry.format == "hwp5"
            && entry.role == "baseline"
            && entry.credential_charset == "ascii"
    }));
    assert!(descriptors.iter().all(|entry| entry.source_path.is_none()));
    assert!(descriptors.iter().all(|entry| entry.credential.is_some()));
}

#[test]
fn manifest_contract_rejects_closed_schema_and_role_violations() {
    for (label, manifest) in password_corpus_manifest::invalid_manifest_cases() {
        assert!(
            load_manifest_for_test(&manifest, &repo_root(), |_| Some("secret".to_owned())).is_err(),
            "{label} must fail closed"
        );
    }
}

#[test]
fn manifest_contract_without_a_manifest_does_not_publish_evidence() {
    assert!(
        !discovery_would_write_evidence(None),
        "missing owner configuration must stop before evidence publication"
    );
}

#[test]
fn budget_boundaries_accept_exact_limits_and_reject_overflow_before_allocation() {
    const MIB: u64 = 1024 * 1024;

    assert!(check_hwp5_budget(64 * MIB, 64 * MIB).is_ok());
    assert!(check_hwp5_budget(64 * MIB + 1, 0).is_err());
    assert!(check_hwp5_budget(u64::MAX, 1).is_err());

    assert!(
        check_hwpx_budget(
            "BinData/payload.bin",
            HwpxBufferSizes {
                ciphertext: 512 * MIB,
                decrypted_compressed: 512 * MIB,
                inflated: 512 * MIB,
                parser_owned: 512 * MIB,
            },
        )
        .is_ok()
    );
    assert!(
        check_hwpx_budget(
            "Contents/section0.xml",
            HwpxBufferSizes {
                ciphertext: 64 * 1024,
                decrypted_compressed: 64 * 1024,
                inflated: 64 * MIB + 1,
                parser_owned: 0,
            },
        )
        .is_err()
    );
    assert!(
        check_hwpx_budget(
            "BinData/ratio.bin",
            HwpxBufferSizes {
                ciphertext: 1,
                decrypted_compressed: 1,
                inflated: 1_001,
                parser_owned: 0,
            },
        )
        .is_err()
    );
    assert!(
        check_hwpx_budget(
            "BinData/overflow.bin",
            HwpxBufferSizes {
                ciphertext: u64::MAX,
                decrypted_compressed: 1,
                inflated: 1,
                parser_owned: 1,
            },
        )
        .is_err()
    );
}

#[test]
fn budget_boundaries_recognize_only_the_evidenced_hwpx_profile() {
    let profile = parse_hwpx_profile(password_corpus_manifest::supported_hwpx_manifest())
        .expect("the synthetic supported profile should parse");
    assert_eq!(profile.algorithm_id, "http://www.w3.org/2001/04/xmlenc#aes256-cbc");
    assert_eq!(
        profile.kdf_id,
        "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2"
    );
    assert!(parse_hwpx_profile(password_corpus_manifest::unsupported_hwpx_manifest()).is_err());
}
