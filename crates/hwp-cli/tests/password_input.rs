//! Released-binary password input coverage for `hwp cat`.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aes::cipher::{
    Block, BlockCipherEncrypt, BlockModeEncrypt, KeyInit, KeyIvInit, block_padding::NoPadding,
};
use aes::{Aes128, Aes256};
use base64::Engine as _;
use flate2::{Compression, write::DeflateEncoder};
use sha1::Digest as _;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("hwp-cli-password-{}", std::process::id()))
        .join(label);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn encrypt_hwp5_stream(plaintext: &[u8], password: &str) -> Vec<u8> {
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

fn evidenced_hwp5_fixture(dir: &Path, password: &str) -> PathBuf {
    let path = dir.join("protected.hwp");
    let created = hwp().arg("new").arg("-o").arg(&path).output().unwrap();
    assert!(
        created.status.success(),
        "base fixture creation failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut cfb = cfb::CompoundFile::open(file).unwrap();
    let mut header = Vec::new();
    cfb.open_stream("/FileHeader")
        .unwrap()
        .read_to_end(&mut header)
        .unwrap();
    let attributes = u32::from_le_bytes(header[36..40].try_into().unwrap()) | (1 << 1);
    header[36..40].copy_from_slice(&attributes.to_le_bytes());
    header[44..48].copy_from_slice(&4u32.to_le_bytes());
    let mut header_stream = cfb.open_stream("/FileHeader").unwrap();
    header_stream.set_len(0).unwrap();
    header_stream.seek(SeekFrom::Start(0)).unwrap();
    header_stream.write_all(&header).unwrap();
    drop(header_stream);

    for stream_path in ["/DocInfo", "/BodyText/Section0"] {
        let mut raw = Vec::new();
        cfb.open_stream(stream_path)
            .unwrap()
            .read_to_end(&mut raw)
            .unwrap();
        let encrypted = encrypt_hwp5_stream(&raw, password);
        let mut stream = cfb.open_stream(stream_path).unwrap();
        stream.set_len(0).unwrap();
        stream.seek(SeekFrom::Start(0)).unwrap();
        stream.write_all(&encrypted).unwrap();
    }
    cfb.flush().unwrap();
    path
}

fn refusal(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr).into_owned()
}

fn evidenced_hwpx_fixture(dir: &Path, password: &str) -> PathBuf {
    let plain = dir.join("plain.hwpx");
    let protected = dir.join("protected.hwpx");
    let created = hwp().arg("new").arg("-o").arg(&plain).output().unwrap();
    assert!(created.status.success(), "base HWPX creation failed");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&plain).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&protected).unwrap());
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
            let (ciphertext, manifest_entry) = encrypt_hwpx_part(&name, &data, password);
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
    protected
}

fn encrypt_hwpx_part(name: &str, plaintext: &[u8], password: &str) -> (Vec<u8>, String) {
    const SALT: [u8; 16] = [7; 16];
    const IV: [u8; 16] = [9; 16];
    let mut compressed = Vec::new();
    let mut deflater = DeflateEncoder::new(&mut compressed, Compression::default());
    deflater.write_all(plaintext).unwrap();
    deflater.finish().unwrap();
    compressed.extend(std::iter::repeat_n(0, (16 - compressed.len() % 16) % 16));
    let mut start_key = sha2::Sha256::digest(password.as_bytes()).to_vec();
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(&start_key, &SALT, 1024, &mut key);
    start_key.fill(0);
    let length = compressed.len();
    let ciphertext = cbc::Encryptor::<Aes256>::new_from_slices(&key, &IV)
        .unwrap()
        .encrypt_padded::<NoPadding>(&mut compressed, length)
        .unwrap()
        .to_vec();
    key.fill(0);
    let checksum = sha2::Sha256::digest(&plaintext[..plaintext.len().min(1024)]);
    let manifest_entry = format!(
        r#"<odf:file-entry full-path="{name}" size="{}"><odf:encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="{}"><odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="{}"/><odf:key-derivation key-derivation-name="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2" key-size="32" iteration-count="1024" salt="{}"/><odf:start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256"/></odf:encryption-data></odf:file-entry>"#,
        plaintext.len(),
        base64::engine::general_purpose::STANDARD.encode(checksum),
        base64::engine::general_purpose::STANDARD.encode(IV),
        base64::engine::general_purpose::STANDARD.encode(SALT),
    );
    (ciphertext, manifest_entry)
}

#[test]
fn hwp5_cat_direct_and_stdin_preserve_exact_password_bytes() {
    let password = "  \u{ac00}  ";
    let file = evidenced_hwp5_fixture(&temp_dir("direct-and-stdin"), password);

    let direct = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password")
        .arg(password)
        .output()
        .unwrap();
    assert!(
        direct.status.success(),
        "direct password failed: {}",
        refusal(&direct.stderr)
    );
    let mut stdin = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    stdin
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{password}\r\n").as_bytes())
        .unwrap();
    let stdin = stdin.wait_with_output().unwrap();
    assert!(
        stdin.status.success(),
        "password stdin failed: {}",
        refusal(&stdin.stderr)
    );

    let mut stdin_lf = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    stdin_lf
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{password}\n").as_bytes())
        .unwrap();
    let stdin_lf = stdin_lf.wait_with_output().unwrap();
    assert!(
        stdin_lf.status.success(),
        "LF password stdin failed: {}",
        refusal(&stdin_lf.stderr)
    );

    let mut stdin_without_newline = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    stdin_without_newline
        .stdin
        .take()
        .unwrap()
        .write_all(password.as_bytes())
        .unwrap();
    let stdin_without_newline = stdin_without_newline.wait_with_output().unwrap();
    assert!(
        stdin_without_newline.status.success(),
        "unterminated password stdin failed: {}",
        refusal(&stdin_without_newline.stderr)
    );

    let nfd = "  \u{1100}\u{1161}  ";
    let wrong = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password")
        .arg(nfd)
        .output()
        .unwrap();
    assert!(!wrong.status.success(), "normalization must not occur");
}

