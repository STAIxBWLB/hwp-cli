//! Synthetic contract coverage for the private Phase 8 password corpus manifest.
//!
//! Genuine document paths and credentials remain outside the repository. This
//! suite exercises only opaque identifiers and synthetic absolute paths.

#[path = "common/password_corpus_manifest.rs"]
mod password_corpus_manifest;

use password_corpus_manifest::{
    Hwp5ProbeObservation, Hwp5ValidatedStream, HwpxBufferSizes, HwpxProbeFailureStage,
    after_hwp5_discovery_gate, check_hwp5_budget, check_hwp5_discovery_eligibility,
    check_hwpx_budget, diagnose_hwpx_password_profile, discovery_would_write_evidence,
    hwpx_integrity_error, hwpx_unsupported_profile_error, load_manifest_for_test, manifest_fixture,
    owner_manifest_path, parse_hwpx_profile, probe_hwp5_encrypt_version_4,
    probe_hwpx_password_profile, repo_root, run_owner_discovery, run_owner_discovery_from_env,
    run_owner_profile_diagnostic, run_owner_profile_diagnostic_from_env,
    serialize_hwp5_profile_observation, serialize_hwpx_profile_observation,
    transform_hwp5_encrypt_version_4_in_place, validate_hwp5_record_identity,
};

struct SyntheticHwpxOptions {
    raw_deflate: Vec<u8>,
    padding: Vec<u8>,
    declared_plaintext_bytes: u64,
    stored: bool,
    corrupt_checksum: bool,
    truncate_ciphertext: bool,
    kdf_id: &'static str,
}

fn raw_deflate(plaintext: &[u8]) -> Vec<u8> {
    use std::io::Write as _;

    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::none());
    encoder
        .write_all(plaintext)
        .expect("synthetic deflate input writes");
    encoder.finish().expect("synthetic deflate completes")
}

fn synthetic_deflate_with_padding(target_padding: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    for payload_len in 1..4096 {
        let plaintext = format!("<doc>{}</doc>", "x".repeat(payload_len)).into_bytes();
        let compressed = raw_deflate(&plaintext);
        let padding_len = (16 - compressed.len() % 16) % 16;
        if padding_len == target_padding {
            return (plaintext, compressed, vec![0; padding_len]);
        }
    }
    panic!("uncompressed synthetic payloads cover every AES padding residue")
}

fn synthetic_hwpx_path(label: &str, options: SyntheticHwpxOptions) -> std::path::PathBuf {
    use aes::Aes256;
    use aes::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::NoPadding};
    use base64::Engine as _;
    use sha2::Digest as _;
    use std::io::Write as _;

    const PASSWORD: &str = "synthetic-password";
    const SALT: [u8; 16] = [7; 16];
    const IV: [u8; 16] = [9; 16];
    let nonce = format!("{}-{}", std::process::id(), label);
    let path = std::env::temp_dir().join(format!("hwp-cli-hwpx-{nonce}.hwpx"));
    let _ = std::fs::remove_file(&path);

    let mut start_key = sha2::Sha256::digest(PASSWORD.as_bytes()).to_vec();
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&start_key, &SALT, 1024, &mut key);
    start_key.fill(0);

    let mut padded = options.raw_deflate.clone();
    padded.extend_from_slice(&options.padding);
    assert!(!padded.is_empty() && padded.len().is_multiple_of(16));
    let padded_len = padded.len();
    let ciphertext = cbc::Encryptor::<Aes256>::new_from_slices(&key, &IV)
        .expect("synthetic AES setup")
        .encrypt_padded::<NoPadding>(&mut padded, padded_len)
        .expect("synthetic no-padding encryption")
        .to_vec();
    key.fill(0);
    let ciphertext = if options.truncate_ciphertext {
        ciphertext[..ciphertext.len() - 1].to_vec()
    } else {
        ciphertext
    };
    let mut checksum = sha2::Sha256::digest(&options.raw_deflate).to_vec();
    if options.corrupt_checksum {
        checksum[0] ^= 0xff;
    }
    let manifest = format!(
        r#"<?xml version="1.0"?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><odf:file-entry full-path="Contents/section0.xml" size="{}"><odf:encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="{}"><odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="{}"/><odf:key-derivation key-derivation-name="{}" key-size="32" iteration-count="1024" salt="{}"/><odf:start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256"/></odf:encryption-data></odf:file-entry></odf:manifest>"#,
        options.declared_plaintext_bytes,
        base64::engine::general_purpose::STANDARD.encode(checksum),
        base64::engine::general_purpose::STANDARD.encode(IV),
        options.kdf_id,
        base64::engine::general_purpose::STANDARD.encode(SALT),
    );

    let file = std::fs::File::create(&path).expect("synthetic HWPX is writable");
    let mut zip = zip::ZipWriter::new(file);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("mimetype", stored)
        .expect("mimetype entry starts");
    zip.write_all(b"application/hwp+zip")
        .expect("mimetype entry writes");
    zip.start_file("META-INF/manifest.xml", stored)
        .expect("manifest entry starts");
    zip.write_all(manifest.as_bytes())
        .expect("manifest entry writes");
    let entry_options =
        zip::write::SimpleFileOptions::default().compression_method(if options.stored {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        });
    zip.start_file("Contents/section0.xml", entry_options)
        .expect("protected entry starts");
    zip.write_all(&ciphertext).expect("protected entry writes");
    zip.finish().expect("synthetic HWPX finalizes");
    path
}

