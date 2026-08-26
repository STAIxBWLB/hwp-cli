//! Released-binary password input coverage for `hwp cat`.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use aes::cipher::{
    Block, BlockCipherEncrypt, BlockModeEncrypt, KeyInit, KeyIvInit, block_padding::NoPadding,
};
use aes::{Aes128, Aes256};
use base64::Engine as _;
use flate2::{Compression, write::DeflateEncoder};
use serde_json::{Value, json};
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
    cfb.create_storage("/Opaque").unwrap();
    cfb.create_new_stream("/Opaque/preserved.bin")
        .unwrap()
        .write_all(b"source-only opaque stream")
        .unwrap();
    cfb.flush().unwrap();
    path
}

fn refusal(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr).into_owned()
}

fn stdin_bytes(mut command: Command, bytes: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    child.wait_with_output().unwrap()
}

fn mcp_calls(root: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = hwp()
        .arg("mcp")
        .arg("--root")
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start MCP server");
    let mut input = String::new();
    for request in requests {
        input.push_str(&request.to_string());
        input.push('\n');
    }
    child
        .stdin
        .take()
        .expect("MCP stdin")
        .write_all(input.as_bytes())
        .expect("write MCP requests");
    let output = child.wait_with_output().expect("wait for MCP server");
    assert!(
        output.status.success(),
        "MCP server must finish cleanly: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("MCP stdout is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("MCP response JSON"))
        .collect()
}

