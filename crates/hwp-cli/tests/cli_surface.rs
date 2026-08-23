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
    Command::new(env!("CARGO_BIN_EXE_hwp"))
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

/// 테스트별 전용 임시 디렉토리 — 시작 시 비우고 재생성한다(PID 재사용·이전 실행
/// 잔재로 인한 오염 방지; render 테스트는 디렉토리 파일 개수를 검사하므로 필수).
fn tmp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("hwp-cli-surface-{}", std::process::id()))
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `--pages 1` 렌더는 정확히 out.png 1개, PNG 시그니처 + IHDR 794×1123(A4@96dpi —
/// 크기는 secPr 유래라 폰트 비의존이라 CI 폰트 환경에서도 동일).
/// 파일 크기 하한은 "빈 흰 페이지" 회귀 방지 — 794×1123 순백 PNG는 수 KB로 압축되므로
/// 실제 잉크(표 괘선·글리프)가 있으면 훨씬 커진다.
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
    // 정확히 out.png 단일 파일(다중 페이지면 out-N.png로 갈리는지 확인).
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

/// 자기 렌더를 기준 PNG로 되먹는 diff — 지표는 전부 완전 일치여야 한다
/// (diff는 불일치여도 exit 0이므로 **출력 지표 문자열이 검증 대상**).
/// 2부: 다른 쪽(2쪽)을 기준으로 주면 0.00%가 **아니어야** 한다 — 렌더러가 전부
/// 백지를 내도 자기일치가 통과하는 상호 승인 맹점을 막는 네거티브 게이트.
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
    // 1) 자기 일치: 완전 0.
    let same = diff(&p1);
    for needle in [
        "잉크 적용률(완전성): 100.0%",
        "dx=0px, dy=0px",
        "픽셀 차이율: 0.00%",
    ] {
        assert!(same.contains(needle), "자기 일치 지표: {needle}\n{same}");
    }
    // 2) 교차(1쪽 vs 2쪽 기준): 차이가 검출돼야 한다.
    let cross = diff(&p2);
    assert!(
        !cross.contains("픽셀 차이율: 0.00%"),
        "1쪽 vs 2쪽은 차이가 나야 정상(전부 백지 렌더 회귀 검출): {cross}"
    );
}

/// PDF 두 경로(convert 위임·render 직접) 모두 구조적으로 유효한 PDF를 낸다.
/// 정확한 페이지 수는 폰트 리플로우로 달라질 수 있어 단언하지 않는다.
/// startxref 오프셋이 실제 xref 자리를 가리키는지까지 확인한다(깨진 트레일러 회귀 방지).
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
        // startxref → 오프셋이 파일 안의 xref 테이블 또는 xref 스트림 객체를 가리켜야 한다.
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
    // convert 위임 경로 (hwp convert -o x.pdf → render 경로 위임).
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
    // render 직접 경로.
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

/// MCP stdio 세션 — 라인 단위 JSON-RPC(실측: Content-Length 프레이밍 아님).
/// initialize → initialized → tools/list → tools/call(hwp_validate) 후 stdin EOF로 종료.
/// 수신은 스레드+채널 recv_timeout(60s), 종료는 try_wait 루프+kill(30s) — CI 행 방지.
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
        "hwp_compose",
        "hwp_convert",
        "hwp_diff",
        "hwp_edit",
        "hwp_fill",
        "hwp_grep",
        "hwp_info",
        "hwp_lint",
        "hwp_list_bookmarks",
        "hwp_list_fields",
        "hwp_new",
        "hwp_read",
        "hwp_render",
        "hwp_slots",
        "hwp_template",
        "hwp_validate",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(names, expect, "도구 17종");

    send(
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name":"hwp_validate","arguments":{"path": fixture().to_string_lossy()}}}),
    );
    let call = recv("tools/call");
    assert_eq!(call["id"], 3);
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(v["valid"], true, "hwp_validate 결과: {text}");

    // hwp_lint: 루트 안 위반 markdown → findings 비어있지 않고 rule_id/line/message 보유.
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

    // stdin EOF = 종료 신호. try_wait 루프(최대 30s) 후 kill.
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

