//! `hwp serve` HTTP adapter surface — the container-deployment contract of
//! docs/design/22-remote-mcp-deployment.md §3.2.
//!
//! Every case drives a real `hwp serve` process over a real socket. The server binds
//! `127.0.0.1:0` and prints the chosen port on its first stderr line, so the tests never
//! guess a port. Documents are produced by the server itself (`hwp_new`), so this suite
//! needs no fixture and no font, which keeps it CI-safe on all three platforms.
//!
//! The HTTP client here is a hand-rolled `TcpStream` exchange rather than a new
//! dev-dependency; a request/response pair over `Connection: close` is a few lines.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[path = "common/nested_tables.rs"]
mod nested_tables;

/// The 22 tools the MCP surface publishes; must agree with `cli_surface.rs`.
const EXPECTED_TOOLS: [&str; 22] = [
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
    "hwp_template",
    "hwp_validate",
];

/// A running server; killed on drop so a failing assertion never leaks a process.
struct Serve {
    child: Child,
    addr: String,
    root: PathBuf,
    /// Held only to keep the stderr pipe open. The server writes one line at
    /// shutdown, and printing into a closed pipe would panic the process under
    /// test rather than let it exit cleanly.
    _stderr: BufReader<std::process::ChildStderr>,
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn tmp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hwp-serve-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spawn(test: &str, files: bool) -> Serve {
    let root = tmp_dir(test);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hwp"));
    cmd.arg("serve")
        .arg("--addr")
        .arg("127.0.0.1:0")
        .arg("--root")
        .arg(&root);
    if files {
        cmd.arg("--files");
    }
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The bound address arrives on the first stderr line.
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut first = String::new();
    stderr.read_line(&mut first).unwrap();
    let addr = first
        .trim()
        .rsplit("http://")
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(!addr.is_empty(), "바인드 주소를 읽지 못했습니다: {first:?}");

    Serve {
        child,
        addr,
        root,
        _stderr: stderr,
    }
}

/// Minimal HTTP/1.1 exchange. Returns (status, body).
fn request(addr: &str, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let (status, _, body) = request_full(addr, method, path, body);
    (status, body)
}

/// Same, but also returns the raw header block so tests can assert on `Allow`.
fn request_full(addr: &str, method: &str, path: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("헤더와 본문 경계를 찾지 못했습니다");
    let headers = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("상태 줄을 해석하지 못했습니다");
    // Every response uses identity framing (issue #312 D2): it always carries
    // `Content-Length` and `Connection: close`, and is never chunked.
    assert!(
        !headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked")),
        "응답이 chunked로 왔습니다: {headers}"
    );
    let body = raw[split + 4..].to_vec();
    (status, headers, body)
}

fn rpc(addr: &str, payload: &str) -> (u16, serde_json::Value) {
    let (status, body) = request(addr, "POST", "/mcp", payload.as_bytes());
    let value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).expect("JSON 응답을 해석하지 못했습니다")
    };
    (status, value)
}

fn call_tool(addr: &str, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 9, "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    });
    let (status, value) = rpc(addr, &payload.to_string());
    assert_eq!(status, 200, "{name} 호출이 200이 아닙니다");
    value["result"].clone()
}

