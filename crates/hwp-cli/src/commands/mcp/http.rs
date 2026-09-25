//! HTTP adapter: Streamable-HTTP style `POST /mcp` for container deployment.
//!
//! This server is a private hop. A trusted edge (Cloudflare Worker, AgentCore
//! runtime) terminates TLS and performs auth, origin checks and body limits
//! before a request ever arrives (docs/design/22-remote-mcp-deployment.md
//! §3.2, §4).
//!
//! It shares the same protocol core as the stdio adapter, so tool semantics
//! cannot drift apart.
//!
//! Every request gets its own connection, closed after the response (issue
//! #312 D2): every response carries `Connection: close` and an exact
//! `Content-Length`; there is no keep-alive and no pipelining. That removes
//! the drain / desync / idle-timeout classes of bugs instead of bounding them.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use zeroize::Zeroizing;

use super::authority::{FileAuthority, LocalFsContext, canonicalize_mcp_path};
use super::wire::{Head, Reject, linger, read_head, write_continue, write_response};
use super::{MAX_REQUEST_LINE_BYTES, handle_request};

/// Cap on a single `--files` upload.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Cap on total workspace usage.
const MAX_WORKSPACE_BYTES: u64 = 256 * 1024 * 1024;
/// Cap on live connection threads (D5). Excess connections are closed without
/// a response.
const MAX_CONNECTIONS: usize = 32;
/// Socket read/write timeout (D5).
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Set by the signal handler, the only writer. Means a shutdown was requested.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Shutdown poll interval; also the worst-case delay before a signal is seen.
const SHUTDOWN_POLL: Duration = Duration::from_millis(200);

/// Signal handler. Performs only async-signal-safe operations.
///
/// The first signal requests a graceful shutdown, so in-flight requests run to
/// completion. The second is treated as an immediate exit, because a long tool
/// call must not trap the process.
#[cfg(unix)]
extern "C" fn on_terminate(_signal: libc::c_int) {
    if SHUTDOWN.swap(true, Ordering::SeqCst) {
        // SAFETY: `_exit` is async-signal-safe. It skips atexit handlers, but
        // this server holds no state that must be flushed at exit.
        unsafe { libc::_exit(130) };
    }
}

/// Registers SIGTERM/SIGINT so the accept loop can break out.
///
/// In a container this process usually runs as PID 1, and the kernel does not
/// deliver default-disposition signals to PID 1. Without a handler SIGTERM is
/// silently discarded, and a platform that stops idle containers that way can
/// never stop this process. This was why nine containers on Cloudflare
/// Containers stayed up for about four hours.
#[cfg(unix)]
fn install_signal_handlers() {
    // SAFETY: the handler performs only async-signal-safe operations (see the
    // comment above), and no other threads exist at this point to race
    // libc::signal itself.
    unsafe {
        let handler = on_terminate as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
}

/// Windows has no equivalent signals. Console close keeps its existing
/// behavior.
#[cfg(not(unix))]
fn install_signal_handlers() {}

/// HTTP JSON-RPC server.
///
/// One container serves one MCP session. `/mcp` and `/files` are handled one
/// at a time under a single dispatch lock (doc 22 §3.2). `/healthz` answers
/// without the lock so a long tool call never delays readiness.
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

    let listener = TcpListener::bind(addr)
        .map_err(|error| anyhow::anyhow!("{addr} 에 바인드할 수 없습니다: {error}"))?;
    let bound = listener
        .local_addr()
        .map_err(|error| anyhow::anyhow!("수신 주소를 확인할 수 없습니다: {error}"))?;
    // Announce the actually bound address (needed with --addr port 0).
    // stdout is reserved for tool output, so operational logs go to stderr.
    eprintln!("hwp serve: listening on http://{bound}");

    install_signal_handlers();

    let dispatch = Arc::new(Mutex::new(()));
    // A blocking accept(2) cannot notice the shutdown signal, so a dedicated
    // thread accepts while this thread polls SHUTDOWN. The listener stays
    // blocking on purpose: on BSD-derived systems accept(2) gives the new
    // socket "the same properties of socket", and std does not clear the flag,
    // so a non-blocking listener would hand every connection socket to its
    // handler in non-blocking mode and break read/write timeouts there.
    {
        let dispatch = Arc::clone(&dispatch);
        thread::spawn(move || accept_loop(listener, Arc::new(ctx), files, &dispatch));
    }

    while !SHUTDOWN.load(Ordering::SeqCst) {
        thread::sleep(SHUTDOWN_POLL);
    }
    // Wait for the in-flight dispatch to release the lock (D8). The accept
    // thread is left blocked in accept(2); process exit reaps it.
    let guard = dispatch
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Hold the lock until the process exits: a connection thread queued on it
    // would otherwise take it once `serve` returns and start a tool call or an
    // upload that exit then cuts off mid-write.
    std::mem::forget(guard);
    eprintln!("hwp serve: shutting down");
    Ok(())
}

