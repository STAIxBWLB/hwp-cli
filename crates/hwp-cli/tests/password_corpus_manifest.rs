//! Synthetic contract coverage for the private Phase 8 password corpus manifest.
//!
//! Genuine document paths and credentials remain outside the repository. This
//! suite exercises only opaque identifiers and synthetic absolute paths.

#[path = "common/password_corpus_manifest.rs"]
mod password_corpus_manifest;

use password_corpus_manifest::{
    Hwp5ProbeObservation, Hwp5ValidatedStream, HwpxBufferSizes, after_hwp5_discovery_gate,
    check_hwp5_budget, check_hwp5_discovery_eligibility, check_hwpx_budget,
    discovery_would_write_evidence, load_manifest_for_test, manifest_fixture, owner_manifest_path,
    parse_hwpx_profile, probe_hwp5_encrypt_version_4, repo_root, run_owner_discovery,
    run_owner_discovery_from_env, serialize_hwp5_profile_observation,
    transform_hwp5_encrypt_version_4_in_place, validate_hwp5_record_identity,
};

fn synthetic_hwp5_header(attributes: u32, encrypt_version: u32) -> hwp5::FileHeader {
    let mut bytes = vec![0u8; hwp5::file_header::FILE_HEADER_SIZE];
    bytes[..hwp5::file_header::SIGNATURE.len()].copy_from_slice(hwp5::file_header::SIGNATURE);
    bytes[32..36].copy_from_slice(&0x05000300u32.to_le_bytes());
    bytes[36..40].copy_from_slice(&attributes.to_le_bytes());
    bytes[44..48].copy_from_slice(&encrypt_version.to_le_bytes());
    hwp5::FileHeader::parse(&bytes).expect("synthetic HWP5 header is valid")
}

fn synthetic_record_stream(tag: u16) -> hwp5::record::ScanResult {
    let bytes = u32::from(tag).to_le_bytes();
    hwp5::record::scan_stream(&bytes, hwp5::record::ScanMode::Strict)
        .expect("single zero-length record is structurally valid")
}

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
    let profile = parse_hwpx_profile(&password_corpus_manifest::supported_hwpx_manifest())
        .expect("the synthetic supported profile should parse");
    assert_eq!(
        profile.algorithm_id,
        "http://www.w3.org/2001/04/xmlenc#aes256-cbc"
    );
    assert_eq!(
        profile.kdf_id,
        "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2"
    );
    assert!(parse_hwpx_profile(&password_corpus_manifest::unsupported_hwpx_manifest()).is_err());
}

#[test]
fn budget_boundaries_keep_the_hwp5_candidate_transform_in_memory() {
    let _: fn(&std::path::Path, &str) -> Result<Hwp5ProbeObservation, String> =
        probe_hwp5_encrypt_version_4;
    let mut ciphertext = [0x5au8; 31];
    let original = ciphertext;
    transform_hwp5_encrypt_version_4_in_place(&mut ciphertext, "synthetic-password")
        .expect("candidate transform should accept a bounded synthetic stream");
    assert_ne!(ciphertext, original);
}

#[test]
fn hwp5_discovery_rejects_unmarked_inconsistent_and_unsupported_profiles_before_access() {
    const ENCRYPTED: u32 = 1 << 1;
    const COMPRESSED: u32 = 1 << 0;
    let unmarked = synthetic_hwp5_header(COMPRESSED, 4);
    assert_eq!(
        check_hwp5_discovery_eligibility(&unmarked).unwrap_err(),
        "HWP5 is not marked encrypted"
    );

    let inconsistent = synthetic_hwp5_header(COMPRESSED | ENCRYPTED, 0);
    assert_eq!(
        check_hwp5_discovery_eligibility(&inconsistent).unwrap_err(),
        "HWP5 encrypted bit + EncryptVersion=0 is internally inconsistent"
    );
    let mut accessed = false;
    assert!(
        after_hwp5_discovery_gate(&inconsistent, || {
            accessed = true;
            transform_hwp5_encrypt_version_4_in_place(&mut [0u8; 16], "synthetic")
        })
        .is_err()
    );
    assert!(
        !accessed,
        "EncryptVersion=0 must be rejected before stream reads or transform access"
    );

    for version in [1, 5] {
        let header = synthetic_hwp5_header(COMPRESSED | ENCRYPTED, version);
        assert_eq!(
            check_hwp5_discovery_eligibility(&header).unwrap_err(),
            format!("HWP5 EncryptVersion={version} is unsupported")
        );
    }

    let candidate = synthetic_hwp5_header(COMPRESSED | ENCRYPTED, 4);
    let mut candidate_accessed = false;
    after_hwp5_discovery_gate(&candidate, || {
        candidate_accessed = true;
        Ok(())
    })
    .expect("EncryptVersion 4 remains the only candidate");
    assert!(candidate_accessed);
}