#[test]
fn serve_speaks_the_same_protocol_as_stdio() {
    let server = spawn("session", false);
    let addr = &server.addr;

    let (status, body) = request(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200);
    assert_eq!(body, b"ok");

    let (status, value) = rpc(
        addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
    );
    assert_eq!(status, 200);
    assert_eq!(value["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(value["result"]["serverInfo"]["name"], "hwp-cli");

    // Notifications have no protocol response.
    let (status, body) = request(
        addr,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert_eq!(status, 202);
    assert!(body.is_empty(), "알림 응답 본문이 비어야 합니다");

    let (status, value) = rpc(addr, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    assert_eq!(status, 200);
    let mut names: Vec<&str> = value["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, EXPECTED_TOOLS, "도구 20종");
}

#[test]
fn serve_runs_tools_inside_the_root() {
    let server = spawn("tools", false);
    let addr = &server.addr;
    let made = server.root.join("made.hwpx");

    let result = call_tool(
        addr,
        "hwp_new",
        serde_json::json!({"output": made.to_str().unwrap(), "markdown": "# 제목\n\n본문."}),
    );
    assert_eq!(result["isError"], false, "hwp_new: {result}");
    assert!(made.exists(), "문서가 생성되지 않았습니다");

    let result = call_tool(
        addr,
        "hwp_info",
        serde_json::json!({"path": made.to_str().unwrap()}),
    );
    assert_eq!(result["isError"], false, "hwp_info: {result}");

    // Writes outside --root are refused, the same as over stdio.
    let outside = std::env::temp_dir().join("hwp-serve-escape.hwpx");
    let result = call_tool(
        addr,
        "hwp_new",
        serde_json::json!({"output": outside.to_str().unwrap(), "markdown": "x"}),
    );
    assert_eq!(result["isError"], true, "root 밖 쓰기가 허용되었습니다");
    assert!(!outside.exists(), "root 밖에 파일이 생겼습니다");
}

#[test]
fn serve_rejects_oversized_and_unsupported_requests() {
    let server = spawn("limits", false);
    let addr = &server.addr;

    // A body over 1 MiB is refused before parsing.
    let oversized = vec![b'x'; 1024 * 1024 + 1];
    let (status, _) = request(addr, "POST", "/mcp", &oversized);
    assert_eq!(status, 413);

    // No server push, so no SSE stream.
    let (status, headers, _) = request_full(addr, "GET", "/mcp", b"");
    assert_eq!(status, 405);
    assert!(
        headers.to_ascii_lowercase().contains("allow: post"),
        "405 응답에 Allow 헤더가 없습니다: {headers}"
    );

    let (status, _) = request(addr, "GET", "/nope", b"");
    assert_eq!(status, 404);

    // Without --files the file routes do not exist.
    let (status, _) = request(addr, "POST", "/files/a.bin", b"data");
    assert_eq!(status, 404);
}

#[test]
fn serve_files_roundtrip_and_name_rules() {
    let server = spawn("files", true);
    let addr = &server.addr;

    let (status, _) = request(addr, "POST", "/files/a.bin", b"hello-hwp");
    assert_eq!(status, 200);
    let (status, body) = request(addr, "GET", "/files/a.bin", b"");
    assert_eq!(status, 200);
    assert_eq!(body, b"hello-hwp");

    let (status, _) = request(addr, "GET", "/files/missing.bin", b"");
    assert_eq!(status, 404);

    // The allowed set has no `/` and no leading `.`, so traversal cannot form.
    for name in [".hidden", "-dash", "%2e%2e", "%ED%95%9C.hwpx"] {
        let (status, _) = request(addr, "POST", &format!("/files/{name}"), b"data");
        assert_eq!(status, 400, "이름 {name} 이 거부되지 않았습니다");
    }

    let entries: Vec<_> = std::fs::read_dir(&server.root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["a.bin"], "workspace에 예상 밖 파일이 있습니다");

    // A directory opens on Unix but has no body to send: answer 404 instead
    // of promising its metadata length under 200 and cutting the body.
    std::fs::create_dir(server.root.join("sub")).unwrap();
    let (status, _) = request(addr, "GET", "/files/sub", b"");
    assert_eq!(status, 404, "디렉터리가 파일처럼 응답되었습니다");
}

/// `store_file` answers 413 from the declared Content-Length alone, before it
/// opens the body — so the per-file cap is testable without a 64 MiB upload.
/// The workspace cap is not reachable this way: it is computed from actual
/// on-disk usage (`workspace_bytes`), so tripping it would require writing
/// ~256 MiB for real. Per-file cap only.
#[test]
fn serve_files_caps() {
    let server = spawn("files-caps", true);
    let addr = &server.addr;

    // Declare MAX_FILE_BYTES (64 MiB) + 1 as Content-Length and send no body.
    let mut stream = TcpStream::connect(addr).unwrap();
    let head = format!(
        "POST /files/too-big.bin HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        64 * 1024 * 1024 + 1
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.flush().unwrap();
    // No half-close. The server refuses on the declared length alone, so the
    // 413 arrives without EOF and linger discards the rest within its caps (D6).
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let (status, _) = read_response(&stream).unwrap();
    assert_eq!(status, 413, "선언 길이 초과 업로드가 거부되지 않았습니다");
    assert!(
        !server.root.join("too-big.bin").exists(),
        "거부된 업로드가 workspace에 남았습니다"
    );

    // The server must keep working after the refusal.
    let (status, _) = request(addr, "POST", "/files/ok.bin", b"fine");
    assert_eq!(status, 200);
    assert_eq!(std::fs::read(server.root.join("ok.bin")).unwrap(), b"fine");
}

/// Reads one response without waiting for connection close: header block, then
/// exactly `Content-Length` body bytes. Reject responses are always identity-
/// framed, and the caller's read timeout bounds a wedged server.
fn read_response(stream: &TcpStream) -> std::io::Result<(u16, Vec<u8>)> {
    let mut reader = BufReader::new(stream);
    let mut status = None;
    let mut content_length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if status.is_none() {
            status = trimmed
                .split_whitespace()
                .nth(1)
                .and_then(|c| c.parse().ok());
        } else if let Some(length) = trimmed
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse().ok())
        {
            content_length = Some(length);
        }
    }
    let mut body = vec![0; content_length.unwrap_or(0)];
    reader.read_exact(&mut body)?;
    Ok((status.unwrap_or(0), body))
}

/// Regression for #310 and the abort range of issue #312. The reject response must
/// go out and the loop must keep answering — with no half-close from the client to
/// end any drain. Extreme declared lengths cover both allocation paths: 2^50 was the
/// verified `memory allocation failed` abort on main (uncatchable, exit 134), 2^63
/// the catchable `capacity overflow` path, and `u64::MAX` the original #310 probe.
#[test]
fn serve_survives_declared_length_stall() {
    let server = spawn("stall", false);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);

    // Declare 64 MiB + 1, send nothing and keep the socket open, with no
    // half-close. The server refuses with 413 on the declared length alone, so
    // only the waiting client's connection thread is involved and the accept
    // loop keeps answering.
    let mut staller = TcpStream::connect(addr).unwrap();
    staller.set_read_timeout(Some(timeout)).unwrap();
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\n\r\n",
        64 * 1024 * 1024 + 1
    );
    staller.write_all(head.as_bytes()).unwrap();
    staller.flush().unwrap();
    let (status, _) = read_response(&staller).unwrap();
    assert_eq!(status, 413, "stalled 요청의 413이 도착하지 않았습니다");

    // The accept loop must stay alive: a new connection is answered in bounded
    // time. The client read timeout is the bound, so a wedged server fails the
    // test instead of hanging it.
    let mut follow = TcpStream::connect(addr).unwrap();
    follow.set_read_timeout(Some(timeout)).unwrap();
    follow
        .write_all(
            format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    follow.flush().unwrap();
    let (status, body) = read_response(&follow).unwrap();
    assert_eq!(status, 200, "stall 뒤 healthz가 응답하지 않았습니다");
    assert_eq!(body, b"ok");
}

/// AC1 (issue #312). Extreme declared lengths — 2^50, 2^63 and `u64::MAX` — each
/// with a 3-byte body and a half-close, all get 413, and the server answers
/// `/healthz` afterwards. On main, 2^50 aborted the process with `memory
/// allocation of 1125899906842624 bytes failed` (exit 134, uncatchable).
#[test]
fn serve_survives_extreme_declared_lengths() {
    let server = spawn("extreme", false);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);

    for length in [1u64 << 50, 1u64 << 63, u64::MAX] {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(timeout)).unwrap();
        let head =
            format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {length}\r\n\r\n",);
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(b"abc").unwrap();
        stream.flush().unwrap();
        // Half-close: the body is complete. The server refuses a declared length
        // over 1 MiB before reading it.
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let (status, body) = read_response(&stream).unwrap();
        assert_eq!(status, 413, "선언 길이 {length} 가 거부되지 않았습니다");
        assert_eq!(
            body, b"request body too large",
            "선언 길이 {length} 의 본문"
        );
    }

    let (status, body) = request(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200, "극단 선언 뒤 서버가 죽었습니다");
    assert_eq!(body, b"ok");
}

/// AC2 (issue #312). A request whose header block exceeds the 16 KiB head cap gets
/// 431, and the server stays up. On main, an unbounded header line buffer let a
/// 64 MiB header line push tiny_http to 70 MB RSS.
#[test]
fn serve_rejects_an_oversized_head_with_431() {
    let server = spawn("head-cap", false);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);

    // One header line over 16 KiB. The server scans chunk by chunk, holds at
    // most one buffer beyond the cap, and answers 431 there (D4).
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(timeout)).unwrap();
    let header = format!("X-Pad: {}\r\n", "x".repeat(64 * 1024));
    stream
        .write_all(format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\n{header}\r\n").as_bytes())
        .unwrap();
    stream.flush().unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let (status, _) = read_response(&stream).unwrap();
    assert_eq!(status, 431, "16 KiB 초과 헤드가 거부되지 않았습니다");

    let (status, body) = request(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200, "헤드 초과 뒤 서버가 죽었습니다");
    assert_eq!(body, b"ok");
}