/// Accepts connections and spawns one thread per connection.
fn accept_loop(
    listener: TcpListener,
    ctx: Arc<LocalFsContext>,
    files: bool,
    dispatch: &Arc<Mutex<()>>,
) {
    let live = Arc::new(AtomicUsize::new(0));
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if SHUTDOWN.load(Ordering::SeqCst) {
                    drop(stream);
                    break;
                }
                if live.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    // Connection cap (D5): close without answering.
                    drop(stream);
                    continue;
                }
                live.fetch_add(1, Ordering::SeqCst);
                let ctx = Arc::clone(&ctx);
                let dispatch = Arc::clone(dispatch);
                let slot = ConnectionSlot(Arc::clone(&live));
                // Tool calls run on this thread, so it gets `main`'s stack.
                let spawned = thread::Builder::new()
                    .stack_size(crate::WORKER_STACK_BYTES)
                    .spawn(move || {
                        let _slot = slot;
                        handle_connection(stream, &ctx, files, &dispatch);
                    });
                // On failure the closure drops: the connection closes and the
                // slot returns. The accept thread must not panic here.
                if let Err(error) = spawned {
                    eprintln!("hwp serve: 연결 스레드를 만들 수 없습니다: {error}");
                }
            }
            Err(error) => {
                if SHUTDOWN.load(Ordering::SeqCst) {
                    break;
                }
                eprintln!("hwp serve: 연결 수신 실패: {error}");
                // A persistent error (EMFILE) would otherwise spin this loop.
                thread::sleep(SHUTDOWN_POLL);
            }
        }
    }
}

/// Returns the connection slot to the pool even when the handler panics, so 32
/// panics cannot exhaust `MAX_CONNECTIONS` and wedge the process into dropping
/// every new connection, `/healthz` included.
struct ConnectionSlot(Arc<AtomicUsize>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn handle_connection(stream: TcpStream, ctx: &LocalFsContext, files: bool, dispatch: &Mutex<()>) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };
    let mut reader = BufReader::with_capacity(8192, stream);
    // A connect-and-close with no bytes (a TCP readiness probe) or an idle
    // timeout is not a request: close without an answer or a log line.
    if !matches!(reader.fill_buf(), Ok(bytes) if !bytes.is_empty()) {
        return;
    }

    let head = match read_head(&mut reader) {
        Ok(head) => head,
        Err(reject) => {
            answer_reject(&mut writer, reject);
            return;
        }
    };

    route_request(&head, reader, &mut writer, ctx, files, dispatch);
}

