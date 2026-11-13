//! CLI surface coverage — subcommand smokes that run in CI.
//!
//! Previously the `mcp`/`diff`/`render`/PDF paths were gated on local fixtures, so CI
//! coverage was zero. This suite hard-depends on the **committed fixture**
//! (fixtures/samples/report-tables.hwpx) and runs without skips (zero new dependencies —
//! existing hwp5/serde_json deps are reused).
//!
//! Font note: no fonts are bundled in the repo (`/fonts/` is gitignored). Render-path
//! glyphs come from system fonts (ubuntu CI installs fonts-nanum — glyf-outline TTFs;
//! the CFF-based fonts-noto-cjk made debug-build rendering ~100x slower — macOS uses its
//! default CJK fonts). Every assertion here is **font-independent** (page size derives
//! from secPr, self-diff, PDF structure) — do not add font-dependent assertions (glyphs,
//! page counts) here.

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

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

/// Each test gets a dedicated temporary directory, cleared and recreated at startup to avoid
/// PID reuse or prior-run residue; render tests count directory files.
fn tmp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("hwp-cli-surface-{}", std::process::id()))
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// --pages 1 emits exactly out.png, with PNG signature and A4@96dpi IHDR dimensions 794x1123.
/// Dimensions come from secPr rather than fonts, so CI font selection does not affect them.
/// The minimum size rejects blank-white-page regressions: a pure white image compresses
/// to a few KB, whereas actual ink from rules and glyphs makes the file much larger.
#[test]
fn render_png_page1_smoke() {
    let dir = tmp_dir("render_png");
    let out = dir.join("page1.png");
    let r = hwp()
        .arg("render")
        .arg(fixture())
        .arg("-o")
        .arg(&out)
        .args(["--pages", "1"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "render: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    // Exactly one out.png; multiple pages would use out-N.png.
    let siblings = dir.read_dir().unwrap().count();
    assert_eq!(siblings, 1, "단일 페이지 출력 파일 1개");

    let data = std::fs::read(&out).unwrap();
    assert_eq!(&data[..8], b"\x89PNG\r\n\x1a\n", "PNG 시그니처");
    let w = u32::from_be_bytes(data[16..20].try_into().unwrap());
    let h = u32::from_be_bytes(data[20..24].try_into().unwrap());
    assert_eq!((w, h), (794, 1123), "A4 @ 96dpi");
    assert!(
        data.len() > 20_000,
        "잉크 있는 페이지 크기: {}B",
        data.len()
    );
}

/// Feed rendered output back as its reference PNG; every diff metric must match exactly.
/// Diff exits zero even on mismatches, so assert its output metric strings.
/// Then compare page 1 with a page-2 reference and require a nonzero difference,
/// preventing a blank renderer from passing through self-comparison alone.
#[test]
fn diff_self_consistency() {
    let dir = tmp_dir("diff_self");
    let p1 = dir.join("p1.png");
    let p2 = dir.join("p2.png");
    for (png, page) in [(&p1, "1"), (&p2, "2")] {
        assert!(
            hwp()
                .arg("render")
                .arg(fixture())
                .arg("-o")
                .arg(png)
                .args(["--pages", page])
                .status()
                .unwrap()
                .success()
        );
    }
    let diff = |reference: &PathBuf| -> String {
        let r = hwp()
            .arg("diff")
            .arg(fixture())
            .arg("--ref")
            .arg(reference)
            .args(["--page", "1"])
            .arg("-o")
            .arg(dir.join("d.png"))
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "diff: {}",
            String::from_utf8_lossy(&r.stderr)
        );
        String::from_utf8_lossy(&r.stdout).into_owned()
    };
    // 1. Self-comparison: exactly zero.
    let same = diff(&p1);
    for needle in [
        "잉크 적용률(완전성): 100.0%",
        "dx=0px, dy=0px",
        "픽셀 차이율: 0.00%",
    ] {
        assert!(same.contains(needle), "자기 일치 지표: {needle}\n{same}");
    }
    // 2. Cross-page comparison must detect a difference.
    let cross = diff(&p2);
    assert!(
        !cross.contains("픽셀 차이율: 0.00%"),
        "1쪽 vs 2쪽은 차이가 나야 정상(전부 백지 렌더 회귀 검출): {cross}"
    );
}

/// Both PDF paths, delegated convert and direct render, emit structurally valid PDFs.
/// Do not pin page counts, which depend on font reflow.
/// Check startxref points to the actual xref location, rejecting broken trailers.
#[test]
fn pdf_smoke_convert_and_render_paths() {
    let dir = tmp_dir("pdf_smoke");
    let check = |out: PathBuf, label: &str| {
        let data = std::fs::read(&out).unwrap();
        assert!(data.starts_with(b"%PDF-"), "{label}: %PDF- 헤더");
        assert!(
            data.windows(5).rev().take(2048).any(|w| w == b"%%EOF"),
            "{label}: %%EOF 트레일러"
        );
        let pages = data.windows(12).filter(|w| *w == b"/Type /Pages").count();
        assert_eq!(pages, 1, "{label}: /Type /Pages는 1개");
        let page = data.windows(11).filter(|w| *w == b"/Type /Page").count();
        assert!(page >= 2, "{label}: /Type /Page 마커(루트+페이지들) >= 2");
        assert!(data.len() > 10_000, "{label}: 내용 있는 크기 (>10KB)");
        // startxref must point within the file to an xref table or xref-stream object.
        let tail = String::from_utf8_lossy(&data[data.len().saturating_sub(2048)..]);
        let off: usize = tail
            .rsplit_once("startxref")
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("{label}: startxref 오프셋 파싱 실패"));
        assert!(off < data.len(), "{label}: startxref 오프셋 범위");
        let at = &data[off..(off + 32).min(data.len())];
        assert!(
            at.starts_with(b"xref") || at.windows(4).any(|w| w == b" obj"),
            "{label}: startxref가 xref/xref 스트림을 가리켜야: {:?}",
            String::from_utf8_lossy(at)
        );
    };
    // Delegated convert path: hwp convert -o x.pdf delegates to render.
    let c = dir.join("conv.pdf");
    let r1 = hwp()
        .arg("convert")
        .arg(fixture())
        .arg("-o")
        .arg(&c)
        .output()
        .unwrap();
    assert!(
        r1.status.success(),
        "convert pdf: {}",
        String::from_utf8_lossy(&r1.stderr)
    );
    check(c, "convert");
    // Direct render path.
    let d = dir.join("rend.pdf");
    let r2 = hwp()
        .arg("render")
        .arg(fixture())
        .arg("-o")
        .arg(&d)
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "render pdf: {}",
        String::from_utf8_lossy(&r2.stderr)
    );
    check(d, "render");
}

