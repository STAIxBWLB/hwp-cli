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
const HWPX_START_KEY: &str = "http://www.w3.org/2000/09/xmldsig#sha256";
const HWPX_CHECKSUM_SUFFIX: &str = "#sha256-1k";
const HWPX_MAX_PBKDF2_ITERATIONS: u32 = 1_000_000;

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
            || !entry
                .checksum_id
                .as_deref()
                .is_some_and(|value| value.ends_with(HWPX_CHECKSUM_SUFFIX))
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
#[derive(Debug, PartialEq, Eq)]
pub struct Hwp5ProbeObservation {
    pub encrypt_version: u32,
    pub cfb_stream_count: usize,
    pub cfb_stream_bytes: u64,
    pub validated_record_stream_count: usize,
}

pub fn probe_hwp5_encrypt_version_4(
    path: &Path,
    password: &str,
) -> Result<Hwp5ProbeObservation, String> {
    let mut container = hwp5::Hwp5Container::open(path)
        .map_err(|_| "HWP5 container cannot be opened for profile discovery".to_owned())?;
    let header = container.file_header().clone();
    if !header.is_encrypted() || header.encrypt_version != 4 {
        return Err("HWP5 EncryptVersion is unsupported or not password-protected".to_owned());
    }
    let streams = container.list_streams();
    let cfb_stream_bytes = streams.iter().try_fold(0u64, |total, stream| {
        total
            .checked_add(stream.size)
            .ok_or_else(|| "HWP5 CFB stream accounting overflowed".to_owned())
    })?;
    let mut validated_record_stream_count = 0usize;
    for stream in streams.iter().filter(|stream| {
        stream.path == "/DocInfo"
            || stream.path.starts_with("/BodyText/")
            || stream.path.starts_with("/ViewText/")
            || stream.path.starts_with("/Scripts/")
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
        validated_record_stream_count += 1;
    }
    if validated_record_stream_count == 0 {
        return Err("HWP5 stream behavior is ambiguous".to_owned());
    }
    Ok(Hwp5ProbeObservation {
        encrypt_version: header.encrypt_version,
        cfb_stream_count: streams.len(),
        cfb_stream_bytes,
        validated_record_stream_count,
    })
}