/// Route matching keeps the existing surface: `/healthz`, `POST /mcp`, other
/// methods on `/mcp` (405 `Allow: POST`), `/files/{name}` when `--files` is on
/// (405 `Allow: GET, POST`), and 404.
fn route_request(
    head: &Head,
    reader: BufReader<TcpStream>,
    writer: &mut TcpStream,
    ctx: &LocalFsContext,
    files: bool,
    dispatch: &Mutex<()>,
) {
    // Handlers read the body only through this `take`, so its limit afterwards
    // is the declared remainder they did not consume. Part of it may already
    // sit in the BufReader rather than on the socket; linger then waits for
    // bytes that never come, which its 2 s deadline bounds.
    let mut body = reader.take(head.content_length);
    let path = head.path.as_str();
    let result = match (head.method.as_str(), path) {
        ("GET", "/healthz") => write_plain(writer, 200, "ok"),
        ("POST", "/mcp") => handle_mcp(head, &mut body, writer, ctx, dispatch),
        // No server push, so no SSE stream.
        (_, "/mcp") => write_response(writer, 405, &[("Allow", "POST")], &mut io::empty(), 0),
        _ if files && path.starts_with("/files/") => {
            let _guard = lock_dispatch(dispatch);
            handle_files(head, &mut body, writer, &path["/files/".len()..], ctx)
        }
        _ => write_plain(writer, 404, "not found"),
    };
    if let Err(error) = result {
        eprintln!("hwp serve: 응답 전송 실패: {error}");
        return;
    }
    // Discard whatever the handler left unread, bounded (D6).
    linger(writer, body.limit());
}

/// Writes one text body as the response.
fn write_plain(writer: &mut TcpStream, status: u16, message: &str) -> io::Result<()> {
    let len = message.len() as u64;
    let mut body: &[u8] = message.as_bytes();
    write_response(
        writer,
        status,
        &[("Content-Type", "text/plain; charset=utf-8")],
        &mut body,
        len,
    )
}

/// The dispatch lock (D5). The main thread waits on the same lock at shutdown.
fn lock_dispatch(dispatch: &Mutex<()>) -> impl Drop + '_ {
    dispatch
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn answer_reject(writer: &mut TcpStream, reject: Reject) {
    let status = reject.status;
    let message = match status {
        411 => "length required",
        431 => "request header fields too large",
        _ => "bad request",
    };
    if let Err(error) = write_plain(writer, status, message) {
        eprintln!("hwp serve: 거부 응답 전송 실패: {error}");
        return;
    }
    // A Reject path never read the body, so close gracefully (D6). Body bytes
    // that followed the head may still be unread; discard up to the cap only.
    linger(writer, u64::MAX);
}

/// Handles one `/mcp` body. `reader` stops at the declared length, so a
/// well-behaved client that sends its body without half-closing and waits for
/// the status still gets its response (AC4).
///
/// The body is read before the dispatch lock: a client that stalls its body
/// parks only its own connection thread, not every `/mcp` and `/files`
/// request. The lock then covers the tool call and the response write, so
/// shutdown still waits for an in-flight call to answer (D8).
fn handle_mcp(
    head: &Head,
    reader: &mut impl Read,
    writer: &mut TcpStream,
    ctx: &LocalFsContext,
    dispatch: &Mutex<()>,
) -> io::Result<()> {
    if head.content_length > MAX_REQUEST_LINE_BYTES as u64 {
        return write_plain(writer, 413, "request body too large");
    }
    if head.expect_continue {
        write_continue(writer)?;
    }
    let mut body = Zeroizing::new(Vec::new());
    let read = reader.read_to_end(&mut body);
    match read {
        Err(_) => return write_plain(writer, 400, "cannot read request body"),
        Ok(_) if (body.len() as u64) < head.content_length => {
            return write_plain(writer, 400, "request body shorter than declared");
        }
        Ok(_) => {}
    }
    let Ok(line) = std::str::from_utf8(&body) else {
        return write_plain(writer, 400, "request body is not valid UTF-8");
    };
    let _guard = lock_dispatch(dispatch);
    // The protocol core keeps no state between calls, so a tool that panics
    // leaves nothing half-updated: answer 500 instead of resetting the
    // connection. A stack overflow aborts regardless; see the thread stack.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handle_request(line.trim(), ctx)
    }));
    let Ok(outcome) = outcome else {
        return write_plain(writer, 500, "internal error");
    };
    let Some(response) = outcome else {
        // Notifications have no protocol response.
        return write_response(writer, 202, &[], &mut io::empty(), 0);
    };
    let len = response.len() as u64;
    let mut response = response.as_bytes();
    write_response(
        writer,
        200,
        &[("Content-Type", "application/json")],
        &mut response,
        len,
    )
}

