//! Evidence-bound ODF encryption support for password-protected HWPX.
//!
//! This module deliberately accepts one observed profile only. It has no
//! compatibility fallbacks: changing identifiers, padding, checksum scope, or
//! password bytes changes the outcome to a closed refusal.

use std::collections::BTreeMap;

use aes::Aes256;
use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::NoPadding};
use base64::Engine as _;
use flate2::{Decompress, FlushDecompress, Status};
use pbkdf2::pbkdf2_hmac;
use quick_xml::events::Event;
use sha1::Sha1;
use sha2::{Digest as _, Sha256};
use zeroize::{Zeroize as _, Zeroizing};

use crate::error::{HwpxError, Result};
use crate::package::PackageLimits;

const ALGORITHM_ID: &str = "http://www.w3.org/2001/04/xmlenc#aes256-cbc";
const KDF_ID: &str = "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2";
const START_KEY_ID: &str = "http://www.w3.org/2000/09/xmldsig#sha256";
const CHECKSUM_ID: &str = "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k";
const MAX_PBKDF2_ITERATIONS: u32 = 1_000_000;
const MAX_TOTAL_PBKDF2_ITERATIONS: u64 = 8_000_000;

/// The exact observed HWPX encryption profile and its protected entries.
#[derive(Debug)]
pub struct EncryptionProfile {
    entries: Vec<ProtectedEntry>,
}

#[derive(Debug)]
pub(crate) struct ProtectedEntry {
    pub(crate) name: String,
    declared_plaintext_bytes: u64,
    iv: Vec<u8>,
    salt: Vec<u8>,
    iterations: u32,
    checksum: Vec<u8>,
}

#[derive(Default)]
struct CurrentEntry {
    name: Option<String>,
    size: Option<u64>,
    encryption: Option<(String, Vec<u8>)>,
    kdf: Option<(String, Vec<u8>, u32, usize)>,
    start_key: Option<String>,
    checksum: Option<(String, Vec<u8>)>,
    in_encryption_data: bool,
}

/// Parses only the profile observed in the owner evidence. Missing, repeated,
/// malformed, and unobserved metadata is a capability refusal before crypto.
pub fn parse_profile(manifest: &str) -> Result<EncryptionProfile> {
    let mut reader = quick_xml::Reader::from_str(manifest);
    let mut current: Option<CurrentEntry> = None;
    let mut entries = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => match local_name(event.name().as_ref()) {
                b"file-entry" => {
                    if current.is_some() {
                        return Err(HwpxError::UnsupportedEncryptionProfile);
                    }
                    current = Some(CurrentEntry {
                        name: attr(&event, b"full-path"),
                        size: attr(&event, b"size").and_then(|value| value.parse().ok()),
                        ..CurrentEntry::default()
                    });
                }
                name => apply_metadata(current.as_mut(), name, &event)?,
            },
            Ok(Event::Empty(event)) => match local_name(event.name().as_ref()) {
                b"file-entry" => {
                    if current.is_some() {
                        return Err(HwpxError::UnsupportedEncryptionProfile);
                    }
                }
                name => {
                    apply_metadata(current.as_mut(), name, &event)?;
                    if name == b"encryption-data" {
                        current
                            .as_mut()
                            .ok_or(HwpxError::UnsupportedEncryptionProfile)?
                            .in_encryption_data = false;
                    }
                }
            },
            Ok(Event::End(event)) if local_name(event.name().as_ref()) == b"encryption-data" => {
                current
                    .as_mut()
                    .ok_or(HwpxError::UnsupportedEncryptionProfile)?
                    .in_encryption_data = false;
            }
            Ok(Event::End(event)) if local_name(event.name().as_ref()) == b"file-entry" => {
                let Some(entry) = current.take() else {
                    return Err(HwpxError::UnsupportedEncryptionProfile);
                };
                if let Some(protected) = complete_entry(entry)? {
                    entries.push(protected);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(HwpxError::UnsupportedEncryptionProfile),
        }
    }
    if current.is_some() || entries.is_empty() {
        return Err(HwpxError::UnsupportedEncryptionProfile);
    }
    let mut names = BTreeMap::new();
    let mut total_pbkdf2_iterations = 0u64;
    for entry in &entries {
        if names.insert(&entry.name, ()).is_some() {
            return Err(HwpxError::UnsupportedEncryptionProfile);
        }
        total_pbkdf2_iterations = total_pbkdf2_iterations
            .checked_add(u64::from(entry.iterations))
            .filter(|total| *total <= MAX_TOTAL_PBKDF2_ITERATIONS)
            .ok_or(HwpxError::UnsupportedEncryptionProfile)?;
    }
    Ok(EncryptionProfile { entries })
}