/// AC3 (issue #312). `Transfer-Encoding: chunked` gets 411, disagreeing
/// `Content-Length` fields get 400, and a non-decimal value gets 400 — including
/// `+7`, which `u64::from_str` would otherwise accept.
#[test]
fn serve_rejects_non_content_length_framing() {
    let server = spawn("framing", false);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);

    let exchanges: [(&str, u16); 4] = [
        (
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            411,
        ),
        (
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 5\r\nContent-Length: 7\r\n\r\n",
            400,
        ),
        (
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 0x10\r\n\r\n",
            400,
        ),
        (
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Length: +7\r\n\r\n",
            400,
        ),
    ];
    for (head, expected) in exchanges {
        let head = head.replace("{addr}", addr);
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(timeout)).unwrap();
        stream.write_all(head.as_bytes()).unwrap();
        stream.flush().unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let (status, _) = read_response(&stream).unwrap();
        assert_eq!(status, expected, "요청 헤드: {head:?}");
    }

    let (status, body) = request(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200, "framing 거부 뒤 서버가 죽었습니다");
    assert_eq!(body, b"ok");
}

/// AC4 (issue #312). An upload with `Expect: 100-continue` receives the interim
/// 100, then 200, and the stored content matches — with no half-close: the client
/// waits for the interim response before sending the body, which is the point of
/// the expectation.
#[test]
fn serve_files_expect_continue_roundtrip() {
    let server = spawn("expect-continue", true);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);
    let payload = b"expect-continue-body";

    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(timeout)).unwrap();
    stream
        .write_all(
            format!(
                "POST /files/expect.bin HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nExpect: 100-continue\r\n\r\n",
                payload.len()
            )
            .as_bytes(),
        )
        .unwrap();
    stream.flush().unwrap();

    // The interim 100 arrives first.
    let interim = read_response(&stream).unwrap();
    assert_eq!(interim.0, 100, "interim 100이 오지 않았습니다");

    // Only then send the body.
    stream.write_all(payload).unwrap();
    stream.flush().unwrap();
    let (status, _) = read_response(&stream).unwrap();
    assert_eq!(status, 200, "100-continue 업로드가 완료되지 않았습니다");
    assert_eq!(
        std::fs::read(server.root.join("expect.bin")).unwrap(),
        payload,
        "저장된 내용이 다릅니다"
    );
}