/// MCP stdio session uses line-delimited JSON-RPC, as observed, rather than Content-Length framing.
/// initialize, initialized, tools/list, tools/call(hwp_validate), then EOF on stdin to terminate.
/// Read via a thread/channel with recv_timeout(60s), and exit via try_wait/kill(30s), avoiding CI hangs.
#[test]
fn mcp_stdio_session() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let mut child = hwp()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("hwp mcp spawn");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let recv = |what: &str| -> serde_json::Value {
        let line = rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("MCP 응답 타임아웃: {what}"));
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("JSON 파싱: {line}"))
    };
    let mut send = |v: serde_json::Value| {
        stdin
            .write_all(serde_json::to_string(&v).unwrap().as_bytes())
            .unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };

    send(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2024-11-05","capabilities":{},
        "clientInfo":{"name":"cli_surface","version":"0"}}}),
    );
    let init = recv("initialize");
    assert_eq!(init["id"], 1);
    assert!(
        init["result"]["serverInfo"]["name"].is_string(),
        "serverInfo: {init}"
    );

    send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = recv("tools/list");
    let mut names: Vec<String> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    let expect: Vec<String> = [
        "hwp_certify",
        "hwp_compare",
        "hwp_compose",
        "hwp_convert",
        "hwp_diff",
        "hwp_edit",
        "hwp_fill",
        "hwp_get_file",
        "hwp_grep",
        "hwp_info",
        "hwp_lint",
        "hwp_list_bookmarks",
        "hwp_list_fields",
        "hwp_merge",
        "hwp_new",
        "hwp_put_file",
        "hwp_read",
        "hwp_render",
        "hwp_slots",
        "hwp_split",
        "hwp_styles",
        "hwp_template",
        "hwp_validate",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(names, expect, "도구 23종");

    send(
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name":"hwp_validate","arguments":{"path": fixture().to_string_lossy()}}}),
    );
    let call = recv("tools/call");
    assert_eq!(call["id"], 3);
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(v["valid"], true, "hwp_validate 결과: {text}");

    // hwp_lint: violating Markdown inside the root yields findings with rule_id/line/message.
    let dir = tmp_dir("mcp_lint_call");
    let violating = dir.join("violating.md");
    std::fs::write(&violating, "시행일: 2020.7.8\n").unwrap();
    send(
        serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
        "name":"hwp_lint","arguments":{"path": violating.to_string_lossy()}}}),
    );
    let call = recv("tools/call");
    assert_eq!(call["id"], 4);
    assert_eq!(call["result"]["isError"], false, "hwp_lint 응답: {call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(report["contract"], "hwp-lint-report-v1", "계약: {text}");
    let findings = report["findings"].as_array().unwrap();
    assert!(!findings.is_empty(), "위반 파일에 findings 없음: {text}");
    for f in findings {
        assert!(f["rule_id"].is_string(), "rule_id 누락: {f}");
        assert!(f["line"].is_number(), "line 누락: {f}");
        assert!(f["message"].is_string(), "message 누락: {f}");
    }

    // EOF on stdin signals exit; try_wait for up to 30 seconds, then kill.
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            assert!(status.success(), "MCP 종료 코드: {status}");
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("MCP가 stdin EOF 후 30s 내 종료하지 않음");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// hwp_lint sandbox: inspect files inside --root; checked_read_path refuses external paths
/// with isError=true. Pins D-09/T-02.3-01.
#[test]
fn mcp_hwp_lint_root_sandbox() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let dir = tmp_dir("mcp_lint_sandbox");
    let in_root = dir.join("violating.md");
    std::fs::write(&in_root, "시행일: 2020.7.8\n").unwrap();

    let mut child = hwp()
        .arg("mcp")
        .arg("--root")
        .arg(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("hwp mcp spawn");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let recv = |what: &str| -> serde_json::Value {
        let line = rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("MCP 응답 타임아웃: {what}"));
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("JSON 파싱: {line}"))
    };
    let mut send = |v: serde_json::Value| {
        stdin
            .write_all(serde_json::to_string(&v).unwrap().as_bytes())
            .unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    };

    send(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2024-11-05","capabilities":{},
        "clientInfo":{"name":"cli_surface","version":"0"}}}),
    );
    recv("initialize");
    send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    // Inside the root: findings must be returned.
    send(
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"hwp_lint","arguments":{"path": in_root.to_string_lossy()}}}),
    );
    let call = recv("tools/call in-root");
    assert_eq!(call["result"]["isError"], false, "in-root: {call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let report: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(
        !report["findings"].as_array().unwrap().is_empty(),
        "in-root 위반 파일에 findings 없음: {text}"
    );

    // Outside the root: checked_read_path refuses with isError=true.
    send(
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name":"hwp_lint","arguments":{"path": fixture().to_string_lossy()}}}),
    );
    let call = recv("tools/call out-of-root");
    assert_eq!(
        call["result"]["isError"], true,
        "루트 밖 경로가 거부되지 않음: {call}"
    );

    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            assert!(status.success(), "MCP 종료 코드: {status}");
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("MCP가 stdin EOF 후 30s 내 종료하지 않음");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// HWP5 synthesis round-trip gate: convert the HWPX fixture to a.hwp, then convert it to
/// b.hwp with --preserve-layout and assert exact stream-level byte identity.
///
/// This CI-safe synthetic case complements the local genuine-file identity
/// gate. A same-format no-op now copies the immutable HWP source snapshot, so
/// the whole file, including CFB directory metadata, must be byte-identical.
#[test]
fn hwp5_synthetic_identity_gate() {
    let dir = tmp_dir("hwp5_identity");
    let a = dir.join("a.hwp");
    let r1 = hwp()
        .arg("convert")
        .arg(fixture())
        .arg("-o")
        .arg(&a)
        .output()
        .unwrap();
    assert!(
        r1.status.success(),
        "hwpx→hwp: {}",
        String::from_utf8_lossy(&r1.stderr)
    );
    let b = dir.join("b.hwp");
    let r2 = hwp()
        .arg("convert")
        .arg(&a)
        .arg("-o")
        .arg(&b)
        .arg("--preserve-layout")
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "hwp→hwp(preserve-layout): {}",
        String::from_utf8_lossy(&r2.stderr)
    );

    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "same-format no-op is an exact source snapshot copy"
    );

    let mut ca = hwp5::Hwp5Container::open(&a).unwrap();
    let mut cb = hwp5::Hwp5Container::open(&b).unwrap();
    let sa: Vec<String> = ca.list_streams().iter().map(|s| s.path.clone()).collect();
    let sb: Vec<String> = cb.list_streams().iter().map(|s| s.path.clone()).collect();
    assert_eq!(sa, sb, "스트림 목록 동일");
    for name in &sa {
        let ra = ca.read_stream_raw(name).unwrap();
        let rb = cb.read_stream_raw(name).unwrap();
        assert_eq!(ra, rb, "스트림 바이트 동일: {name}");
    }
}