fn supported_options(
    raw_deflate: Vec<u8>,
    padding: Vec<u8>,
    declared_plaintext_bytes: u64,
) -> SyntheticHwpxOptions {
    SyntheticHwpxOptions {
        raw_deflate,
        padding,
        declared_plaintext_bytes,
        stored: true,
        corrupt_checksum: false,
        truncate_ciphertext: false,
        kdf_id: "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2",
    }
}

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
fn hwpx_discovery_accepts_only_no_padding_zero_to_block_profiles() {
    for padding_len in 1..=15 {
        let (plaintext, compressed, padding) = synthetic_deflate_with_padding(padding_len);
        let path = synthetic_hwpx_path(
            &format!("zero-padding-{padding_len}"),
            supported_options(compressed, padding, plaintext.len() as u64),
        );
        let observation = probe_hwpx_password_profile(&path, "synthetic-password")
            .expect("a 1-15 byte zero suffix must validate");
        assert_eq!(observation.validated_entry_count, 1);
        let _ = std::fs::remove_file(path);
    }

    let (plaintext, compressed, padding) = synthetic_deflate_with_padding(0);
    let path = synthetic_hwpx_path(
        "exact-aes-block",
        supported_options(compressed, padding, plaintext.len() as u64),
    );
    probe_hwpx_password_profile(&path, "synthetic-password")
        .expect("an exact AES block requires no zero suffix");
    let _ = std::fs::remove_file(path);
}