fn mcp_call(id: u64, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    })
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
        if name == "Contents/header.xml"
            || name == "Contents/section0.xml"
            || name == "Preview/PrvText.txt"
        {
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
    let custom_name = "META-INF/custom.xml";
    let custom_plaintext = b"<custom>authenticated opaque entry</custom>";
    let (custom_ciphertext, custom_manifest_entry) =
        encrypt_hwpx_part(custom_name, custom_plaintext, password);
    manifest_entries.push(custom_manifest_entry);
    writer
        .start_file(
            custom_name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(&custom_ciphertext).unwrap();
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
fn password_stdin_rejects_an_oversized_line_before_document_access() {
    let mut oversized = vec![b'x'; 64 * 1024 + 1];
    oversized.push(b'\n');
    let mut command = hwp();
    command
        .arg("cat")
        .arg("does-not-exist.hwp")
        .arg("--password-stdin");
    let output = stdin_bytes(command, &oversized);
    assert!(!output.status.success());
    let stderr = refusal(&output.stderr);
    assert!(stderr.contains("65536바이트 제한"));
    assert!(!stderr.contains("does-not-exist"));
    assert!(!stderr.contains(&"x".repeat(128)));
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

    let correct_preview = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--preview")
        .arg("--password")
        .arg(password)
        .output()
        .unwrap();
    assert!(correct_preview.status.success());
    let absent_preview = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--preview")
        .output()
        .unwrap();
    let wrong_preview = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--preview")
        .arg("--password")
        .arg("different-secret")
        .output()
        .unwrap();
    assert!(!absent_preview.status.success() && !wrong_preview.status.success());
    assert_eq!(
        refusal(&absent_preview.stderr),
        refusal(&wrong_preview.stderr)
    );
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

    let correct_preview = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--preview")
        .arg("--password")
        .arg(password)
        .output()
        .unwrap();
    assert!(correct_preview.status.success());
    let absent_preview = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--preview")
        .output()
        .unwrap();
    let wrong_preview = hwp()
        .arg("cat")
        .arg(&file)
        .arg("--preview")
        .arg("--password")
        .arg("  가  ")
        .output()
        .unwrap();
    assert!(!absent_preview.status.success() && !wrong_preview.status.success());
    assert_eq!(
        refusal(&absent_preview.stderr),
        refusal(&wrong_preview.stderr)
    );
}

#[test]
fn plain_hwpx_accepts_an_ignored_password_in_cli_and_mcp() {
    let dir = temp_dir("plain-hwpx-password");
    let plain = dir.join("plain.hwpx");
    let created = hwp().arg("new").arg("-o").arg(&plain).output().unwrap();
    assert!(created.status.success(), "plain HWPX creation failed");

    let cli = hwp()
        .arg("cat")
        .arg(&plain)
        .arg("--password")
        .arg("ignored-password")
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "an unencrypted HWPX must stay readable: {}",
        refusal(&cli.stderr)
    );

    let responses = mcp_calls(
        &dir,
        &[mcp_call(
            1,
            "hwp_read",
            json!({"path": plain, "format": "plain", "password": "ignored-password"}),
        )],
    );
    assert_eq!(responses[0]["result"]["isError"], false);
}

#[test]
fn mcp_read_password_is_per_call_closed_and_sandboxed() {
    let password = "mcp-read-secret";
    let dir = temp_dir("mcp-read");
    let root = dir.join("root");
    std::fs::create_dir_all(&root).unwrap();
    let hwp5_dir = root.join("hwp5");
    let hwpx_dir = root.join("hwpx");
    let outside_dir = dir.join("outside");
    std::fs::create_dir_all(&hwp5_dir).unwrap();
    std::fs::create_dir_all(&hwpx_dir).unwrap();
    std::fs::create_dir_all(&outside_dir).unwrap();
    let hwp5 = evidenced_hwp5_fixture(&hwp5_dir, password);
    let hwpx = evidenced_hwpx_fixture(&hwpx_dir, password);
    let outside = evidenced_hwp5_fixture(&outside_dir, password);

    let responses = mcp_calls(
        &root,
        &[
            mcp_call(1, "hwp_read", json!({"path": hwp5, "format": "plain"})),
            mcp_call(
                2,
                "hwp_read",
                json!({"path": hwp5, "format": "plain", "password": "wrong-password"}),
            ),
            mcp_call(
                3,
                "hwp_read",
                json!({"path": hwp5, "format": "plain", "password": password}),
            ),
            mcp_call(
                4,
                "hwp_read",
                json!({"path": hwpx, "format": "plain", "password": password}),
            ),
            mcp_call(5, "hwp_read", json!({"path": hwp5, "format": "plain"})),
            mcp_call(
                6,
                "hwp_read",
                json!({"path": outside, "format": "plain", "password": password}),
            ),
        ],
    );
    assert_eq!(responses.len(), 6);

    let absent = &responses[0]["result"];
    let wrong = &responses[1]["result"];
    assert_eq!(absent["isError"], true);
    assert_eq!(wrong["isError"], true);
    assert_eq!(absent["structuredContent"], wrong["structuredContent"]);
    assert_eq!(
        absent["structuredContent"]["code"],
        "HWP_PASSWORD_REQUIRED_OR_INVALID"
    );
    for response in [&responses[0], &responses[1], &responses[4], &responses[5]] {
        let serialized = response.to_string();
        assert!(!serialized.contains(password));
        assert!(!serialized.contains("wrong-password"));
    }
    assert_eq!(responses[2]["result"]["isError"], false);
    assert_eq!(responses[3]["result"]["isError"], false);
    assert_eq!(responses[4]["result"]["isError"], true);
    assert_eq!(responses[5]["result"]["isError"], true);
    assert_ne!(
        responses[5]["result"]["structuredContent"]["code"], "HWP_PASSWORD_REQUIRED_OR_INVALID",
        "root rejection must happen before password-aware loading"
    );
}

#[test]
fn mcp_convert_render_passwords_are_per_call_and_publish_atomically() {
    let password = "mcp-output-secret";
    let dir = temp_dir("mcp-convert-render");
    let root = dir.join("root");
    let hwp5_dir = root.join("hwp5");
    let hwpx_dir = root.join("hwpx");
    std::fs::create_dir_all(&hwp5_dir).unwrap();
    std::fs::create_dir_all(&hwpx_dir).unwrap();
    let fixtures = [
        ("hwp5", evidenced_hwp5_fixture(&hwp5_dir, password)),
        ("hwpx", evidenced_hwpx_fixture(&hwpx_dir, password)),
    ];

    let tools = mcp_calls(
        &root,
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})],
    );
    for name in ["hwp_read", "hwp_convert", "hwp_render"] {
        let tool = tools[0]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            tool["inputSchema"]["properties"]["password"]["type"],
            "string"
        );
    }

    for (index, (label, input)) in fixtures.iter().enumerate() {
        let converted = root.join(format!("{label}.md"));
        let rendered = root.join(format!("{label}.svg"));
        let wrong_convert = root.join(format!("{label}-wrong.md"));
        let absent_render = root.join(format!("{label}-absent.svg"));
        let responses = mcp_calls(
            &root,
            &[
                mcp_call(
                    10 + index as u64,
                    "hwp_convert",
                    json!({"input": input, "output": converted, "password": password}),
                ),
                mcp_call(
                    20 + index as u64,
                    "hwp_render",
                    json!({"path": input, "output_path": rendered, "format": "svg", "password": password}),
                ),
                mcp_call(
                    30 + index as u64,
                    "hwp_convert",
                    json!({"input": input, "output": wrong_convert, "password": "wrong-password"}),
                ),
                mcp_call(
                    40 + index as u64,
                    "hwp_render",
                    json!({"path": input, "output_path": absent_render, "format": "svg"}),
                ),
                mcp_call(
                    50 + index as u64,
                    "hwp_info",
                    json!({"path": input, "password": password}),
                ),
            ],
        );
        assert_eq!(responses[0]["result"]["isError"], false);
        assert_eq!(responses[1]["result"]["isError"], false);
        assert!(
            converted.exists(),
            "{label} convert must publish after decrypt"
        );
        assert!(
            rendered.exists(),
            "{label} render must publish after decrypt"
        );
        assert_eq!(responses[2]["result"]["isError"], true);
        assert_eq!(responses[3]["result"]["isError"], true);
        assert_eq!(
            responses[2]["result"]["structuredContent"],
            responses[3]["result"]["structuredContent"]
        );
        assert!(!wrong_convert.exists());
        assert!(!absent_render.exists());
        assert_eq!(responses[4]["result"]["isError"], true);
        for response in &responses[2..] {
            let serialized = response.to_string();
            assert!(!serialized.contains(password));
            assert!(!serialized.contains("wrong-password"));
        }
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

        let native_extension = if label == "hwp5" { "hwp" } else { "hwpx" };
        let native = dir.join(format!("{label}-decrypted.{native_extension}"));
        let native_convert = hwp()
            .arg("convert")
            .arg(&input)
            .arg("-o")
            .arg(&native)
            .arg("--password")
            .arg(password)
            .output()
            .unwrap();
        assert!(
            native_convert.status.success(),
            "{label} same-format conversion with a password must succeed: {}",
            refusal(&native_convert.stderr)
        );
        let reopened = hwp().arg("cat").arg(&native).output().unwrap();
        assert!(
            reopened.status.success(),
            "{label} same-format output must reopen without a password: {}",
            refusal(&reopened.stderr)
        );
        if label == "hwp5" {
            assert!(
                !hwp5::Hwp5Container::open(&native)
                    .unwrap()
                    .file_header()
                    .is_encrypted(),
                "same-format HWP output must be an ordinary plaintext package"
            );
            let strict_native = dir.join("hwp5-decrypted-strict.hwp");
            let strict_convert = hwp()
                .arg("convert")
                .arg(&input)
                .arg("-o")
                .arg(&strict_native)
                .arg("--password")
                .arg(password)
                .arg("--strict")
                .output()
                .unwrap();
            assert!(
                !strict_convert.status.success(),
                "strict conversion must refuse an opaque stream that plaintext synthesis drops"
            );
            assert!(!strict_native.exists());
        } else {
            let mut package = hwpx::HwpxPackage::open(&native).unwrap();
            assert!(
                !package.has_encryption_marker().unwrap(),
                "same-format HWPX output must discard source encryption metadata"
            );
        }

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

#[test]
fn cli_password_scope_and_korean_help_are_exact() {
    for command in ["cat", "convert", "render"] {
        let help = hwp()
            .env("HWP_LANG", "ko")
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(help.status.success());
        let stdout = String::from_utf8_lossy(&help.stdout);
        assert!(stdout.contains("--password"));
        assert!(stdout.contains("명령줄에서 직접 입력할 암호"));
        assert!(stdout.contains("표준 입력에서 UTF-8 암호 한 줄 읽기"));
    }

    for command in [
        "info", "grep", "new", "compose", "edit", "validate", "dump", "diff", "skill",
    ] {
        let rejected = hwp()
            .args([command, "--password", "must-not-be-accepted"])
            .output()
            .unwrap();
        assert!(
            !rejected.status.success(),
            "{command} must remain outside password scope"
        );
        assert!(
            !refusal(&rejected.stderr).contains("must-not-be-accepted"),
            "{command} must not echo rejected credentials"
        );
    }
}

#[test]
fn cli_convert_render_password_stdin_and_refusals_never_publish() {
    let password = "  \u{ac00}  ";
    let dir = temp_dir("convert-render-stdin-refusals");
    std::fs::create_dir_all(dir.join("hwp5")).unwrap();
    std::fs::create_dir_all(dir.join("hwpx")).unwrap();
    let fixtures = [
        ("hwp5", evidenced_hwp5_fixture(&dir.join("hwp5"), password)),
        ("hwpx", evidenced_hwpx_fixture(&dir.join("hwpx"), password)),
    ];
    let mut failures = Vec::new();

    for (label, input) in fixtures {
        let converted = dir.join(format!("{label}-stdin.md"));
        let converted_result = stdin_bytes(
            {
                let mut command = hwp();
                command
                    .arg("convert")
                    .arg(&input)
                    .arg("-o")
                    .arg(&converted)
                    .arg("--password-stdin");
                command
            },
            format!("{password}\r\n").as_bytes(),
        );
        assert!(
            converted_result.status.success(),
            "{label} convert password stdin must succeed: {}",
            refusal(&converted_result.stderr)
        );
        assert!(converted.exists());

        let rendered = dir.join(format!("{label}-stdin.svg"));
        let rendered_result = stdin_bytes(
            {
                let mut command = hwp();
                command
                    .arg("render")
                    .arg(&input)
                    .arg("-o")
                    .arg(&rendered)
                    .arg("--password-stdin");
                command
            },
            format!("{password}\n").as_bytes(),
        );
        assert!(
            rendered_result.status.success(),
            "{label} render password stdin must succeed: {}",
            refusal(&rendered_result.stderr)
        );
        assert!(rendered.exists());

        for (command, extension) in [("convert", "md"), ("render", "svg")] {
            for credential in [None, Some("wrong-password")] {
                let destination = dir.join(format!(
                    "{label}-{command}-{}.{}",
                    if credential.is_some() {
                        "wrong"
                    } else {
                        "absent"
                    },
                    extension
                ));
                let report = dir.join(format!(
                    "{label}-{command}-{}.json",
                    if credential.is_some() {
                        "wrong"
                    } else {
                        "absent"
                    }
                ));
                let mut process = hwp();
                process.arg(command).arg(&input).arg("-o").arg(&destination);
                if command == "render" {
                    process.arg("--report").arg(&report);
                }
                if let Some(value) = credential {
                    process.arg("--password").arg(value);
                }
                let result = process.output().unwrap();
                assert!(!result.status.success());
                assert!(
                    !destination.exists(),
                    "{label} {command} must not publish output"
                );
                assert!(
                    !report.exists(),
                    "{label} {command} must not publish a report"
                );
                let stderr = refusal(&result.stderr);
                assert!(stderr.contains("HWP_PASSWORD_REQUIRED_OR_INVALID"));
                assert!(!stderr.contains(password));
                assert!(!stderr.contains("wrong-password"));
                failures.push(stderr);
            }
        }
    }
    assert!(failures.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn cli_convert_batch_authenticates_before_creating_outputs() {
    let dir = temp_dir("convert-batch-authentication");
    let first = evidenced_hwp5_fixture(&dir, "first-password");
    let second = dir.join("second.hwp");
    std::fs::copy(&first, &second).unwrap();
    let out_dir = dir.join("output");

    let failed = hwp()
        .arg("convert")
        .arg(&first)
        .arg(&second)
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--to")
        .arg("md")
        .arg("--password")
        .arg("wrong-password")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(
        !out_dir.exists(),
        "batch credential failure must not create output"
    );
    assert!(refusal(&failed.stderr).contains("HWP_PASSWORD_REQUIRED_OR_INVALID"));
}

#[test]
fn cli_convert_batch_and_document_stdin_keep_password_channels_distinct() {
    let password = "batch-password";
    let dir = temp_dir("convert-batch-and-stdin");
    let first = evidenced_hwp5_fixture(&dir, password);
    let second = dir.join("second.hwp");
    std::fs::copy(&first, &second).unwrap();
    let out_dir = dir.join("output");
    let batch = hwp()
        .arg("convert")
        .arg(&first)
        .arg(&second)
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--to")
        .arg("md")
        .arg("--password")
        .arg(password)
        .output()
        .unwrap();
    assert!(
        batch.status.success(),
        "one batch password must load every input: {}",
        refusal(&batch.stderr)
    );
    assert!(out_dir.join("protected.md").exists());
    assert!(out_dir.join("second.md").exists());

    let hwpx_dir = dir.join("hwpx");
    std::fs::create_dir_all(&hwpx_dir).unwrap();
    let first_hwpx = evidenced_hwpx_fixture(&hwpx_dir, password);
    let second_hwpx = hwpx_dir.join("second.hwpx");
    std::fs::copy(&first_hwpx, &second_hwpx).unwrap();
    let hwpx_out_dir = dir.join("hwpx-output");
    let hwpx_batch = hwp()
        .arg("convert")
        .arg(&first_hwpx)
        .arg(&second_hwpx)
        .arg("--out-dir")
        .arg(&hwpx_out_dir)
        .arg("--to")
        .arg("md")
        .arg("--password")
        .arg(password)
        .output()
        .unwrap();
    assert!(
        hwpx_batch.status.success(),
        "HWPX batch preflight must reserve before decrypting: {}",
        refusal(&hwpx_batch.stderr)
    );
    assert!(hwpx_out_dir.join("protected.md").exists());
    assert!(hwpx_out_dir.join("second.md").exists());

    let from_stdin = dir.join("stdin.md");
    let stdin_document = stdin_bytes(
        {
            let mut command = hwp();
            command
                .arg("convert")
                .arg("-")
                .arg("-o")
                .arg(&from_stdin)
                .arg("--password")
                .arg(password);
            command
        },
        &std::fs::read(&first).unwrap(),
    );
    assert!(
        stdin_document.status.success(),
        "direct passwords remain valid with document stdin: {}",
        refusal(&stdin_document.stderr)
    );
    assert!(from_stdin.exists());

    let blocked = dir.join("blocked.md");
    let collision = hwp()
        .arg("convert")
        .arg("-")
        .arg("-o")
        .arg(&blocked)
        .arg("--password-stdin")
        .output()
        .unwrap();
    assert!(!collision.status.success());
    assert!(!blocked.exists());
    assert!(
        refusal(&collision.stderr)
            .contains("문서 입력과 --password-stdin은 모두 표준 입력을 사용할 수 없습니다")
    );

    let non_first_collision = hwp()
        .arg("convert")
        .arg(&first)
        .arg("-")
        .arg("--out-dir")
        .arg(dir.join("non-first-output"))
        .arg("--to")
        .arg("md")
        .arg("--password-stdin")
        .output()
        .unwrap();
    assert!(!non_first_collision.status.success());
    assert!(
        refusal(&non_first_collision.stderr)
            .contains("문서 입력과 --password-stdin은 모두 표준 입력을 사용할 수 없습니다"),
        "a non-first stdin input must be rejected before password resolution: {}",
        refusal(&non_first_collision.stderr)
    );
}