// --- GATE-02: protection refusal through the release binary, end to end. ---

/// HWP5 FileHeader attribute bits (DWORD at 36..40), synchronized with the private
/// attr module in crates/hwp5/src/file_header.rs. Library changes must update these
/// constants because this test mirrors the gate's actual wiring.
mod gate_bits {
    pub const ENCRYPTED: u32 = 1 << 1;
    pub const DRM: u32 = 1 << 4;
    pub const HAS_SIGNATURE: u32 = 1 << 7;
    pub const CERT_ENCRYPTED: u32 = 1 << 8;
    pub const CERT_DRM: u32 = 1 << 10;
}

/// Synthesize protection fixtures by converting the committed HWPX sample to HWP5,
/// then OR bits into the FileHeader attribute DWORD (36..40, LE). Retain the writer's
/// compression bit. One call creates a fixture for each new refusal branch.
fn protected_hwp5_fixture(dir: &Path, name: &str, bits: u32) -> PathBuf {
    let base = dir.join(format!("{name}.hwp"));
    let r = hwp()
        .arg("convert")
        .arg(fixture())
        .arg("-o")
        .arg(&base)
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "기반 hwp5 합성: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    let mut cfb = cfb::open_rw(&base).unwrap();
    let mut header = Vec::new();
    {
        let mut stream = cfb.open_stream("/FileHeader").unwrap();
        std::io::Read::read_to_end(&mut stream, &mut header).unwrap();
    }
    let attrs = u32::from_le_bytes(header[36..40].try_into().unwrap());
    header[36..40].copy_from_slice(&(attrs | bits).to_le_bytes());
    cfb.create_stream("/FileHeader")
        .unwrap()
        .write_all(&header)
        .unwrap();
    drop(cfb);
    base
}

