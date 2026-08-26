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

/// Applies the only evidence-backed HWP5 password transform in place.
///
/// The feedback register consumes ciphertext bits, so this function is the
/// decryption direction. No password normalization, case folding, trimming,
/// or fallback is performed here or by its caller.
pub(crate) fn decrypt_hwp5_encrypt_version_4_in_place(
    bytes: &mut Zeroizing<Vec<u8>>,
    password: &str,
) -> Result<()> {
    let mut password_bytes = Zeroizing::new(password.as_bytes().to_vec());
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
        decrypt_hwp5_encrypt_version_4_in_place, validate_live_bytes,
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
    fn password_transform_uses_exact_utf8_bytes() {
        let plaintext = b"observed HWP5 record bytes";
        let password_nfc = "\u{ac00}";
        let password_nfd = "\u{1100}\u{1161}";
        let ciphertext = encrypt_for_test(plaintext, password_nfc);

        let mut correct = Zeroizing::new(ciphertext.clone());
        decrypt_hwp5_encrypt_version_4_in_place(&mut correct, password_nfc).unwrap();
        assert_eq!(&*correct, plaintext);

        let mut decomposed = Zeroizing::new(ciphertext.clone());
        decrypt_hwp5_encrypt_version_4_in_place(&mut decomposed, password_nfd).unwrap();
        assert_ne!(&*decomposed, plaintext);

        let mut wrong = Zeroizing::new(ciphertext);
        decrypt_hwp5_encrypt_version_4_in_place(&mut wrong, "wrong-password").unwrap();
        assert_ne!(&*wrong, plaintext);
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
        assert!(validate_live_bytes(u64::MAX, 1).is_err());
    }
}
