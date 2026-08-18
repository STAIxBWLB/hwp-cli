//! Hancom distribution document (배포용문서) ViewText decryption.
//!
//! A distribution document keeps its body under `/ViewText/SectionN` instead of
//! `/BodyText/SectionN`. The stream's raw (pre-decompression) bytes begin with a
//! 256-byte `DISTRIBUTE_DOC_DATA` record (tag `0x1C`, level 0) that obfuscates an
//! embedded SHA-1 hex digest with an MSVC `rand()`-keyed XOR pass; the first 16
//! bytes of that digest are the AES-128 key for the ciphertext that follows.
//! Everything after the record is AES-128-ECB ciphertext; decrypting it and (if the
//! document is compressed) raw-inflating the result yields an ordinary record
//! stream shaped exactly like `/BodyText/SectionN` — no second parser is needed
//! downstream, `read_document` feeds this module's output into the same
//! `scan_stream`/`parse_section` pair used for a plain document.
//!
//! Algorithm source: reconstructed from pyhwp's `hwp5/distdoc.py`
//! (<https://github.com/mete0r/pyhwp/blob/master/src/hwp5/distdoc.py>, Changwoo
//! Ryu's published reverse-engineering), not the Hancom specification. Verified
//! directly against all 11 genuine distribution documents in the project's
//! ground-truth corpus (`crates/hwp5/tests/distdoc_corpus.rs`).

use aes::Aes128;
use aes::cipher::{Array, BlockCipherDecrypt, KeyInit};

use crate::codec::ByteReader;
use crate::error::{Hwp5Error, Result};
use crate::record::header::RecordHeader;
use crate::record::tag;

/// Linear congruential generator matching the Microsoft C runtime's `rand()`,
/// used by Hancom to key the DISTRIBUTE_DOC_DATA obfuscation XOR. This is
/// Hancom's own record-scrambling scheme, not real cryptography, and is safe to
/// hand-roll — unlike the AES step below, which must not be.
struct MsvcRand(u32);

impl MsvcRand {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(214013).wrapping_add(2531011);
        (self.0 >> 16) & 0x7fff
    }
}

/// Recovers the AES-128 key hidden in a 256-byte DISTRIBUTE_DOC_DATA payload.
///
/// Bytes `0..4` are a little-endian seed. An MSVC-rand-keyed XOR pass over bytes
/// `4..256` recovers an 80-byte UTF-16LE SHA-1 hex string at offset
/// `4 + (seed & 0xf)`; the first 16 bytes of that string are the AES-128 key.
fn decode_head(payload: &[u8; 256]) -> Result<[u8; 16]> {
    let seed = u32::from_le_bytes(payload[0..4].try_into().expect("fixed 4-byte slice"));
    let mut rng = MsvcRand(seed);
    let mut data = *payload;
    let mut key_byte = 0u32;
    let mut run = 0u32;
    for (i, byte) in data.iter_mut().enumerate() {
        if run == 0 {
            key_byte = rng.next() & 0xff;
            run = (rng.next() & 0xf) + 1;
        }
        if i >= 4 {
            *byte ^= key_byte as u8;
        }
        run -= 1;
    }
    let offset = 4 + (seed & 0xf) as usize;
    if offset + 80 > data.len() {
        return Err(Hwp5Error::MalformedRecord(format!(
            "DISTRIBUTE_DOC_DATA SHA-1 영역 오프셋 {offset}이 256바이트 페이로드를 벗어납니다"
        )));
    }
    Ok(data[offset..offset + 80][..16]
        .try_into()
        .expect("80-byte region has at least 16 bytes"))
}