/// Assert hwp cat refuses and stderr includes the exact Korean condition sentence.
/// Check the full condition and remedy to distinguish sibling variants (D-06);
/// return all stderr so callers can add assertions.
fn assert_refusal_names_condition(file: &Path, sentence: &str) -> String {
    let r = hwp().arg("cat").arg(file).output().unwrap();
    assert!(!r.status.success(), "거부 기대: {}", file.display());
    let stderr = String::from_utf8_lossy(&r.stderr).into_owned();
    assert!(
        stderr.contains(sentence),
        "문장 {sentence:?} 부재. stderr: {stderr}"
    );
    stderr
}

#[test]
fn password_encrypted_document_refuses_by_name() {
    let dir = tmp_dir("refuse_password");
    let f = protected_hwp5_fixture(&dir, "pw", gate_bits::ENCRYPTED);
    assert_refusal_names_condition(
        &f,
        "암호가 필요하거나 올바르지 않습니다. --password-stdin으로 암호를 전달하세요.",
    );
}

#[test]
fn certificate_encrypted_document_refuses_by_name() {
    let dir = tmp_dir("refuse_cert_enc");
    let f = protected_hwp5_fixture(&dir, "cert_enc", gate_bits::CERT_ENCRYPTED);
    assert_refusal_names_condition(
        &f,
        "공인 인증서로 암호화된 문서는 지원하지 않습니다. 한글에서 인증서 암호화를 해제한 뒤 다시 저장하세요.",
    );
}

#[test]
fn certificate_drm_document_refuses_by_name() {
    let dir = tmp_dir("refuse_cert_drm");
    let f = protected_hwp5_fixture(&dir, "cert_drm", gate_bits::CERT_DRM);
    assert_refusal_names_condition(
        &f,
        "공인 인증서 DRM으로 보호된 문서는 지원하지 않습니다. 한글에서 인증서 DRM 보안을 해제한 뒤 다시 저장하세요.",
    );
}