/// AC5 (issue #312). Every response carries `Connection: close` and
/// `Content-Length`, and a second request pipelined on the same connection is not
/// answered — one request per connection (D2).
#[test]
fn serve_sends_close_per_request_and_ignores_pipelining() {
    let server = spawn("close", false);
    let addr = &server.addr;

    // The /healthz response carries both framing headers.
    let (status, headers, _) = request_full(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200);
    let lowered = headers.to_ascii_lowercase();
    assert!(lowered.contains("connection: close"), "{headers}");
    assert!(lowered.contains("content-length: 2"), "{headers}");
    assert!(
        lowered.contains("content-type: text/plain; charset=utf-8"),
        "{headers}"
    );

    // A second request written on the same connection is not answered: the
    // server closes the connection, so the client sees EOF.
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .write_all(
            format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    stream
        .write_all(
            format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    stream.flush().unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw);
    assert_eq!(
        text.matches("HTTP/1.1 200 OK").count(),
        1,
        "두 번째 요청까지 답했습니다: {text:?}"
    );
}

/// AC6 (issue #312). While one connection holds an incomplete request head, a
/// `/healthz` request on another connection returns within 1 s — no shared
/// blocking.
#[test]
fn serve_answers_healthz_while_another_head_is_incomplete() {
    let server = spawn("incomplete-head", false);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);

    // Send half a head, with no empty line, and keep the socket open.
    let mut holder = TcpStream::connect(addr).unwrap();
    holder.set_read_timeout(Some(timeout)).unwrap();
    holder
        .write_all(b"POST /mcp HTTP/1.1\r\nContent-Length: 5\r\n")
        .unwrap();
    holder.flush().unwrap();

    let start = std::time::Instant::now();
    let (status, body) = request(addr, "GET", "/healthz", b"");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "healthz가 1초 안에 오지 않았습니다"
    );
    assert_eq!(status, 200);
    assert_eq!(body, b"ok");
}