/// Decrypts one `/ViewText/SectionN` stream's raw (pre-decompression) bytes.
///
/// `raw` is exactly what [`crate::container::Hwp5Container::read_stream_raw`]
/// returns for a `/ViewText/SectionN` path — never call `read_record_stream` on
/// this path; its raw-inflate assumption does not hold here, since the stream
/// begins with the unencrypted DISTRIBUTE_DOC_DATA record followed by AES
/// ciphertext, not deflate data. `compressed` is the document's FileHeader
/// compression bit; when set, the decrypted bytes are raw-inflated with
/// [`crate::codec::decompress`] exactly as `/BodyText/` streams already are.
pub fn decrypt_view_text_section(raw: &[u8], compressed: bool) -> Result<Vec<u8>> {
    let mut reader = ByteReader::new(raw);
    let header = RecordHeader::decode(&mut reader)?;
    if header.tag != tag::DISTRIBUTE_DOC_DATA || header.size != 256 {
        return Err(Hwp5Error::MalformedRecord(format!(
            "ViewText 선행 레코드가 DISTRIBUTE_DOC_DATA(태그 0x{:03X}, 256B)가 아닙니다 \
             (관측: 태그 0x{:03X}, 크기 {}B)",
            tag::DISTRIBUTE_DOC_DATA,
            header.tag,
            header.size
        )));
    }
    let payload: [u8; 256] = reader
        .read_bytes(256)?
        .try_into()
        .expect("read_bytes(256) guarantees a 256-byte slice");
    let key = decode_head(&payload)?;

    let tail = reader.take_rest();
    if tail.is_empty() || !tail.len().is_multiple_of(16) {
        return Err(Hwp5Error::MalformedRecord(format!(
            "ViewText AES 암호문 길이가 16의 배수가 아니거나 비어 있습니다 (관측: {}바이트)",
            tail.len()
        )));
    }

    let cipher = Aes128::new(&Array::from(key));
    let mut plain = tail.to_vec();
    for block in plain.chunks_exact_mut(16) {
        let mut b = Array::from(<[u8; 16]>::try_from(&*block).expect("chunks_exact(16)"));
        cipher.decrypt_block(&mut b);
        block.copy_from_slice(&b);
    }

    if compressed {
        crate::codec::decompress(&plain, "ViewText")
    } else {
        Ok(plain)
    }
}

#[cfg(test)]
mod tests {
    use aes::cipher::BlockCipherEncrypt;

    use super::*;
    use crate::codec::ByteWriter;

    /// Builds a synthetic 256-byte DISTRIBUTE_DOC_DATA payload whose forward
    /// XOR pass matches `decode_head`'s decoding pass (XOR is its own
    /// inverse), embedding `hex_region` at the seed-derived offset so a
    /// reader recovers it byte-for-byte.
    fn synthetic_payload(seed: u32, hex_region: &[u8; 80]) -> [u8; 256] {
        let mut data = [0u8; 256];
        data[0..4].copy_from_slice(&seed.to_le_bytes());
        let offset = 4 + (seed & 0xf) as usize;
        data[offset..offset + 80].copy_from_slice(hex_region);
        let mut rng = MsvcRand(seed);
        let mut key_byte = 0u32;
        let mut run = 0u32;
        for (i, byte) in data.iter_mut().enumerate() {
            if run == 0 {
                key_byte = rng.next() & 0xff;
                run = (rng.next() & 0xf) + 1;
            }
            if i >= 4 {
                *byte ^= key_byte as u8;
            }
            run -= 1;
        }
        data
    }

    fn known_hex_region() -> [u8; 80] {
        let mut region = [0u8; 80];
        for (i, b) in region.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        region
    }

    /// Builds a full synthetic `/ViewText/SectionN` raw stream: a record
    /// header (`tag`, `declared_size`), `payload_len` bytes of a
    /// known-good 256-byte payload, and `tail_len` bytes of AES-128-ECB
    /// ciphertext (encrypted under the payload's own derivable key so a
    /// well-formed stream round-trips). One field changed per call is
    /// enough to reach each malformed-input behavior.
    fn synthetic_stream(
        tag: u16,
        declared_size: u32,
        payload_len: usize,
        tail_len: usize,
    ) -> Vec<u8> {
        synthetic_stream_with_fill(tag, declared_size, payload_len, tail_len, 0xAB)
    }