#[test]
fn hwpx_discovery_collapses_credential_and_integrity_failures() {
    let (plaintext, compressed, padding) = synthetic_deflate_with_padding(1);
    let (block_plaintext, block_compressed, _) = synthetic_deflate_with_padding(0);
    let invalid_deflate = vec![0x07, 0x55, 0xaa];
    let invalid_padding = vec![0; 16 - invalid_deflate.len()];
    let invalid_xml = b"<doc>";
    let invalid_xml_compressed = raw_deflate(invalid_xml);
    let invalid_xml_padding = vec![0; (16 - invalid_xml_compressed.len() % 16) % 16];
    let cases = [
        (
            "nonzero-suffix",
            SyntheticHwpxOptions {
                padding: vec![1],
                ..supported_options(compressed.clone(), padding.clone(), plaintext.len() as u64)
            },
            "synthetic-password",
        ),
        (
            "sixteen-zero-suffix",
            SyntheticHwpxOptions {
                padding: vec![0; 16],
                raw_deflate: block_compressed,
                ..supported_options(Vec::new(), Vec::new(), block_plaintext.len() as u64)
            },
            "synthetic-password",
        ),
        (
            "invalid-deflate",
            supported_options(invalid_deflate, invalid_padding, plaintext.len() as u64),
            "synthetic-password",
        ),
        (
            "invalid-xml",
            supported_options(
                invalid_xml_compressed,
                invalid_xml_padding,
                invalid_xml.len() as u64,
            ),
            "synthetic-password",
        ),
        (
            "truncated-ciphertext",
            SyntheticHwpxOptions {
                truncate_ciphertext: true,
                ..supported_options(compressed.clone(), padding.clone(), plaintext.len() as u64)
            },
            "synthetic-password",
        ),
        (
            "corrupt-checksum",
            SyntheticHwpxOptions {
                corrupt_checksum: true,
                ..supported_options(compressed.clone(), padding.clone(), plaintext.len() as u64)
            },
            "synthetic-password",
        ),
        (
            "wrong-password",
            supported_options(compressed.clone(), padding.clone(), plaintext.len() as u64),
            "not-the-password",
        ),
    ];

    for (label, options, password) in cases {
        let path = synthetic_hwpx_path(label, options);
        assert_eq!(
            probe_hwpx_password_profile(&path, password).unwrap_err(),
            hwpx_integrity_error(),
            "{label} must not expose its failure stage"
        );
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn owner_discovery_redacts_probe_failures_to_fixture_and_format() {
    use serde_json::json;

    let nonce = format!("{}-owner-error", std::process::id());
    let hwp5_path = std::env::temp_dir().join(format!("hwp-cli-{nonce}-baseline.hwp"));
    let hwpx_path = std::env::temp_dir().join(format!("hwp-cli-{nonce}-baseline.hwpx"));
    let unicode_path = std::env::temp_dir().join(format!("hwp-cli-{nonce}-unicode.hwpx"));
    let manifest_path = std::env::temp_dir().join(format!("hwp-cli-{nonce}.json"));
    let evidence_path = std::env::temp_dir().join(format!("hwp-cli-{nonce}-evidence.json"));

    std::fs::write(&hwp5_path, b"not a protected HWP5 document")
        .expect("synthetic HWP5 source is writable");
    let (plaintext, compressed, padding) = synthetic_deflate_with_padding(1);
    let corrupt_hwpx_path = synthetic_hwpx_path(
        "owner-corrupt-integrity",
        SyntheticHwpxOptions {
            corrupt_checksum: true,
            ..supported_options(compressed.clone(), padding.clone(), plaintext.len() as u64)
        },
    );
    let valid_hwpx_path = synthetic_hwpx_path(
        "owner-wrong-password",
        supported_options(compressed, padding, plaintext.len() as u64),
    );
    let (unsupported_plaintext, unsupported_compressed, unsupported_padding) =
        synthetic_deflate_with_padding(1);
    let unsupported_hwpx_path = synthetic_hwpx_path(
        "owner-unsupported-profile",
        SyntheticHwpxOptions {
            kdf_id: "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2-hmac-sha256",
            ..supported_options(
                unsupported_compressed,
                unsupported_padding,
                unsupported_plaintext.len() as u64,
            )
        },
    );
    std::fs::write(&hwpx_path, b"unreached placeholder").expect("placeholder source is writable");
    std::fs::write(&unicode_path, b"unreached placeholder")
        .expect("placeholder source is writable");
    let private_paths = [
        hwp5_path.to_string_lossy().into_owned(),
        hwpx_path.to_string_lossy().into_owned(),
        unicode_path.to_string_lossy().into_owned(),
        corrupt_hwpx_path.to_string_lossy().into_owned(),
        valid_hwpx_path.to_string_lossy().into_owned(),
        unsupported_hwpx_path.to_string_lossy().into_owned(),
    ];

    let manifest_for = |first: serde_json::Value| {
        json!({
            "version": "password-corpus-manifest-v1",
            "fixtures": [
                first,
                {
                    "fixture_id": "opaque-hwp5-baseline",
                    "source_path": hwp5_path,
                    "format": "hwp5",
                    "role": "baseline",
                    "credential_charset": "ascii",
                    "credential_ref": "PRIVATE_HWP5_REF"
                },
                {
                    "fixture_id": "opaque-non-ascii-success",
                    "source_path": unicode_path,
                    "format": "hwpx",
                    "role": "non_ascii_success",
                    "credential_charset": "non_ascii",
                    "credential_ref": "PRIVATE_UNICODE_REF"
                }
            ]
        })
    };
    let run = |manifest: serde_json::Value, expected: &str, password: &str| {
        std::fs::write(&manifest_path, manifest.to_string()).expect("manifest is writable");
        let error = run_owner_discovery(&manifest_path, &evidence_path, |_| {
            Some(password.to_owned())
        })
        .expect_err("the selected fixture must fail before evidence publication");
        assert_eq!(error, expected);
        for forbidden in [
            "/tmp/",
            "PRIVATE_",
            "synthetic-password",
            "not-the-password",
            "checksum",
            "inflate",
            "ciphertext",
            "plaintext",
            "padding",
            "container",
            "record",
            "stream",
            "not a protected HWP5 document",
        ] {
            assert!(
                !error.contains(forbidden),
                "owner diagnostic must not expose {forbidden}"
            );
        }
        for private_path in &private_paths {
            assert!(
                !error.contains(private_path),
                "owner diagnostic must not expose a private source path"
            );
        }
        assert!(
            !evidence_path.exists(),
            "failed discovery must not publish evidence"
        );
    };

    let baseline = |source_path: &std::path::Path| {
        json!({
            "fixture_id": "opaque-hwpx-baseline",
            "source_path": source_path,
            "format": "hwpx",
            "role": "baseline",
            "credential_charset": "ascii",
            "credential_ref": "PRIVATE_HWPX_REF"
        })
    };
    let expected_baseline =
        "fixture opaque-hwpx-baseline (hwpx): credential or integrity validation failed";
    run(
        manifest_for(baseline(&corrupt_hwpx_path)),
        expected_baseline,
        "synthetic-password",
    );
    run(
        manifest_for(baseline(&valid_hwpx_path)),
        expected_baseline,
        "not-the-password",
    );
    run(
        manifest_for(baseline(&unsupported_hwpx_path)),
        "fixture opaque-hwpx-baseline (hwpx): protected profile is unsupported",
        "synthetic-password",
    );

    let unicode_first = json!({
        "fixture_id": "opaque-non-ascii-success",
        "source_path": corrupt_hwpx_path,
        "format": "hwpx",
        "role": "non_ascii_success",
        "credential_charset": "non_ascii",
        "credential_ref": "PRIVATE_UNICODE_REF"
    });
    let unicode_manifest = json!({
        "version": "password-corpus-manifest-v1",
        "fixtures": [
            unicode_first,
            {
                "fixture_id": "opaque-hwp5-baseline",
                "source_path": hwp5_path,
                "format": "hwp5",
                "role": "baseline",
                "credential_charset": "ascii",
                "credential_ref": "PRIVATE_HWP5_REF"
            },
            {
                "fixture_id": "opaque-hwpx-baseline",
                "source_path": hwpx_path,
                "format": "hwpx",
                "role": "baseline",
                "credential_charset": "ascii",
                "credential_ref": "PRIVATE_HWPX_REF"
            }
        ]
    });
    let expected_unicode =
        "fixture opaque-non-ascii-success (hwpx): credential or integrity validation failed";
    run(unicode_manifest, expected_unicode, "synthetic-password");
    assert_ne!(
        expected_baseline, expected_unicode,
        "fixture IDs must distinguish roles"
    );

    let hwp5_manifest = json!({
        "version": "password-corpus-manifest-v1",
        "fixtures": [
            {
                "fixture_id": "opaque-hwp5-baseline",
                "source_path": hwp5_path,
                "format": "hwp5",
                "role": "baseline",
                "credential_charset": "ascii",
                "credential_ref": "PRIVATE_HWP5_REF"
            },
            {
                "fixture_id": "opaque-hwpx-baseline",
                "source_path": hwpx_path,
                "format": "hwpx",
                "role": "baseline",
                "credential_charset": "ascii",
                "credential_ref": "PRIVATE_HWPX_REF"
            },
            {
                "fixture_id": "opaque-non-ascii-success",
                "source_path": unicode_path,
                "format": "hwpx",
                "role": "non_ascii_success",
                "credential_charset": "non_ascii",
                "credential_ref": "PRIVATE_UNICODE_REF"
            }
        ]
    });
    run(
        hwp5_manifest,
        "fixture opaque-hwp5-baseline (hwp5): credential or integrity validation failed",
        "synthetic-password",
    );

    for path in [
        hwp5_path,
        hwpx_path,
        unicode_path,
        manifest_path,
        evidence_path,
        corrupt_hwpx_path,
        valid_hwpx_path,
        unsupported_hwpx_path,
    ] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn hwpx_discovery_rejects_unsupported_profiles_before_credential_work() {
    let hmac_sha256 = password_corpus_manifest::supported_hwpx_manifest()
        .replace("#pbkdf2\"", "#pbkdf2-hmac-sha256\"");
    assert_eq!(
        parse_hwpx_profile(&hmac_sha256).unwrap_err(),
        "HWPX profile is unsupported or incomplete"
    );

    let (plaintext, compressed, padding) = synthetic_deflate_with_padding(1);
    let path = synthetic_hwpx_path(
        "unsupported-hmac-sha256",
        SyntheticHwpxOptions {
            kdf_id: "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2-hmac-sha256",
            ..supported_options(compressed, padding, plaintext.len() as u64)
        },
    );
    assert_eq!(
        probe_hwpx_password_profile(&path, "synthetic-password").unwrap_err(),
        hwpx_unsupported_profile_error()
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn hwpx_discovery_rejects_zip_compression_and_declared_size_before_allocation() {
    let (plaintext, compressed, padding) = synthetic_deflate_with_padding(1);
    let stored_path = synthetic_hwpx_path(
        "zip-deflated-entry",
        SyntheticHwpxOptions {
            stored: false,
            ..supported_options(compressed.clone(), padding.clone(), plaintext.len() as u64)
        },
    );
    assert_eq!(
        probe_hwpx_password_profile(&stored_path, "synthetic-password").unwrap_err(),
        hwpx_integrity_error()
    );
    let _ = std::fs::remove_file(stored_path);

    for (label, declared_plaintext_bytes) in [
        ("xml-bound", 64 * 1024 * 1024 + 1),
        ("ratio-bound", 1_000_000),
        ("declared-size-stream", plaintext.len() as u64 - 1),
    ] {
        let path = synthetic_hwpx_path(
            label,
            supported_options(
                compressed.clone(),
                padding.clone(),
                declared_plaintext_bytes,
            ),
        );
        assert_eq!(
            probe_hwpx_password_profile(&path, "synthetic-password").unwrap_err(),
            hwpx_integrity_error(),
            "{label} must be rejected before output allocation"
        );
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn hwpx_profile_evidence_is_limited_to_selected_content_free_facts() {
    let (plaintext, compressed, padding) = synthetic_deflate_with_padding(1);
    let path = synthetic_hwpx_path(
        "redacted-evidence",
        supported_options(compressed, padding, plaintext.len() as u64),
    );
    let observation = probe_hwpx_password_profile(&path, "synthetic-password")
        .expect("selected profile validates before it is serializable");
    let evidence = serialize_hwpx_profile_observation(observation);
    let keys = evidence
        .as_object()
        .expect("profile evidence is an object")
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        keys,
        [
            "algorithm_id",
            "cbc_padding",
            "checksum_id",
            "format",
            "kdf_id",
            "pbkdf2_prf",
            "protected_entry_bytes",
            "protected_entry_count",
            "start_key_id",
            "validated_entry_count",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert_eq!(evidence["pbkdf2_prf"], "hmac-sha1");
    assert_eq!(evidence["cbc_padding"], "zero-to-aes-block");
    let encoded = evidence.to_string();
    for forbidden in [
        "password",
        "private",
        "plaintext",
        "ciphertext",
        "synthetic-password",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "evidence must not contain {forbidden}"
        );
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn hwpx_diagnostic_stage_vocabulary_is_closed() {
    let labels = HwpxProbeFailureStage::ALL.map(|stage| {
        serde_json::to_value(stage)
            .expect("closed diagnostic stage serializes")
            .as_str()
            .expect("diagnostic stage is a string")
            .to_owned()
    });
    assert_eq!(
        labels,
        [
            "ciphertext_storage_profile",
            "key_derivation_cipher_init",
            "zero_suffix",
            "deflate_stream",
            "compressed_checksum",
            "declared_size",
            "xml_structure",
            "bounds",
        ]
    );
}

#[test]
fn owner_profile_diagnostic_outputs_only_closed_redacted_metadata() {
    use serde_json::json;

    let nonce = format!("{}-owner-diagnostic", std::process::id());
    let hwp5_path = std::env::temp_dir().join(format!("hwp-cli-{nonce}.hwp"));
    let manifest_path = std::env::temp_dir().join(format!("hwp-cli-{nonce}.json"));
    std::fs::write(&hwp5_path, b"synthetic invalid hwp5")
        .expect("synthetic HWP5 source is writable");
    let (plaintext, compressed, padding) = synthetic_deflate_with_padding(1);
    let corrupt_hwpx_path = synthetic_hwpx_path(
        "diagnostic-corrupt-checksum",
        SyntheticHwpxOptions {
            corrupt_checksum: true,
            ..supported_options(compressed.clone(), padding.clone(), plaintext.len() as u64)
        },
    );
    let valid_hwpx_path = synthetic_hwpx_path(
        "diagnostic-valid",
        supported_options(compressed, padding, plaintext.len() as u64),
    );
    let manifest = json!({
        "version": "password-corpus-manifest-v1",
        "fixtures": [
            {
                "fixture_id": "opaque-hwp5-baseline",
                "source_path": hwp5_path,
                "format": "hwp5",
                "role": "baseline",
                "credential_charset": "ascii",
                "credential_ref": "PRIVATE_HWP5_REF"
            },
            {
                "fixture_id": "opaque-hwpx-baseline",
                "source_path": corrupt_hwpx_path,
                "format": "hwpx",
                "role": "baseline",
                "credential_charset": "ascii",
                "credential_ref": "PRIVATE_HWPX_REF"
            },
            {
                "fixture_id": "opaque-unicode-success",
                "source_path": valid_hwpx_path,
                "format": "hwpx",
                "role": "non_ascii_success",
                "credential_charset": "non_ascii",
                "credential_ref": "PRIVATE_UNICODE_REF"
            }
        ]
    });
    std::fs::write(&manifest_path, manifest.to_string()).expect("manifest is writable");

    let lines =
        run_owner_profile_diagnostic(&manifest_path, |_| Some("synthetic-password".to_owned()))
            .expect("diagnostic runs all owner fixtures without publishing evidence");
    assert_eq!(lines.len(), 3);
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line).expect("diagnostic is JSON");
        let keys = value
            .as_object()
            .expect("diagnostic is an object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            ["algorithm_id", "fixture_id", "format", "kdf_id", "result"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        for forbidden in [
            "/tmp/",
            "PRIVATE_",
            "synthetic-password",
            "source_path",
            "credential_ref",
            "plaintext",
            "ciphertext\"",
            "salt",
            "initialisation-vector",
        ] {
            assert!(
                !line.contains(forbidden),
                "diagnostic output must not expose {forbidden}"
            );
        }
    }
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"result\":\"hwp5_probe\""))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"result\":\"compressed_checksum\""))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("\"result\":\"validated\""))
    );

    for path in [hwp5_path, manifest_path, corrupt_hwpx_path, valid_hwpx_path] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn hwpx_diagnostic_stage_does_not_widen_normal_discovery_errors() {
    let (plaintext, compressed, padding) = synthetic_deflate_with_padding(1);
    let path = synthetic_hwpx_path(
        "diagnostic-normalization",
        SyntheticHwpxOptions {
            corrupt_checksum: true,
            ..supported_options(compressed, padding, plaintext.len() as u64)
        },
    );
    assert_eq!(
        diagnose_hwpx_password_profile(&path, "synthetic-password").stage,
        Some(HwpxProbeFailureStage::CompressedChecksum)
    );
    assert_eq!(
        probe_hwpx_password_profile(&path, "synthetic-password").unwrap_err(),
        hwpx_integrity_error(),
        "the normal public probe must keep diagnostic stage details collapsed"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
#[ignore = "requires owner-controlled password corpus"]
fn discover_owner_profiles() {
    run_owner_discovery_from_env()
        .expect("owner discovery must publish only after all profile probes validate");
}

#[test]
#[ignore = "requires explicit owner diagnostic authorization and password corpus"]
fn diagnose_owner_profiles() {
    for line in run_owner_profile_diagnostic_from_env()
        .expect("owner diagnostic must have explicit authorization, manifest, and credentials")
    {
        println!("{line}");
    }
}