#[test]
fn serve_refuses_to_start_without_a_usable_root() {
    let missing_root = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(["serve", "--addr", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert!(
        !missing_root.status.success(),
        "--root 없이 기동이 성공했습니다"
    );

    let absent = std::env::temp_dir().join(format!("hwp-serve-absent-{}", std::process::id()));
    assert!(!Path::new(&absent).exists());
    let bad_root = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(["serve", "--addr", "127.0.0.1:0", "--root"])
        .arg(&absent)
        .output()
        .unwrap();
    assert!(
        !bad_root.status.success(),
        "존재하지 않는 --root 로 기동이 성공했습니다"
    );
}

/// A container platform stops an idle instance with SIGTERM, and the kernel does not
/// deliver a signal with its default disposition to PID 1 — which `hwp serve` becomes
/// in a container. Without a handler the signal is discarded and the process runs
/// forever; that is exactly how nine Cloudflare containers stayed up for hours.
#[test]
#[cfg(unix)]
fn serve_exits_on_sigterm() {
    let mut server = spawn("sigterm", false);
    // Prove it is serving first, so a pass cannot mean "it never started".
    let (status, _) = request(&server.addr, "GET", "/healthz", b"");
    assert_eq!(status, 200);

    // SAFETY: the pid belongs to a child this test spawned and has not reaped.
    unsafe { libc::kill(server.child.id() as libc::pid_t, libc::SIGTERM) };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let exit = loop {
        if let Some(exit) = server.child.try_wait().unwrap() {
            break exit;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "SIGTERM 후 10초가 지나도 종료되지 않았습니다"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(exit.success(), "정상 종료가 아닙니다: {exit:?}");
}

/// Inline transfer over HTTP: the flow Tier B has to use, where `/mcp` is the entire
/// contract and no `/files` sideband exists. Proves a document can enter and leave a
/// session without any route but this one.
#[test]
fn serve_inline_transfer_roundtrip() {
    let server = spawn("inline", false);

    // Build a real document inside the workspace, then read its bytes back out so the
    // test has something with genuine structure to send, not a synthetic blob.
    let made = call_tool(
        &server.addr,
        "hwp_new",
        serde_json::json!({
            "output": server.root.join("made.hwpx").to_string_lossy(),
            "markdown": "# 인라인 전송\n\n본문.",
        }),
    );
    assert_eq!(made["isError"], false, "{made}");
    let original = std::fs::read(server.root.join("made.hwpx")).unwrap();

    let fetched = call_tool(
        &server.addr,
        "hwp_get_file",
        serde_json::json!({"path": server.root.join("made.hwpx").to_string_lossy()}),
    );
    assert_eq!(fetched["isError"], false, "{fetched}");
    let blob = fetched["content"][1]["resource"]["blob"].as_str().unwrap();

    // Round the bytes back in under a different name, then confirm the copy is usable
    // by a normal path-taking tool — the doc 22 §7 model, where inline transfer feeds
    // the workspace rather than replacing path arguments.
    let put = call_tool(
        &server.addr,
        "hwp_put_file",
        serde_json::json!({
            "name": server.root.join("copy.hwpx").to_string_lossy(),
            "content": blob,
        }),
    );
    assert_eq!(put["isError"], false, "{put}");
    assert_eq!(
        std::fs::read(server.root.join("copy.hwpx")).unwrap(),
        original,
        "HTTP 왕복 후 바이트가 달라졌습니다"
    );

    let info = call_tool(
        &server.addr,
        "hwp_info",
        serde_json::json!({"path": server.root.join("copy.hwpx").to_string_lossy()}),
    );
    assert_eq!(
        info["isError"], false,
        "왕복한 문서를 읽지 못했습니다: {info}"
    );
}

/// Regression for the hand-rolled framing (issue #312 review): the body may
/// arrive in a separate write well after the head, and the client does not
/// half-close — it sends exactly Content-Length bytes and waits. The adapter
/// must not wait for one more byte (tiny_http's EqualReader stopped at the
/// declared length; a raw stream does not EOF until the client closes).
#[test]
fn serve_mcp_accepts_a_body_written_after_the_head() {
    let server = spawn("delayed-mcp", false);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);
    let payload = r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#;

    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(timeout)).unwrap();
    stream
        .write_all(
            format!(
                "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\n\r\n",
                payload.len()
            )
            .as_bytes(),
        )
        .unwrap();
    stream.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    stream.write_all(payload.as_bytes()).unwrap();
    stream.flush().unwrap();

    let (status, body) = read_response(&stream).unwrap();
    assert_eq!(status, 200, "지연된 본문이 처리되지 않았습니다");
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(value["result"]["tools"].is_array(), "{value}");
}

/// Same as above, for `POST /files`: the upload lands intact even when the
/// body follows the head by 300 ms with no half-close.
#[test]
fn serve_files_accepts_a_body_written_after_the_head() {
    let server = spawn("delayed-files", true);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);
    let payload = b"delayed-body-roundtrip";

    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(timeout)).unwrap();
    stream
        .write_all(
            format!(
                "POST /files/delayed.bin HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\n\r\n",
                payload.len()
            )
            .as_bytes(),
        )
        .unwrap();
    stream.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    stream.write_all(payload).unwrap();
    stream.flush().unwrap();

    let (status, _) = read_response(&stream).unwrap();
    assert_eq!(status, 200, "지연된 업로드가 완료되지 않았습니다");
    assert_eq!(
        std::fs::read(server.root.join("delayed.bin")).unwrap(),
        payload,
        "저장된 내용이 다릅니다"
    );
}

