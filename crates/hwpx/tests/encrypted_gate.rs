//! GATE-02 HWPX proof: the committed sample plus the one genuine encrypted package.
//!
//! Two of the three tests here are CI-safe and always run: the committed fixture
//! (`fixtures/samples/report-tables.hwpx`) still reads unmodified, and the same
//! fixture refuses once its manifest is rewritten to look encrypted. The third
//! test reads the one genuine encrypted HWPX in the ground-truth corpus and is
//! gated on `HWP_CORPUS_DIR`, which is never set in continuous integration (the
//! corpus lives outside the repository and is never committed). Run it locally with:
//!
//! ```text
//! HWP_CORPUS_DIR=~/Documents/hwp_samples cargo test -p hwpx --test encrypted_gate
//! ```
//!
//! The corpus-gated half cannot run in CI; the committed-fixture half always does.

use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use aes::Aes256;
use aes::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::NoPadding};
use base64::Engine as _;
use flate2::{Compression, write::DeflateEncoder};
use hwpx::{HwpxError, ReadOptions, read_document, read_document_with_options};
use sha2::Digest as _;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn fixture() -> PathBuf {
    let p = repo().join("fixtures/samples/report-tables.hwpx");
    assert!(p.exists(), "커밋된 픽스처 없음: {}", p.display());
    p
}

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

fn temp_file(label: &str) -> PathBuf {
    let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "hwpx_encrypted_gate_{label}_{}_{}.hwpx",
        std::process::id(),
        id
    ))
}

const ENCRYPTED_MANIFEST: &[u8] = br##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><odf:file-entry full-path="Contents/header.xml" media-type="application/xml" size="1"><odf:encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="AAAA"><odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="AAAA"/><odf:key-derivation key-derivation-name="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2" key-size="32" iteration-count="1024" salt="AAAA"/><odf:start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256" key-size="32"/></odf:encryption-data></odf:file-entry></odf:manifest>"##;

/// Copies `source` into a fresh archive with `META-INF/manifest.xml` replaced by
/// `new_manifest`, keeping every other entry's content unchanged. The mimetype entry
/// is written first and stored uncompressed, matching the OPC convention the package
/// opener's mimetype check depends on.
fn repack_with_manifest(source: &Path, new_manifest: &[u8]) -> PathBuf {
    let file = std::fs::File::open(source).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();

    let output = temp_file("repacked");
    let out_file = std::fs::File::create(&output).unwrap();
    let mut writer = zip::ZipWriter::new(out_file);

    // mimetype first, stored uncompressed (OPC convention the opener checks).
    let mimetype_options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("mimetype", mimetype_options).unwrap();
    writer
        .write_all(hwpx::package::MIMETYPE.as_bytes())
        .unwrap();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        if name == "mimetype" {
            continue; // already written above
        }
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
        if name == "META-INF/manifest.xml" {
            data = new_manifest.to_vec();
        }
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(&name, options).unwrap();
        writer.write_all(&data).unwrap();
    }
    writer.finish().unwrap();
    output
}

#[test]
fn the_committed_sample_still_reads() {
    let result = read_document(&fixture());
    let result = result.expect("the unmodified committed sample must still read");
    assert!(
        !result.document.sections.is_empty(),
        "expected at least one section"
    );
}

#[test]
fn the_committed_sample_with_an_encrypted_manifest_refuses() {
    let repacked = repack_with_manifest(&fixture(), ENCRYPTED_MANIFEST);
    let result = read_document(&repacked);
    let _ = std::fs::remove_file(&repacked);
    match result {
        Ok(_) => panic!("expected a document with a rewritten manifest to be refused"),
        Err(error) => assert!(
            matches!(error, HwpxError::Encrypted),
            "expected the encryption variant, got: {error}"
        ),
    }
}

#[test]
fn correct_password_opens_an_evidenced_profile() {
    let password = "synthetic-password";
    let repacked = repack_with_evidenced_password(&fixture(), password);
    let result = read_document_with_options(
        &repacked,
        &ReadOptions {
            password: Some(password),
        },
    );
    let _ = std::fs::remove_file(&repacked);
    assert!(
        result.is_ok(),
        "evidenced HWPX profile must be password-aware"
    );
}

