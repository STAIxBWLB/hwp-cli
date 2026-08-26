// Evidence-bounded HWP5 password stream handling. Only the owner-observed
// EncryptVersion 4 transform is implemented. The password is consumed as
// borrowed, exact UTF-8 bytes; derived material is held in zeroizing buffers
// for the duration of one stream transform.

use aes::Aes128;
use aes::cipher::{Block, BlockCipherEncrypt, KeyInit};
use sha1::Digest as _;
use zeroize::{Zeroize as _, Zeroizing};

use crate::error::{Hwp5Error, Result};

pub(crate) const HWP5_PASSWORD_MAX_STREAM_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const HWP5_PASSWORD_MAX_TRANSFORM_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const HWP5_PASSWORD_MAX_STREAMS: usize = 4_096;
pub(crate) const HWP5_PASSWORD_MAX_TOTAL_STREAM_NAME_BYTES: u64 = 16 * 1024 * 1024;

/// Checks an incoming stream and all other buffers that would be live beside
/// it before either allocation or parser handoff. The caller owns the actual
/// reservations; this small pure helper keeps the overflow boundary testable.
pub(crate) fn validate_live_bytes(stream_bytes: u64, other_live_bytes: u64) -> Result<()> {
    if stream_bytes > HWP5_PASSWORD_MAX_STREAM_BYTES {
        return Err(Hwp5Error::ResourceLimitExceeded {
            resource: "password-protected HWP5 stream".to_string(),
            limit: HWP5_PASSWORD_MAX_STREAM_BYTES,
        });
    }
    let live = stream_bytes.checked_add(other_live_bytes).ok_or_else(|| {
        Hwp5Error::ResourceLimitExceeded {
            resource: "password-protected HWP5 live buffers".to_string(),
            limit: HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES,
        }
    })?;
    if live > HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES {
        return Err(Hwp5Error::ResourceLimitExceeded {
            resource: "password-protected HWP5 live buffers".to_string(),
            limit: HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES,
        });
    }
    Ok(())
}

/// Bounds aggregate CFB1 work before the bit-level password transform starts.
///
/// EncryptVersion 4 performs one AES block encryption per input bit, so the
/// memory-oriented stream cap is far too large to serve as a CPU budget. The
/// owner-observed record streams total 1,296 bytes; 2 MiB leaves more than
/// three orders of magnitude of headroom while bounding work to 16,777,216
/// AES block operations per document.
pub(crate) fn validate_transform_bytes(total_ciphertext_bytes: u64) -> Result<()> {
    if total_ciphertext_bytes > HWP5_PASSWORD_MAX_TRANSFORM_BYTES {
        return Err(Hwp5Error::ResourceLimitExceeded {
            resource: "password-protected HWP5 transform bytes".to_string(),
            limit: HWP5_PASSWORD_MAX_TRANSFORM_BYTES,
        });
    }
    Ok(())
}

/// Applies the only evidence-backed HWP5 password transform in place.
///
/// The feedback register consumes ciphertext bits, so this function is the
/// decryption direction. It consumes the password as the exact bytes it is
/// given: no normalization, case folding, or trimming happens here. Choosing
/// which byte encoding those are is `password_byte_candidates`' job.
/// The password byte encodings Hangul is observed to derive keys from.
///
/// Hangul encodes a password in the **legacy Korean code page (CP949)**, not
/// UTF-8, before deriving a key. ASCII passwords are byte-identical in both
/// encodings, which is why only a non-ASCII password diverges; a genuine
/// document saved with the Korean password `비밀번호1` authenticates under
/// CP949 and under no other encoding tried (UTF-8, UTF-16LE/BE, UTF-8+NUL).
///
/// The list is **closed and ordered**: UTF-8 first, then CP949, and CP949 only
/// when the password is not already ASCII and encodes without replacement.
/// This is not a guess-and-hope fallback: every candidate must still clear the
/// same authentication boundary that a single candidate had to clear, so a
/// wrong password cannot be admitted by adding an encoding.
pub(crate) fn password_byte_candidates(password: &str) -> Vec<Zeroizing<Vec<u8>>> {
    let mut candidates = vec![Zeroizing::new(password.as_bytes().to_vec())];
    if !password.is_ascii() {
        let (encoded, _, had_errors) = encoding_rs::EUC_KR.encode(password);
        if !had_errors {
            candidates.push(Zeroizing::new(encoded.into_owned()));
        }
    }
    candidates
}

pub(crate) fn decrypt_hwp5_encrypt_version_4_in_place(
    bytes: &mut Zeroizing<Vec<u8>>,
    password: &[u8],
) -> Result<()> {
    let mut password_bytes = Zeroizing::new(password.to_vec());
    let source_length = password_bytes
        .len()
        .checked_mul(2)
        .ok_or(Hwp5Error::Encrypted)?;
    let mut source = Zeroizing::new(Vec::new());
    source
        .try_reserve_exact(source_length)
        .map_err(|_| Hwp5Error::Encrypted)?;
    for index in 0..password_bytes.len() {
        let previous = if index == 0 {
            0xec
        } else {
            password_bytes[index - 1]
        };
        source.push(previous.rotate_left(1));
        source.push(password_bytes[index]);
    }
    password_bytes.zeroize();

    let mut digest = sha1::Sha1::digest(&*source);
    let mut key = Zeroizing::new([0u8; 16]);
    key.copy_from_slice(&digest[..16]);
    digest.zeroize();
    source.zeroize();

    let cipher = Aes128::new_from_slice(&*key).map_err(|_| Hwp5Error::Encrypted)?;
    let mut register = Zeroizing::new([0u8; 16]);
    for block in bytes.chunks_mut(16) {
        let mut ciphertext = [0u8; 16];
        ciphertext[..block.len()].copy_from_slice(block);
        let mut plaintext = [0u8; 16];
        for bit_index in 0..128 {
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;
            let mut keystream = Block::<Aes128>::from(*register);
            cipher.encrypt_block(&mut keystream);
            let input_bit = (ciphertext[byte_index] >> (7 - bit_offset)) & 1;
            let result_bit = input_bit ^ (keystream[0] >> 7);
            for index in 0..15 {
                register[index] = (register[index] << 1) | (register[index + 1] >> 7);
            }
            register[15] = (register[15] << 1) | input_bit;
            plaintext[byte_index] |= result_bit << (7 - bit_offset);
        }
        block.copy_from_slice(&plaintext[..block.len()]);
        ciphertext.zeroize();
        plaintext.zeroize();
    }
    register.zeroize();
    key.zeroize();
    Ok(())
}

