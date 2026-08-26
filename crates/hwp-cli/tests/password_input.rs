//! Released-binary password input coverage for `hwp cat`.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aes::Aes128;
use aes::cipher::{Block, BlockCipherEncrypt, KeyInit};
use sha1::Digest as _;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn fixture() -> PathBuf {
    let path = repo().join("fixtures/samples/report-tables.hwpx");
    assert!(path.exists(), "missing committed fixture: {}", path.display());
    path
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

fn evidenced_hwp5_fixture(dir: &Path, password: &str) -> PathBuf {
    let path = dir.join("protected.hwp");
    let converted = hwp()
        .arg("convert")
        .arg(fixture())
        .arg("-o")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        converted.status.success(),
        "base fixture conversion failed: {}",
        String::from_utf8_lossy(&converted.stderr)
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
    assert!(!direct.stdout.is_empty(), "decrypted document has text output");

    let mut stdin = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    stdin.stdin.take().unwrap().write_all(format!("{password}\r\n").as_bytes()).unwrap();
    let stdin = stdin.wait_with_output().unwrap();
    assert!(
        stdin.status.success(),
        "password stdin failed: {}",
        refusal(&stdin.stderr)
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