fn apply_metadata(
    current: Option<&mut CurrentEntry>,
    name: &[u8],
    event: &quick_xml::events::BytesStart<'_>,
) -> Result<()> {
    let Some(entry) = current else {
        return Ok(());
    };
    match name {
        b"encryption-data" => {
            if entry.in_encryption_data || entry.checksum.is_some() {
                return Err(HwpxError::UnsupportedEncryptionProfile);
            }
            entry.in_encryption_data = true;
            entry.checksum = Some((
                required_attr(event, b"checksum-type")?,
                decode_attr(event, b"checksum")?,
            ));
        }
        b"algorithm" => {
            if !entry.in_encryption_data || entry.encryption.is_some() {
                return Err(HwpxError::UnsupportedEncryptionProfile);
            }
            entry.encryption = Some((
                required_attr(event, b"algorithm-name")?,
                decode_attr(event, b"initialisation-vector")?,
            ));
        }
        b"key-derivation" => {
            if !entry.in_encryption_data || entry.kdf.is_some() {
                return Err(HwpxError::UnsupportedEncryptionProfile);
            }
            entry.kdf = Some((
                required_attr(event, b"key-derivation-name")?,
                decode_attr(event, b"salt")?,
                required_attr(event, b"iteration-count")?
                    .parse()
                    .map_err(|_| HwpxError::UnsupportedEncryptionProfile)?,
                required_attr(event, b"key-size")?
                    .parse()
                    .map_err(|_| HwpxError::UnsupportedEncryptionProfile)?,
            ));
        }
        b"start-key-generation" => {
            if !entry.in_encryption_data || entry.start_key.is_some() {
                return Err(HwpxError::UnsupportedEncryptionProfile);
            }
            entry.start_key = Some(required_attr(event, b"start-key-generation-name")?);
        }
        _ => {}
    }
    Ok(())
}

fn complete_entry(entry: CurrentEntry) -> Result<Option<ProtectedEntry>> {
    if entry.in_encryption_data {
        return Err(HwpxError::UnsupportedEncryptionProfile);
    }
    let protected = entry.checksum.is_some()
        || entry.encryption.is_some()
        || entry.kdf.is_some()
        || entry.start_key.is_some();
    if !protected {
        return Ok(None);
    }
    let (algorithm, iv) = entry
        .encryption
        .ok_or(HwpxError::UnsupportedEncryptionProfile)?;
    let (kdf, salt, iterations, key_size) =
        entry.kdf.ok_or(HwpxError::UnsupportedEncryptionProfile)?;
    let (checksum_id, checksum) = entry
        .checksum
        .ok_or(HwpxError::UnsupportedEncryptionProfile)?;
    let name = entry.name.ok_or(HwpxError::UnsupportedEncryptionProfile)?;
    let declared_plaintext_bytes = entry.size.ok_or(HwpxError::UnsupportedEncryptionProfile)?;
    if algorithm != ALGORITHM_ID
        || kdf != KDF_ID
        || entry.start_key.as_deref() != Some(START_KEY_ID)
        || checksum_id != CHECKSUM_ID
        || iv.len() != 16
        || salt.is_empty()
        || salt.len() > 1024
        || iterations == 0
        || iterations > MAX_PBKDF2_ITERATIONS
        || key_size != 32
        || checksum.len() != 32
    {
        return Err(HwpxError::UnsupportedEncryptionProfile);
    }
    Ok(Some(ProtectedEntry {
        name,
        declared_plaintext_bytes,
        iv,
        salt,
        iterations,
        checksum,
    }))
}

