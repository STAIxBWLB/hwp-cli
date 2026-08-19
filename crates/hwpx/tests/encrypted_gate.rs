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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hwpx::{HwpxError, read_document};

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