fn handle_files(
    head: &Head,
    reader: &mut impl Read,
    writer: &mut TcpStream,
    name: &str,
    ctx: &LocalFsContext,
) -> io::Result<()> {
    if !valid_file_name(name) {
        return write_plain(writer, 400, "invalid file name");
    }
    let root = ctx.roots().first().expect("--root 는 항상 설정된다");
    let target = root.join(name);
    match head.method.as_str() {
        "POST" => store_file(head, reader, writer, &target, root),
        // A directory opens on Unix too; only a regular file has a body whose
        // length the response can promise.
        "GET" => match File::open(&target).and_then(|file| Ok((file.metadata()?, file))) {
            Ok((meta, file)) if meta.is_file() => write_response(
                writer,
                200,
                &[("Content-Type", "application/octet-stream")],
                &mut &file,
                meta.len(),
            ),
            _ => write_plain(writer, 404, "not found"),
        },
        _ => {
            let mut body: &[u8] = b"";
            write_response(writer, 405, &[("Allow", "GET, POST")], &mut body, 0)
        }
    }
}

fn store_file(
    head: &Head,
    reader: &mut impl Read,
    writer: &mut TcpStream,
    target: &Path,
    root: &Path,
) -> io::Result<()> {
    let declared = head.content_length;
    if declared > MAX_FILE_BYTES {
        return write_plain(writer, 413, "file too large");
    }
    // Check total workspace usage before receiving. Under the dispatch lock
    // this pre-check and the write cannot race.
    let used = workspace_bytes(root).unwrap_or(0);
    if used.saturating_add(declared) > MAX_WORKSPACE_BYTES {
        // Within the per-file cap, take the body before answering. A client
        // that writes the whole body before reading (the Worker's buffered
        // forward) would otherwise outlast linger's 2 s bound and see a reset
        // instead of this 413. An `Expect: 100-continue` client has not sent
        // the body and will not (D7).
        if !head.expect_continue {
            let _ = io::copy(reader, &mut io::sink());
        }
        return write_plain(writer, 413, "workspace quota exceeded");
    }

    let mut file = match File::create(target) {
        Ok(file) => file,
        Err(error) => {
            return write_plain(writer, 500, &format!("cannot create file: {error}"));
        }
    };
    // Invite the body only once it has somewhere to go (D7).
    if head.expect_continue
        && let Err(error) = write_continue(writer)
    {
        drop(file);
        let _ = std::fs::remove_file(target);
        return Err(error);
    }
    // `reader` stops at the declared length: a well-behaved client sends
    // precisely that many bytes and then waits for the response without
    // half-closing, so waiting for one more byte would deadlock.
    let written = io::copy(reader, &mut file);
    drop(file);

    let mut discard = |status: u16, message: &'static str| -> io::Result<()> {
        let _ = std::fs::remove_file(target);
        write_plain(writer, status, message)
    };
    match written {
        Err(_) => {
            let _ = std::fs::remove_file(target);
            write_plain(writer, 500, "cannot write file")
        }
        Ok(count) if count < declared => discard(400, "request body shorter than declared"),
        Ok(_) if workspace_bytes(root).unwrap_or(0) > MAX_WORKSPACE_BYTES => {
            discard(413, "workspace quota exceeded")
        }
        Ok(_) => write_response(writer, 200, &[], &mut io::empty(), 0),
    }
}

/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`
///
/// The allowed set has no `/` and no leading `.`, so `root.join(name)` cannot
/// escape the root.
///
/// Percent-escapes are intentionally not decoded: the allowed set is already
/// URL-safe, so legitimate names never need encoding, and decoding would
/// resurrect inputs like `%2e%2e` as traversal. Encoded names fail at `%` and
/// are rejected as-is.
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