/// Decrypts one observed-profile entry and returns plaintext only after its
/// Hancom-observed inflated-plaintext checksum validates.
pub(crate) fn decrypt_entry(
    entry: &ProtectedEntry,
    password: &str,
    ciphertext: Vec<u8>,
    limits: &PackageLimits,
    retained_plaintext: u64,
) -> Result<Zeroizing<Vec<u8>>> {
    let ciphertext_len = u64::try_from(ciphertext.len()).map_err(|_| HwpxError::Encrypted)?;
    let entry_limit = limits.entry_uncompressed_limit(&entry.name);
    if ciphertext_len == 0
        || ciphertext_len % 16 != 0
        || ciphertext_len > limits.max_entry_uncompressed_bytes
        || entry.declared_plaintext_bytes > entry_limit
        || entry.declared_plaintext_bytes
            > ciphertext_len.saturating_mul(limits.max_compression_ratio)
    {
        return Err(HwpxError::Encrypted);
    }
    let transient = ciphertext_len
        .checked_add(ciphertext_len)
        .and_then(|value| value.checked_add(entry.declared_plaintext_bytes))
        .and_then(|value| value.checked_add(retained_plaintext))
        .ok_or(HwpxError::Encrypted)?;
    if transient > limits.max_total_uncompressed_bytes {
        return Err(HwpxError::Encrypted);
    }

    let mut start_key = Zeroizing::new(Sha256::digest(password.as_bytes()).to_vec());
    let mut key = Zeroizing::new([0u8; 32]);
    pbkdf2_hmac::<Sha1>(&start_key, &entry.salt, entry.iterations, &mut *key);
    start_key.zeroize();
    let mut ciphertext = Zeroizing::new(ciphertext);
    let decryptor = cbc::Decryptor::<Aes256>::new_from_slices(&*key, &entry.iv)
        .map_err(|_| HwpxError::Encrypted)?;
    let compressed = decryptor
        .decrypt_padded::<NoPadding>(&mut ciphertext)
        .map_err(|_| HwpxError::Encrypted)?;
    key.zeroize();
    let (plaintext, compressed_end) =
        inflate_raw_bounded(compressed, entry.declared_plaintext_bytes, entry_limit)?;
    let suffix = &compressed[compressed_end..];
    if suffix.len() > 15 || suffix.iter().any(|byte| *byte != 0) {
        return Err(HwpxError::Encrypted);
    }
    let checksum = Sha256::digest(&plaintext[..plaintext.len().min(1024)]);
    if checksum.as_slice() != entry.checksum.as_slice() {
        return Err(HwpxError::Encrypted);
    }
    Ok(plaintext)
}

impl EncryptionProfile {
    pub(crate) fn entries(&self) -> &[ProtectedEntry] {
        &self.entries
    }
}