    /// As [`synthetic_stream`], but with the pre-encryption tail plaintext
    /// filled with `fill` instead of a fixed byte — `0xAB` round-trips fine
    /// through raw deflate (DEFLATE tolerates almost any bit pattern), so a
    /// test that specifically needs a *malformed* deflate stream after
    /// decryption needs a byte pattern that actually violates DEFLATE's
    /// block-header constraints (`0xFF` sets a reserved BTYPE).
    fn synthetic_stream_with_fill(
        tag: u16,
        declared_size: u32,
        payload_len: usize,
        tail_len: usize,
        fill: u8,
    ) -> Vec<u8> {
        let seed = 0x1234_5678u32;
        let hex_region = known_hex_region();
        let payload = synthetic_payload(seed, &hex_region);
        let key: [u8; 16] = hex_region[..16].try_into().unwrap();

        let mut w = ByteWriter::new();
        RecordHeader {
            tag,
            level: 0,
            size: declared_size,
        }
        .encode(&mut w);
        let mut bytes = w.into_bytes();
        bytes.extend_from_slice(&payload[..payload_len.min(256)]);

        let mut tail_plain = vec![fill; tail_len];
        let cipher = Aes128::new(&Array::from(key));
        for block in tail_plain.chunks_exact_mut(16) {
            let mut b = Array::from(<[u8; 16]>::try_from(&*block).unwrap());
            cipher.encrypt_block(&mut b);
            block.copy_from_slice(&b);
        }
        bytes.extend_from_slice(&tail_plain);
        bytes
    }

    #[test]
    fn distribution_key_derivation_matches_known_payload() {
        let seed = 0x1234_5678u32;
        let hex_region = known_hex_region();
        let payload = synthetic_payload(seed, &hex_region);
        let expected_key: [u8; 16] = hex_region[..16].try_into().unwrap();

        let key = decode_head(&payload).expect("known-good synthetic payload decodes");
        assert_eq!(key, expected_key);
    }

    #[test]
    fn truncated_distribute_record_is_an_error() {
        // Wrong leading tag: MalformedRecord naming the observed tag.
        let bytes = synthetic_stream(tag::PARA_HEADER, 256, 256, 32);
        match decrypt_view_text_section(&bytes, false) {
            Err(Hwp5Error::MalformedRecord(msg)) => {
                assert!(msg.contains(&format!("{:03X}", tag::PARA_HEADER)), "{msg}");
            }
            other => panic!("expected MalformedRecord naming the observed tag, got {other:?}"),
        }

        // Declared size other than 256: MalformedRecord.
        let bytes = synthetic_stream(tag::DISTRIBUTE_DOC_DATA, 255, 256, 32);
        match decrypt_view_text_section(&bytes, false) {
            Err(Hwp5Error::MalformedRecord(msg)) => assert!(msg.contains("255"), "{msg}"),
            other => panic!("expected MalformedRecord naming the observed size, got {other:?}"),
        }

        // Stream ends inside the 256-byte payload: UnexpectedEof.
        let bytes = synthetic_stream(tag::DISTRIBUTE_DOC_DATA, 256, 100, 0);
        match decrypt_view_text_section(&bytes, false) {
            Err(Hwp5Error::UnexpectedEof { .. }) => {}
            other => panic!("expected UnexpectedEof, got {other:?}"),
        }
    }

    #[test]
    fn ciphertext_length_not_a_multiple_of_the_block_size_is_an_error() {
        let bytes = synthetic_stream(tag::DISTRIBUTE_DOC_DATA, 256, 256, 17);
        match decrypt_view_text_section(&bytes, false) {
            Err(Hwp5Error::MalformedRecord(msg)) => assert!(msg.contains("17"), "{msg}"),
            other => panic!("expected MalformedRecord naming the observed length, got {other:?}"),
        }

        // Empty ciphertext tail: also MalformedRecord, not a panic.
        let bytes = synthetic_stream(tag::DISTRIBUTE_DOC_DATA, 256, 256, 0);
        match decrypt_view_text_section(&bytes, false) {
            Err(Hwp5Error::MalformedRecord(_)) => {}
            other => panic!("expected MalformedRecord, got {other:?}"),
        }
    }

    #[test]
    fn garbage_after_decryption_is_reported_not_swallowed() {
        // Well-formed record + payload + a genuinely AES-decryptable tail, but
        // the plaintext (0xFF repeated, an invalid DEFLATE block header) is
        // not a valid raw-deflate stream — the decompress error must surface,
        // not an empty (or garbage) success.
        let bytes = synthetic_stream_with_fill(tag::DISTRIBUTE_DOC_DATA, 256, 256, 16, 0xFF);
        match decrypt_view_text_section(&bytes, true) {
            Err(Hwp5Error::Decompress { .. }) => {}
            other => panic!("expected Decompress error, got {other:?}"),
        }
    }
}