/// A short body with the client half-closing its sending side is a protocol
/// error, not a truncated success: `/mcp` and `/files` answer 400 and no
/// partial file survives.
#[test]
fn serve_rejects_a_short_body_with_400() {
    let server = spawn("short-body", true);
    let addr = &server.addr;
    let timeout = std::time::Duration::from_secs(5);

    for (path, declared) in [("/mcp", 64u64), ("/files/short.bin", 64u64)] {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(timeout)).unwrap();
        stream
            .write_all(
                format!(
                    "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {declared}\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        // Send less than declared and half-close: the body ends here.
        stream.write_all(b"abc").unwrap();
        stream.flush().unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let (status, body) = read_response(&stream).unwrap();
        assert_eq!(status, 400, "{path} 의 짧은 본문이 거부되지 않았습니다");
        assert_eq!(
            body, b"request body shorter than declared",
            "{path} 의 본문"
        );
    }
    assert!(
        !server.root.join("short.bin").exists(),
        "짧은 업로드의 잔여 파일이 남았습니다"
    );

    let (status, body) = request(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200, "짧은 본문 거부 뒤 서버가 죽었습니다");
    assert_eq!(body, b"ok");
}

/// A large `GET /files` response must arrive byte-exact — about 8 MiB, past
/// every socket and copy buffer boundary. Guards against truncated responses.
#[test]
fn serve_files_get_delivers_a_large_file_byte_exact() {
    let server = spawn("large-get", true);
    let addr = &server.addr;

    // 8 MiB + 17 bytes of a deterministic pattern.
    let total = 8 * 1024 * 1024 + 17;
    let payload: Vec<u8> = (0..total).map(|i| (i % 251) as u8).collect();
    let (status, _) = request(addr, "POST", "/files/big.bin", &payload);
    assert_eq!(status, 200, "대용량 업로드가 실패했습니다");

    let (status, body) = request(addr, "GET", "/files/big.bin", b"");
    assert_eq!(status, 200);
    assert_eq!(body.len(), payload.len(), "응답이 잘렸습니다");
    assert_eq!(body, payload, "응답 바이트가 다릅니다");
}

/// A document nested past the section parser's depth bound (#317) is refused
/// with a tool error, and the server keeps answering. 200 nested tables once
/// overflowed a 2 MiB connection thread and aborted the whole process
/// (SIGABRT, uncatchable); connection threads now also run on `main`'s 32 MiB
/// stack, but the bound is what keeps the parser from recursing that deep.
#[test]
fn serve_refuses_deeply_nested_tables_and_stays_up() {
    let server = spawn("nested", false);
    let addr = &server.addr;
    let flat = server.root.join("flat.hwpx");
    let result = call_tool(
        addr,
        "hwp_new",
        serde_json::json!({
            "output": flat.to_str().unwrap(),
            "markdown": nested_tables::TABLE_MARKDOWN,
        }),
    );
    assert_eq!(result["isError"], false, "hwp_new: {result}");

    let deep = server.root.join("deep.hwpx");
    nested_tables::write_nested_tables(&flat, 200, &deep);
    let result = call_tool(
        addr,
        "hwp_read",
        serde_json::json!({"path": deep.to_str().unwrap()}),
    );
    assert_eq!(
        result["isError"], true,
        "깊은 중첩이 거부되지 않았습니다: {result}"
    );
    assert!(result.to_string().contains("256"), "{result}");
    let (status, _) = request(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200, "깊은 중첩 표 뒤 서버가 죽었습니다");
}

/// A client that declares a `/mcp` body and then stalls parks only its own
/// connection thread: the body is read before the dispatch lock, so another
/// request is answered at once instead of after the 30 s read timeout.
#[test]
fn serve_answers_mcp_while_another_body_stalls() {
    let server = spawn("body-stall", false);
    let addr = &server.addr;

    let mut staller = TcpStream::connect(addr).unwrap();
    staller
        .write_all(
            format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 50\r\n\r\n").as_bytes(),
        )
        .unwrap();
    staller.flush().unwrap();
    // Give the server time to take the head and start reading the body.
    std::thread::sleep(std::time::Duration::from_millis(200));

    let start = std::time::Instant::now();
    let (status, value) = rpc(addr, r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
    assert_eq!(status, 200);
    assert!(value["result"]["tools"].is_array(), "{value}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "멈춘 본문이 다른 /mcp 요청을 {:?} 막았습니다",
        start.elapsed()
    );
    drop(staller);
}

/// A connect-and-close with no bytes (a TCP readiness probe) is not a request:
/// the server closes without writing a response.
#[test]
fn serve_closes_an_empty_connection_without_answering() {
    let server = spawn("probe", false);
    let addr = &server.addr;

    let mut probe = TcpStream::connect(addr).unwrap();
    probe.shutdown(std::net::Shutdown::Write).unwrap();
    probe
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut raw = Vec::new();
    probe.read_to_end(&mut raw).unwrap();
    assert!(
        raw.is_empty(),
        "빈 연결에 응답했습니다: {:?}",
        String::from_utf8_lossy(&raw)
    );

    let (status, _) = request(addr, "GET", "/healthz", b"");
    assert_eq!(status, 200);
}