fn inflate_raw_bounded(
    compressed: &[u8],
    expected_size: u64,
    limit: u64,
) -> Result<(Zeroizing<Vec<u8>>, usize)> {
    let mut inflater = Decompress::new(false);
    let mut plaintext = Zeroizing::new(Vec::new());
    let mut output = [0u8; 8 * 1024];
    loop {
        let input_offset =
            usize::try_from(inflater.total_in()).map_err(|_| HwpxError::Encrypted)?;
        if input_offset > compressed.len() {
            return Err(HwpxError::Encrypted);
        }
        let before = inflater.total_out();
        let status = inflater
            .decompress(
                &compressed[input_offset..],
                &mut output,
                FlushDecompress::None,
            )
            .map_err(|_| HwpxError::Encrypted)?;
        let produced = inflater
            .total_out()
            .checked_sub(before)
            .ok_or(HwpxError::Encrypted)?;
        let next = (plaintext.len() as u64)
            .checked_add(produced)
            .ok_or(HwpxError::Encrypted)?;
        if next > expected_size || next > limit {
            return Err(HwpxError::Encrypted);
        }
        let produced = usize::try_from(produced).map_err(|_| HwpxError::Encrypted)?;
        plaintext
            .try_reserve(produced)
            .map_err(|_| HwpxError::Encrypted)?;
        plaintext.extend_from_slice(&output[..produced]);
        if status == Status::StreamEnd {
            if plaintext.len() as u64 != expected_size {
                return Err(HwpxError::Encrypted);
            }
            let end = usize::try_from(inflater.total_in()).map_err(|_| HwpxError::Encrypted)?;
            return Ok((plaintext, end));
        }
        if status != Status::Ok || (produced == 0 && input_offset == inflater.total_in() as usize) {
            return Err(HwpxError::Encrypted);
        }
    }
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

fn required_attr(event: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Result<String> {
    attr(event, key).ok_or(HwpxError::UnsupportedEncryptionProfile)
}

fn decode_attr(event: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(required_attr(event, key)?)
        .map_err(|_| HwpxError::UnsupportedEncryptionProfile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::NATIVE_PACKAGE_LIMITS;

    #[test]
    fn rejects_profile_metadata_outside_encryption_data() {
        let manifest = r#"<manifest><file-entry full-path="Contents/header.xml" size="16"><algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="AAAAAAAAAAAAAAAAAAAAAA=="/><key-derivation key-derivation-name="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2" key-size="32" iteration-count="1024" salt="AAAAAAAAAAAAAAAAAAAAAA=="/><start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256"/><encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="/></file-entry></manifest>"#;
        assert!(matches!(
            parse_profile(manifest),
            Err(HwpxError::UnsupportedEncryptionProfile)
        ));
    }

    #[test]
    fn rejects_aggregate_pbkdf2_work_before_decryption() {
        let entry = |index: usize| {
            format!(
                r#"<file-entry full-path="Contents/entry{index}.xml" size="16"><encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="><algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="AAAAAAAAAAAAAAAAAAAAAA=="/><key-derivation key-derivation-name="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2" key-size="32" iteration-count="1000000" salt="AAAAAAAAAAAAAAAAAAAAAA=="/><start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256"/></encryption-data></file-entry>"#
            )
        };
        let manifest = |count: usize| {
            format!(
                "<manifest>{}</manifest>",
                (0..count).map(entry).collect::<String>()
            )
        };

        assert!(parse_profile(&manifest(8)).is_ok());
        assert!(matches!(
            parse_profile(&manifest(9)),
            Err(HwpxError::UnsupportedEncryptionProfile)
        ));
    }

    #[test]
    fn rejects_entry_and_live_budget_edges_before_crypto_allocation() {
        let entry = |name: &str, declared_plaintext_bytes| ProtectedEntry {
            name: name.to_string(),
            declared_plaintext_bytes,
            iv: vec![0; 16],
            salt: vec![0; 16],
            iterations: 1,
            checksum: vec![0; 32],
        };
        for (entry, retained_plaintext) in [
            (
                entry(
                    "Contents/header.xml",
                    NATIVE_PACKAGE_LIMITS.max_xml_uncompressed_bytes + 1,
                ),
                0,
            ),
            (
                entry(
                    "BinData/item.bin",
                    16 * (NATIVE_PACKAGE_LIMITS.max_compression_ratio + 1),
                ),
                0,
            ),
            (entry("BinData/item.bin", 1), u64::MAX),
        ] {
            assert!(matches!(
                decrypt_entry(
                    &entry,
                    "irrelevant",
                    vec![0; 16],
                    &NATIVE_PACKAGE_LIMITS,
                    retained_plaintext,
                ),
                Err(HwpxError::Encrypted)
            ));
        }
    }
}