#[test]
fn drm_document_refuses_by_name() {
    let dir = tmp_dir("refuse_drm");
    let f = protected_hwp5_fixture(&dir, "drm", gate_bits::DRM);
    assert_refusal_names_condition(
        &f,
        "DRM으로 보호된 문서는 지원하지 않습니다. 한글에서 DRM 보안을 해제한 뒤 다시 저장하세요.",
    );
}

#[test]
fn signed_document_refuses_by_name() {
    let dir = tmp_dir("refuse_signed");
    let f = protected_hwp5_fixture(&dir, "signed", gate_bits::HAS_SIGNATURE);
    assert_refusal_names_condition(
        &f,
        "서명된 문서는 지원하지 않습니다. 한글에서 서명을 제거한 뒤 다시 저장하세요.",
    );
}

/// HWPX gate: repack the committed sample after adding encryption-data to its manifest.
/// Keep mimetype first and uncompressed under OPC rules; require encryption refusal
/// rather than the downstream XML parsing error seen before GATE-02.
#[test]
fn encrypted_package_refuses_by_name() {
    let dir = tmp_dir("refuse_encrypted_package");
    let repacked = dir.join("encrypted.hwpx");

    const ENCRYPTED_MANIFEST: &[u8] = br##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><odf:file-entry full-path="Contents/header.xml" media-type="application/xml" size="1"><odf:encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="AAAA"><odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="AAAA"/><odf:key-derivation key-derivation-name="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2" key-size="32" iteration-count="1024" salt="AAAA"/><odf:start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256" key-size="32"/></odf:encryption-data></odf:file-entry></odf:manifest>"##;

    let mut archive = zip::ZipArchive::new(std::fs::File::open(fixture()).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&repacked).unwrap());
    let mimetype_options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("mimetype", mimetype_options).unwrap();
    writer
        .write_all(hwpx::package::MIMETYPE.as_bytes())
        .unwrap();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().unwrap().into_owned();
        if name == "mimetype" {
            continue;
        }
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
        if name == "META-INF/manifest.xml" {
            data = ENCRYPTED_MANIFEST.to_vec();
        }
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file(&name, options).unwrap();
        writer.write_all(&data).unwrap();
    }
    writer.finish().unwrap();

    let stderr = assert_refusal_names_condition(
        &repacked,
        "암호가 필요하거나 올바르지 않습니다. --password-stdin으로 암호를 전달하세요.",
    );
    assert!(
        !stderr.contains("XML 파싱 오류"),
        "하류 XML 파싱 오류로 떨어지면 안 됨: {stderr}"
    );
    assert!(
        !stderr.contains("header.xml"),
        "헤더 콘텐츠 파트 이름을 노출하면 안 됨: {stderr}"
    );
}

/// For all six refusals, structurally assert a remedy sentence follows the condition (D-08).
/// Pin the presence of a second sentence rather than its exact wording, allowing
/// copy changes while rejecting removal of the hint.
#[test]
fn every_refusal_carries_a_remedy_hint() {
    let dir = tmp_dir("refusal_hints");
    let cases: Vec<(PathBuf, &str)> = [
        (
            "pw",
            gate_bits::ENCRYPTED,
            "암호가 필요하거나 올바르지 않습니다",
        ),
        (
            "cert_enc",
            gate_bits::CERT_ENCRYPTED,
            "공인 인증서로 암호화된 문서는",
        ),
        (
            "cert_drm",
            gate_bits::CERT_DRM,
            "공인 인증서 DRM으로 보호된 문서는",
        ),
        ("drm", gate_bits::DRM, "DRM으로 보호된 문서는"),
        ("signed", gate_bits::HAS_SIGNATURE, "서명된 문서는"),
    ]
    .into_iter()
    .map(|(name, bits, prefix)| (protected_hwp5_fixture(&dir, name, bits), prefix))
    .collect();

    for (file, prefix) in cases {
        let stderr = assert_refusal_names_condition(&file, prefix);
        // Every refusal names its condition and then a remedy. Split on the
        // first sentence end rather than one refusal's wording, so a message
        // that stops using the original Korean refusal phrase remains held to the contract.
        let after_condition = stderr
            .split_once(". ")
            .map(|(_, rest)| rest.trim())
            .unwrap_or_else(|| panic!("조건 문장 종결 부재: {stderr}"));
        assert!(!after_condition.is_empty(), "해결 힌트 문장 부재: {stderr}");
    }
}