fn repack_with_evidenced_password(source: &Path, password: &str) -> PathBuf {
    const SALT: [u8; 16] = [7; 16];
    const IV: [u8; 16] = [9; 16];
    let mut archive = zip::ZipArchive::new(std::fs::File::open(source).unwrap()).unwrap();
    let output = temp_file("evidenced");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&output).unwrap());
    let mut manifest_entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        if name == "mimetype" || name == "META-INF/manifest.xml" {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if name == "Contents/header.xml" || name == "Contents/section0.xml" {
            let (ciphertext, manifest_entry) =
                encrypt_evidenced_entry(&name, &data, password, &SALT, &IV);
            manifest_entries.push(manifest_entry);
            writer
                .start_file(
                    &name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(&ciphertext).unwrap();
        } else {
            writer
                .start_file(
                    &name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(&data).unwrap();
        }
    }
    writer
        .start_file(
            "mimetype",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer
        .write_all(hwpx::package::MIMETYPE.as_bytes())
        .unwrap();
    let manifest = format!(
        r#"<?xml version="1.0"?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">{}</odf:manifest>"#,
        manifest_entries.join("")
    );
    writer
        .start_file(
            "META-INF/manifest.xml",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(manifest.as_bytes()).unwrap();
    writer.finish().unwrap();
    output
}

fn encrypt_evidenced_entry(
    name: &str,
    plaintext: &[u8],
    password: &str,
    salt: &[u8],
    iv: &[u8],
) -> (Vec<u8>, String) {
    let mut compressed = Vec::new();
    let mut deflater = DeflateEncoder::new(&mut compressed, Compression::default());
    deflater.write_all(plaintext).unwrap();
    deflater.finish().unwrap();
    compressed.extend(std::iter::repeat_n(0, (16 - compressed.len() % 16) % 16));
    let mut start_key = sha2::Sha256::digest(password.as_bytes()).to_vec();
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&start_key, salt, 1024, &mut key);
    start_key.fill(0);
    let length = compressed.len();
    let ciphertext = cbc::Encryptor::<Aes256>::new_from_slices(&key, iv)
        .unwrap()
        .encrypt_padded::<NoPadding>(&mut compressed, length)
        .unwrap()
        .to_vec();
    key.fill(0);
    let checksum = sha2::Sha256::digest(&plaintext[..plaintext.len().min(1024)]);
    let entry = format!(
        r#"<odf:file-entry full-path="{name}" size="{}"><odf:encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="{}"><odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="{}"/><odf:key-derivation key-derivation-name="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2" key-size="32" iteration-count="1024" salt="{}"/><odf:start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256"/></odf:encryption-data></odf:file-entry>"#,
        plaintext.len(),
        base64::engine::general_purpose::STANDARD.encode(checksum),
        base64::engine::general_purpose::STANDARD.encode(iv),
        base64::engine::general_purpose::STANDARD.encode(salt),
    );
    (ciphertext, entry)
}

fn corpus_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("HWP_CORPUS_DIR").map(PathBuf::from)?;
    dir.is_dir().then_some(dir)
}

#[test]
fn the_genuine_encrypted_package_refuses_with_the_typed_message() {
    let Some(dir) = corpus_dir() else {
        eprintln!("skip: HWP_CORPUS_DIR unset — only the genuine corpus proves this branch");
        return;
    };
    let path = dir.join("enc-01-hwpx-odf-aes256-pw123456.hwpx");
    assert!(
        path.exists(),
        "corpus present but missing expected file: {}",
        path.display()
    );

    let result = read_document(&path);
    let error = match result {
        Ok(_) => panic!("expected the genuine encrypted HWPX to be refused"),
        Err(e) => e,
    };
    assert!(
        matches!(error, HwpxError::Encrypted),
        "expected the encryption variant, got {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("암호화된 문서는 지원하지 않습니다"),
        "message: {message}"
    );
    // This is the exact downstream parse error GATE-02 exists to eliminate — assert
    // its absence, not just the presence of the right variant.
    assert!(
        !message.contains("XML 파싱 오류"),
        "must not fall through to the XML parse error: {message}"
    );
    assert!(
        !message.contains("header.xml"),
        "must not name the header content part: {message}"
    );
}