#[test]
fn hwp5_cat_refusals_and_conflicts_are_secret_free() {
    let password = "secret-not-for-output";
    let file = evidenced_hwp5_fixture(&temp_dir("refusal-and-conflict"), password);

    let absent = hwp().arg("cat").arg(&file).output().unwrap();
    let wrong = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password")
        .arg("different-secret")
        .output()
        .unwrap();
    assert!(!absent.status.success() && !wrong.status.success());
    assert_eq!(refusal(&absent.stderr), refusal(&wrong.stderr));
    for output in [&absent.stderr, &wrong.stderr] {
        let output = refusal(output);
        assert!(output.contains("HWP_PASSWORD_REQUIRED_OR_INVALID"));
        assert!(output.contains("암호화된 문서는 지원하지 않습니다"));
        assert!(!output.contains(password));
        assert!(!output.contains("different-secret"));
        assert!(!output.contains("EncryptVersion"));
    }

    let conflict = hwp()
        .arg("cat")
        .arg("does-not-exist.hwp")
        .arg("--password")
        .arg(password)
        .arg("--password-stdin")
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    assert!(!refusal(&conflict.stderr).contains(password));

    let stdin_collision = hwp()
        .arg("cat")
        .arg("-")
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .output()
        .unwrap();
    assert!(!stdin_collision.status.success());
    assert!(!refusal(&stdin_collision.stderr).contains("No such file"));

    let root_help = hwp().arg("--help").output().unwrap();
    let cat_help = hwp().args(["cat", "--help"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&root_help.stdout).contains("--password"));
    assert!(String::from_utf8_lossy(&cat_help.stdout).contains("--password-stdin"));
}

#[test]
fn hwpx_cat_uses_exact_password_bytes_and_one_public_refusal() {
    let password = "  \u{ac00}  ";
    let file = evidenced_hwpx_fixture(&temp_dir("hwpx-public-refusal"), password);
    let correct = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password")
        .arg(password)
        .output()
        .unwrap();
    assert!(correct.status.success(), "exact HWPX password must decrypt");
    let absent = hwp().arg("cat").arg(&file).output().unwrap();
    let wrong = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password")
        .arg("  \u{1100}\u{1161}  ")
        .output()
        .unwrap();
    assert!(!absent.status.success() && !wrong.status.success());
    assert_eq!(refusal(&absent.stderr), refusal(&wrong.stderr));
    for output in [&absent.stderr, &wrong.stderr] {
        let output = refusal(output);
        assert!(output.contains("HWP_PASSWORD_REQUIRED_OR_INVALID"));
        assert!(!output.contains(password));
        assert!(!output.contains("Contents/header.xml"));
        assert!(!output.contains("AES256-CBC"));
    }
}

#[test]
fn cli_convert_render_support_password_inputs_before_publication() {
    let password = "convert-render-secret";
    let dir = temp_dir("convert-render");
    std::fs::create_dir_all(dir.join("hwp5")).unwrap();
    std::fs::create_dir_all(dir.join("hwpx")).unwrap();
    for (label, input) in [
        ("hwp5", evidenced_hwp5_fixture(&dir.join("hwp5"), password)),
        ("hwpx", evidenced_hwpx_fixture(&dir.join("hwpx"), password)),
    ] {
        let converted = dir.join(format!("{label}.md"));
        let convert = hwp()
            .arg("convert")
            .arg(&input)
            .arg("-o")
            .arg(&converted)
            .arg("--password")
            .arg(password)
            .output()
            .unwrap();
        assert!(
            convert.status.success(),
            "{label} convert with a password must succeed: {}",
            refusal(&convert.stderr)
        );
        assert!(converted.exists());

        let rendered = dir.join(format!("{label}.svg"));
        let render = hwp()
            .arg("render")
            .arg(&input)
            .arg("-o")
            .arg(&rendered)
            .arg("--password")
            .arg(password)
            .output()
            .unwrap();
        assert!(
            render.status.success(),
            "{label} render with a password must succeed: {}",
            refusal(&render.stderr)
        );
        assert!(rendered.exists());

        let rejected = dir.join(format!("{label}-rejected.md"));
        let wrong = hwp()
            .arg("convert")
            .arg(&input)
            .arg("-o")
            .arg(&rejected)
            .arg("--password")
            .arg("wrong-password")
            .output()
            .unwrap();
        assert!(!wrong.status.success());
        assert!(
            !rejected.exists(),
            "wrong credentials must not publish output"
        );
        assert!(refusal(&wrong.stderr).contains("HWP_PASSWORD_REQUIRED_OR_INVALID"));
        assert!(!refusal(&wrong.stderr).contains(password));
    }
}
