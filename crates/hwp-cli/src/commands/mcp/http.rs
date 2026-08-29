//! HTTP adapter: Streamable-HTTP style `POST /mcp` for container deployment.
//!
//! 이 서버는 private hop이다. 신뢰된 edge(Cloudflare Worker, AgentCore runtime)가
//! TLS 종단·인증·origin 검증·body 제한을 이미 수행한 뒤에야 요청이 도달한다
//! (docs/design/22-remote-mcp-deployment.md §3.2, §4).
//!
//! stdio adapter와 같은 protocol core를 공유하므로 도구 의미론이 갈라지지 않는다.

use std::fs::File;
use std::io::{self, Cursor, Read as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use tiny_http::{Header, Method, Request, Response, Server};
use zeroize::Zeroizing;

use super::authority::{LocalFsContext, canonicalize_mcp_path};
use super::{MAX_REQUEST_LINE_BYTES, handle_request};

/// `--files` 업로드 한 건의 상한.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// workspace 전체 사용량 상한.
const MAX_WORKSPACE_BYTES: u64 = 256 * 1024 * 1024;

type Body = Response<Cursor<Vec<u8>>>;

fn plain(status: u16, body: &str) -> Body {
    Response::from_string(body).with_status_code(status)
}

fn empty(status: u16) -> Body {
    Response::from_data(Vec::new()).with_status_code(status)
}

fn header(name: &str, value: &str) -> Header {
    // 호출부가 상수만 넘기므로 실패할 수 없다.
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("정적 헤더 이름/값은 항상 유효하다")
}

/// HTTP JSON-RPC 서버. 요청을 한 번에 하나씩 처리한다.
///
/// container 하나가 MCP session 하나를 담당하므로 순차 처리가 곧 올바른 동작이다.
// ponytail: 단일 스레드 루프 — 긴 도구 호출은 /healthz까지 지연시킨다. 한 프로세스가
// 여러 session을 담당해야 하는 날이 오면 doc 22 §4의 선택지 1로 뒤집는다.
pub fn serve(
    addr: SocketAddr,
    root: PathBuf,
    font_dirs: Vec<PathBuf>,
    files: bool,
) -> anyhow::Result<()> {
    let canonical_root = canonicalize_mcp_path(&root).map_err(|error| {
        anyhow::anyhow!(
            "--root 경로를 확인할 수 없습니다: {} ({error})",
            root.display()
        )
    })?;
    if !canonical_root.is_dir() {
        anyhow::bail!(
            "--root 는 디렉터리여야 합니다: {}",
            canonical_root.display()
        );
    }
    let ctx = LocalFsContext::new(font_dirs, vec![canonical_root.clone()]);

    let server = Server::http(addr)
        .map_err(|error| anyhow::anyhow!("{addr} 에 바인드할 수 없습니다: {error}"))?;
    let bound = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow::anyhow!("수신 주소를 확인할 수 없습니다"))?;
    // 실제 바인드된 주소를 한 줄로 알린다(--addr 의 포트 0을 쓸 때 필요).
    // stdout은 도구 출력 전용이므로 운영 로그는 stderr로 보낸다.
    eprintln!("hwp serve: listening on http://{bound}");

    for mut request in server.incoming_requests() {
        let path = request.url().split('?').next().unwrap_or("").to_string();
        let method = request.method().clone();
        let result = match (&method, path.as_str()) {
            (Method::Get, "/healthz") => request.respond(plain(200, "ok")),
            (Method::Post, "/mcp") => {
                let response = handle_mcp(&mut request, &ctx);
                request.respond(response)
            }
            // server push가 없으므로 SSE stream을 제공하지 않는다.
            (_, "/mcp") => request.respond(empty(405).with_header(header("Allow", "POST"))),
            _ if files && path.starts_with("/files/") => {
                handle_files(request, &method, &path["/files/".len()..], &canonical_root)
            }
            _ => request.respond(plain(404, "not found")),
        };
        if let Err(error) = result {
            eprintln!("hwp serve: 응답 전송 실패: {error}");
        }
    }
    Ok(())
}