#[test]
fn hwp5_discovery_refuses_non_password_protection_before_password_probing() {
    const ENCRYPTED: u32 = 1 << 1;
    const DISTRIBUTION: u32 = 1 << 2;
    const DRM: u32 = 1 << 4;
    const SIGNED: u32 = 1 << 7;
    const CERT_ENCRYPTED: u32 = 1 << 8;
    const CERT_DRM: u32 = 1 << 10;
    for (flag, message) in [
        (
            CERT_ENCRYPTED,
            "HWP5 certificate encryption must not enter password discovery",
        ),
        (
            CERT_DRM,
            "HWP5 certificate DRM must not enter password discovery",
        ),
        (DRM, "HWP5 DRM must not enter password discovery"),
        (
            SIGNED,
            "HWP5 digital signature must not enter password discovery",
        ),
        (
            DISTRIBUTION,
            "HWP5 distribution document must not enter password discovery",
        ),
    ] {
        let header = synthetic_hwp5_header(ENCRYPTED | flag, 4);
        assert_eq!(
            check_hwp5_discovery_eligibility(&header).unwrap_err(),
            message
        );
    }
}

#[test]
fn hwp5_discovery_requires_record_identity_and_serializes_only_cfb_evidence() {
    let doc_info = synthetic_record_stream(hwp5::record::tag::DOCUMENT_PROPERTIES);
    validate_hwp5_record_identity("/DocInfo", &doc_info)
        .expect("DocInfo must identify itself with DOCUMENT_PROPERTIES");
    let body = synthetic_record_stream(hwp5::record::tag::PARA_HEADER);
    validate_hwp5_record_identity("/BodyText/Section0", &body)
        .expect("BodyText must identify itself with PARA_HEADER");
    let generic = synthetic_record_stream(hwp5::record::tag::FACE_NAME);
    assert!(
        validate_hwp5_record_identity("/DocInfo", &generic).is_err(),
        "a structurally valid but generic record stream must not validate"
    );

    let evidence = serialize_hwp5_profile_observation(&Hwp5ProbeObservation {
        encrypt_version: 4,
        cfb_stream_count: 3,
        cfb_stream_bytes: 32,
        validated_record_stream_count: 2,
        validated_record_streams: vec![
            Hwp5ValidatedStream {
                path: "/DocInfo".to_owned(),
                size: 8,
            },
            Hwp5ValidatedStream {
                path: "/BodyText/Section0".to_owned(),
                size: 24,
            },
        ],
    });
    assert_eq!(evidence["validated_record_streams"][0]["path"], "/DocInfo");
    assert_eq!(evidence["validated_record_streams"][1]["size"], 24);
    let encoded = evidence.to_string();
    assert!(!encoded.contains("/private/") && !encoded.contains("synthetic-password"));
}

#[test]
fn owner_discovery_entrypoint_is_present_and_ignored() {
    let source = include_str!("password_corpus_manifest.rs");
    assert!(source.contains("fn discover_owner_profiles"));
    assert!(source.contains("#[ignore = \"requires owner-controlled password corpus\"]"));
}

#[test]
fn owner_discovery_without_manifest_fails_closed_without_evidence() {
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    );
    let absent_manifest =
        std::env::temp_dir().join(format!("hwp-cli-missing-manifest-{nonce}.json"));
    let evidence = std::env::temp_dir().join(format!("hwp-cli-missing-evidence-{nonce}.json"));
    let _ = std::fs::remove_file(&evidence);

    assert!(
        owner_manifest_path(None).is_err(),
        "missing environment must stop discovery"
    );
    assert!(
        run_owner_discovery(&absent_manifest, &evidence, |_| None).is_err(),
        "missing owner setup must stop before publication"
    );
    assert!(
        !evidence.exists(),
        "missing owner setup must leave no evidence artifact"
    );
}

#[test]
#[ignore = "requires owner-controlled password corpus"]
fn discover_owner_profiles() {
    run_owner_discovery_from_env()
        .expect("owner discovery must publish only after all profile probes validate");
}