/// hwp_lint 샌드박스: --root 안의 파일은 검사되고, 루트 밖 경로는
/// checked_read_path가 거부한다(isError=true). D-09/T-02.3-01 고정.
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

    // 루트 안: findings가 돌아와야 한다.
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

    // 루트 밖: checked_read_path 거부(isError=true).
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

/// hwp5 합성 왕복 게이트: hwpx 픽스처 → a.hwp → `convert a.hwp -o b.hwp --preserve-layout`
/// 후 **스트림 단위 바이트 동일** 단언.
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

// --- GATE-02: 보호 문서 거부를 출시 바이너 end-to-end로 고정한다. ---

/// hwp5 FileHeader 속성 비트 (36..40 DWORD) — crates/hwp5/src/file_header.rs의
/// 비공개 `attr` 모듈과 동기화 유지. 라이브러리 쪽이 바뀌면 이 상수들도 바뀌어야
/// 하는 게 맞다(테스트가 게이트의 실제 전선을 미러링).
mod gate_bits {
    pub const ENCRYPTED: u32 = 1 << 1;
    pub const DRM: u32 = 1 << 4;
    pub const HAS_SIGNATURE: u32 = 1 << 7;
    pub const CERT_ENCRYPTED: u32 = 1 << 8;
    pub const CERT_DRM: u32 = 1 << 10;
}

/// 보호 비트 픽스처 합성: 커밋된 hwpx 샘플을 hwp5로 변환한 뒤 FileHeader 스트림의
/// 속성 DWORD(36..40, LE)에 `bits`를 OR해 다시 쓴다. 라이터가 항상 세우는 압축
/// 비트는 보존된다. 새 거부 브랜치 추가 시 이 함수 한 번 호출로 픽스처 하나.
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

/// `hwp cat`이 거부로 실패하고, stderr가 그 조건을 이름 붙인 정확한 한국어 문장을
/// 담는지 단언한다. 전체 문장(조건+해결 힌트)을 단언해 형제 변형과 구분되게 한다
/// (D-06). stderr 전문을 반환해 호출자가 추가 단언을 얹을 수 있게 한다.
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
        "암호화된 문서는 지원하지 않습니다. 한글에서 암호를 해제한 뒤 다시 저장하세요.",
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

/// hwpx 쪽 게이트: 커밋된 샘플의 매니페스트를 encryption-data를 담은 것으로
/// 다시 써서 재포장(mimetype은 첫 엔트리·무압축 — OPC 규약)하면 암호화 거부가
/// 뜨고, GATE-02 이전의 하류 XML 파싱 오류는 뜨지 않는다.
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
        let name = entry.name().to_string();
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
        "암호화된 문서는 지원하지 않습니다. 한글에서 암호를 해제한 뒤 다시 저장하세요.",
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

/// 여섯 거부 모두 조건 문장 뒤에 해결 힌트 문장이 따라오는지(D-08) 구조로 단언한다
/// — 힌트의 정확한 문구가 아니라 "둘째 문장이 존재한다"는 사실만 고정해, 문구 수정은
/// 허용하되 힌트 삭제는 깨지게 한다.
#[test]
fn every_refusal_carries_a_remedy_hint() {
    let dir = tmp_dir("refusal_hints");
    let cases: Vec<(PathBuf, &str)> = [
        ("pw", gate_bits::ENCRYPTED, "암호화된 문서는"),
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
        let after_condition = stderr
            .split_once("지원하지 않습니다. ")
            .map(|(_, rest)| rest.trim())
            .unwrap_or_else(|| panic!("조건 문장 종결 부재: {stderr}"));
        assert!(!after_condition.is_empty(), "해결 힌트 문장 부재: {stderr}");
    }
}
