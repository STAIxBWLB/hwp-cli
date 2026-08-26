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
            let previous = if index == 0 { 0xec } else { password[index - 1] };
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
        assert!(validate_live_bytes(
            HWP5_PASSWORD_MAX_STREAM_BYTES,
            HWP5_PASSWORD_MAX_STREAM_BYTES,
        )
        .is_ok());
        assert!(validate_live_bytes(HWP5_PASSWORD_MAX_STREAM_BYTES + 1, 0).is_err());
        assert!(validate_live_bytes(u64::MAX, 1).is_err());
    }
}