#[cfg(test)]
mod tests {
    use aes::Aes128;
    use aes::cipher::{Block, BlockCipherEncrypt, KeyInit};
    use sha1::Digest as _;
    use zeroize::Zeroizing;

    use super::{
        HWP5_PASSWORD_MAX_STREAM_BYTES, HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES,
        HWP5_PASSWORD_MAX_TRANSFORM_BYTES, decrypt_hwp5_encrypt_version_4_in_place,
        password_byte_candidates, validate_live_bytes, validate_transform_bytes,
    };

    fn encrypt_for_test(plaintext: &[u8], password: &str) -> Vec<u8> {
        let password = password.as_bytes();
        let mut source = Vec::with_capacity(password.len() * 2);
        for (index, byte) in password.iter().copied().enumerate() {
            let previous = if index == 0 {
                0xec
            } else {
                password[index - 1]
            };
            source.push(previous.rotate_left(1));
            source.push(byte);
        }
        let digest = sha1::Sha1::digest(&source);
        let cipher = Aes128::new_from_slice(&digest[..16]).expect("fixed AES key length");
        let mut register = [0u8; 16];
        let mut encrypted = plaintext.to_vec();

        for block in encrypted.chunks_mut(16) {
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
                register[15] = (register[15] << 1) | result_bit;
                transformed[byte_index] |= result_bit << (7 - bit_offset);
            }
            block.copy_from_slice(&transformed[..block.len()]);
        }
        encrypted
    }

    #[test]
    fn password_transform_uses_the_exact_bytes_it_is_given() {
        // The transform itself normalizes nothing. Picking which bytes a
        // password becomes is `password_byte_candidates`' job, so an NFD
        // spelling of the same syllable is simply a different key here.
        let plaintext = b"observed HWP5 record bytes";
        let password_nfc = "\u{ac00}";
        let password_nfd = "\u{1100}\u{1161}";
        let ciphertext = encrypt_for_test(plaintext, password_nfc);

        let mut correct = Zeroizing::new(ciphertext.clone());
        decrypt_hwp5_encrypt_version_4_in_place(&mut correct, password_nfc.as_bytes()).unwrap();
        assert_eq!(&*correct, plaintext);

        let mut decomposed = Zeroizing::new(ciphertext.clone());
        decrypt_hwp5_encrypt_version_4_in_place(&mut decomposed, password_nfd.as_bytes()).unwrap();
        assert_ne!(&*decomposed, plaintext);

        let mut wrong = Zeroizing::new(ciphertext);
        decrypt_hwp5_encrypt_version_4_in_place(&mut wrong, b"wrong-password").unwrap();
        assert_ne!(&*wrong, plaintext);
    }

    #[test]
    fn password_candidates_add_cp949_only_for_non_ascii() {
        // An ASCII password is byte-identical in both encodings, so offering a
        // second candidate would be pure waste and would double the work every
        // wrong ASCII password costs.
        let ascii = password_byte_candidates("pw123456");
        assert_eq!(ascii.len(), 1);
        assert_eq!(&*ascii[0], b"pw123456");

        // Hangul derives the key from CP949 bytes. UTF-8 stays first because it
        // is what the format specifications say; CP949 is the observed reality.
        let korean = password_byte_candidates("\u{bE44}\u{BC00}\u{BC88}\u{D638}1");
        assert_eq!(korean.len(), 2);
        assert_eq!(&*korean[0], "\u{bE44}\u{BC00}\u{BC88}\u{D638}1".as_bytes());
        assert_eq!(korean[0].len(), 13);
        assert_eq!(korean[1].len(), 9);
        assert_ne!(&*korean[0], &*korean[1]);

        // A password CP949 cannot represent contributes no candidate rather
        // than a replacement-mangled one that could never authenticate.
        let unmappable = password_byte_candidates("\u{1F600}");
        assert_eq!(unmappable.len(), 1);
    }

    #[test]
    fn live_buffer_budget_accepts_limits_and_rejects_overflow_before_allocation() {
        assert_eq!(HWP5_PASSWORD_MAX_STREAM_BYTES, 64 * 1024 * 1024);
        assert_eq!(HWP5_PASSWORD_MAX_TOTAL_LIVE_BYTES, 128 * 1024 * 1024);
        assert!(
            validate_live_bytes(
                HWP5_PASSWORD_MAX_STREAM_BYTES,
                HWP5_PASSWORD_MAX_STREAM_BYTES,
            )
            .is_ok()
        );
        assert!(validate_live_bytes(HWP5_PASSWORD_MAX_STREAM_BYTES + 1, 0).is_err());
        assert!(validate_transform_bytes(HWP5_PASSWORD_MAX_TRANSFORM_BYTES).is_ok());
        assert!(validate_transform_bytes(HWP5_PASSWORD_MAX_TRANSFORM_BYTES + 1).is_err());
        assert!(validate_live_bytes(u64::MAX, 1).is_err());
    }
}
