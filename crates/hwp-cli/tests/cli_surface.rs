//! CLI 표면 커버리지 — CI에서 실행되는 서브커맨드 스모크 모음.
//!
//! 기존에는 `mcp`·`diff`·`render`·PDF 경로가 로컬 픽스처 게이트라 CI 커버리지가 0이었다.
//! 이 스위트는 **커밋된 픽스처**(fixtures/samples/report-tables.hwpx)에 하드 의존해
//! 스킵 없이 동작한다(신규 의존성 0 — hwp5·serde_json 기존 의존 재사용).
//!
//! 폰트 주의: 저장소에 동봉 폰트는 없다(`/fonts/`는 gitignore). 렌더 경로의 글리프는
//! 시스템 폰트(ubuntu는 CI가 설치하는 noto-cjk, macOS는 기본 CJK)에서 온다. 그래서
//! 이 스위트의 단언은 전부 **폰트 비의존**(페이지 크기=secPr 유래, 자기-diff, PDF 구조)
//! 으로 설계돼 있다 — 글리프·페이지수 등 폰트 의존 단언을 여기 추가하지 말 것.

use std::io::{BufRead, BufReader, Write as _};
use std::path::PathBuf;
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
        "hwp_info",
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
    assert_eq!(names, expect, "도구 15종");

    send(
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name":"hwp_validate","arguments":{"path": fixture().to_string_lossy()}}}),
    );
    let call = recv("tools/call");
    assert_eq!(call["id"], 3);
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(v["valid"], true, "hwp_validate 결과: {text}");

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

/// hwp5 합성 왕복 게이트: hwpx 픽스처 → a.hwp → `convert a.hwp -o b.hwp --preserve-layout`
/// 후 **스트림 단위 바이트 동일** 단언.
///
/// 주의: 이 테스트는 로컬 전용 정품 identity 게이트(crates/hwp5/tests/identity.rs,
/// 로컬 픽스처 필요)를 대체하지 않고 **보완**한다 — 커밋 픽스처로 CI에서 도는 합성 경로
/// 왕복이다. 전체 파일 비교는 불가: cfb 크레이트가 디렉터리 엔트리에 Timestamp::now()
/// (18바이트)를 찍어 파일 단위 해시는 매번 달라진다(실측). `--preserve-layout`은
/// 줄 배치 캐시 보존 전제 — 무수정 왕복 경로를 타게 하는 필수 플래그.
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