fn handle_mcp(request: &mut Request, ctx: &LocalFsContext) -> Body {
    if request
        .body_length()
        .is_some_and(|n| n > MAX_REQUEST_LINE_BYTES)
    {
        return plain(413, "request body too large");
    }
    let mut body = Zeroizing::new(Vec::new());
    let capped = (MAX_REQUEST_LINE_BYTES as u64).saturating_add(1);
    if request
        .as_reader()
        .take(capped)
        .read_to_end(&mut body)
        .is_err()
    {
        return plain(400, "cannot read request body");
    }
    if body.len() > MAX_REQUEST_LINE_BYTES {
        return plain(413, "request body too large");
    }
    let Ok(line) = std::str::from_utf8(&body) else {
        return plain(400, "request body is not valid UTF-8");
    };
    match handle_request(line.trim(), ctx) {
        Some(response) => {
            Response::from_string(response).with_header(header("Content-Type", "application/json"))
        }
        // 알림은 프로토콜 응답이 없다.
        None => empty(202),
    }
}

fn handle_files(
    mut request: Request,
    method: &Method,
    name: &str,
    root: &Path,
) -> Result<(), io::Error> {
    if !valid_file_name(name) {
        return request.respond(plain(400, "invalid file name"));
    }
    let target = root.join(name);
    match method {
        Method::Post => {
            let response = store_file(&mut request, &target, root);
            request.respond(response)
        }
        Method::Get => match File::open(&target) {
            Ok(file) => request.respond(
                Response::from_file(file)
                    .with_header(header("Content-Type", "application/octet-stream")),
            ),
            Err(_) => request.respond(plain(404, "not found")),
        },
        _ => request.respond(empty(405).with_header(header("Allow", "GET, POST"))),
    }
}

fn store_file(request: &mut Request, target: &Path, root: &Path) -> Body {
    let declared = request.body_length().map(|n| n as u64);
    if declared.is_some_and(|n| n > MAX_FILE_BYTES) {
        return plain(413, "file too large");
    }
    // 받기 전에 workspace 총량을 확인한다.
    // ponytail: 단일 스레드 루프라 이 사전 확인과 쓰기 사이에 경합이 없다.
    // 동시 처리를 도입하면 잠금 아래에서 다시 확인해야 한다.
    let used = workspace_bytes(root).unwrap_or(0);
    if used.saturating_add(declared.unwrap_or(0)) > MAX_WORKSPACE_BYTES {
        return plain(413, "workspace quota exceeded");
    }

    let mut file = match File::create(target) {
        Ok(file) => file,
        Err(error) => return plain(500, &format!("cannot create file: {error}")),
    };
    let capped = MAX_FILE_BYTES.saturating_add(1);
    let written = io::copy(&mut request.as_reader().take(capped), &mut file);
    drop(file);

    let discard = |message: &str| -> Body {
        let _ = std::fs::remove_file(target);
        plain(413, message)
    };
    match written {
        Err(error) => {
            let _ = std::fs::remove_file(target);
            plain(500, &format!("cannot write file: {error}"))
        }
        Ok(count) if count > MAX_FILE_BYTES => discard("file too large"),
        Ok(_) if workspace_bytes(root).unwrap_or(0) > MAX_WORKSPACE_BYTES => {
            discard("workspace quota exceeded")
        }
        Ok(_) => empty(200),
    }
}

/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`
///
/// 허용 문자에 `/`와 선행 `.`이 없으므로 `root.join(name)`은 root를 벗어날 수 없다.
///
/// percent-escape는 의도적으로 복호하지 않는다. 허용 문자 집합이 이미 URL-safe라
/// 정상적인 이름은 인코딩이 필요 없고, 복호를 하면 `%2e%2e` 같은 입력이 traversal로
/// 되살아난다. 인코딩된 이름은 `%`에서 걸려 그대로 거부된다.
fn valid_file_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn workspace_bytes(root: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        total += if meta.is_dir() {
            workspace_bytes(&entry.path()).unwrap_or(0)
        } else {
            meta.len()
        };
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::valid_file_name;

    #[test]
    fn file_names_reject_traversal_and_hidden_entries() {
        assert!(valid_file_name("a.hwpx"));
        assert!(valid_file_name("out-1_final.pdf"));
        assert!(valid_file_name("A"));

        assert!(!valid_file_name(""));
        assert!(!valid_file_name(".hidden"));
        assert!(!valid_file_name("-dash"));
        assert!(!valid_file_name(".."));
        assert!(!valid_file_name("a/b"));
        assert!(!valid_file_name("../escape"));
        assert!(!valid_file_name("한글.hwpx"));
        assert!(!valid_file_name(&"a".repeat(129)));
        assert!(valid_file_name(&"a".repeat(128)));
    }
}
