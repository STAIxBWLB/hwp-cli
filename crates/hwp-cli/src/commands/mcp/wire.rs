//! HTTP/1.1 framing for `hwp serve`: request head parsing, response writing and
//! the bounded lingering close.
//!
//! httparse owns request-head parsing; this module owns framing only (issue #312,
//! docs/design/22-remote-mcp-deployment.md §4). No route knowledge lives here.
//! Close-per-request (D2) is this module's contract: every response carries an
//! exact `Content-Length` and `Connection: close`. Body framing is
//! `Content-Length` only (D3).

use std::io::{self, BufRead, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Request head cap (D4). A head larger than this is rejected with 431.
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// Request header count cap (D4). Also the httparse parser buffer size.
const MAX_HEADERS: usize = 64;
/// Cap on the bytes linger discards (D6).
const MAX_LINGER_BYTES: u64 = 2 * 1024 * 1024;
/// Cap on how long linger holds the socket (D6).
const LINGER_TIMEOUT: Duration = Duration::from_secs(2);
/// How long linger waits for pipelined strays when the handler left nothing
/// declared-unread. Long enough for a second request already in flight, short
/// enough that a quiet client does not hold the connection thread.
const LINGER_IDLE: Duration = Duration::from_millis(250);

/// Parsed request head. `path` is the target with the query (`?` suffix) cut.
pub(super) struct Head {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) content_length: u64,
    pub(super) expect_continue: bool,
}

/// Head-level rejection. `status` is the final status; callers linger after
/// answering.
#[derive(Debug)]
pub(super) struct Reject {
    pub(super) status: u16,
}

impl From<httparse::Error> for Reject {
    fn from(error: httparse::Error) -> Self {
        Self {
            status: match error {
                httparse::Error::TooManyHeaders => 431,
                _ => 400,
            },
        }
    }
}

/// Reads the head up to the empty-line boundary and parses it. Reads at most
/// `MAX_HEAD_BYTES`.
///
/// Chunk scanning keeps memory bounded: a 64 MiB single header line still
/// enters memory one buffer at a time (D4). EOF before the boundary is an
/// incomplete head, hence 400. A socket read timeout arrives on the same path.
pub(super) fn read_head(reader: &mut impl BufRead) -> Result<Head, Reject> {
    let mut raw = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|_| Reject { status: 400 })?;
        if available.is_empty() {
            return Err(Reject { status: 400 });
        }
        // Move one line at a time into `raw`.
        let end = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|index| index + 1)
            .unwrap_or(available.len());
        raw.extend_from_slice(&available[..end]);
        reader.consume(end);
        if raw.len() > MAX_HEAD_BYTES {
            return Err(Reject { status: 431 });
        }
        // The head ends at an empty line. The terminator can straddle a read
        // boundary (a CRLF split across two chunks, or a chunk that opens on a
        // header line's own CRLF), so test the accumulated bytes, not the
        // current chunk.
        if raw.ends_with(b"\n\r\n") || raw.ends_with(b"\n\n") {
            break;
        }
    }
    parse_head(&raw)
}

fn parse_head(raw: &[u8]) -> Result<Head, Reject> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    if !matches!(
        request.parse(raw).map_err(Reject::from)?,
        httparse::Status::Complete(_)
    ) {
        return Err(Reject { status: 400 });
    }
    let method = request.method.ok_or(Reject { status: 400 })?;
    let target = request.path.ok_or(Reject { status: 400 })?;
    if !target.starts_with('/') {
        return Err(Reject { status: 400 });
    }
    let mut content_length: Option<u64> = None;
    let mut transfer_encoding = false;
    let mut expect_continue = false;
    for header in request.headers {
        let value = std::str::from_utf8(header.value).map_err(|_| Reject { status: 400 })?;
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_encoding = true;
        } else if header.name.eq_ignore_ascii_case("content-length") {
            // `u64::from_str` accepts a leading `+` ("+7"), so parsing alone is
            // not enough; require an all-decimal-digit value first.
            let text = value.trim();
            if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Reject { status: 400 });
            }
            let length = text.parse::<u64>().map_err(|_| Reject { status: 400 })?;
            if content_length.is_some_and(|existing| existing != length) {
                return Err(Reject { status: 400 });
            }
            content_length = Some(length);
        } else if header.name.eq_ignore_ascii_case("expect")
            && value.trim().eq_ignore_ascii_case("100-continue")
        {
            expect_continue = true;
        }
    }
    if transfer_encoding {
        return Err(Reject { status: 411 });
    }
    Ok(Head {
        method: method.to_owned(),
        path: target.split('?').next().unwrap_or("").to_owned(),
        content_length: content_length.unwrap_or(0),
        expect_continue,
    })
}

/// Fixed reason-phrase table for the statuses this adapter emits.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "Internal Server Error",
    }
}

/// Writes one response (D2, AC5): status line, extra headers,
/// `Content-Length`, `Connection: close`, and exactly `len` body bytes.
///
/// The head is flushed first, so the status reaches the client even while the
/// reject path lingers after answering.
pub(super) fn write_response(
    out: &mut impl Write,
    status: u16,
    extra_headers: &[(&str, &str)],
    body: &mut impl Read,
    len: u64,
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", status, reason_phrase(status));
    for (name, value) in extra_headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str(&format!(
        "Content-Length: {len}\r\nConnection: close\r\n\r\n"
    ));
    out.write_all(head.as_bytes())?;
    out.flush()?;
    io::copy(&mut body.take(len), out)?;
    out.flush()
}

/// The `Expect: 100-continue` interim response (D7). Written only immediately
/// before the body is read.
pub(super) fn write_continue(out: &mut impl Write) -> io::Result<()> {
    let line = format!("HTTP/1.1 {} {}\r\n\r\n", 100, reason_phrase(100));
    out.write_all(line.as_bytes())
}

/// Graceful close for paths that answered without reading the whole declared
/// body (D6).
///
/// The write half shuts down first so the client can read the status; the rest
/// of the body is then discarded, up to `min(remaining, MAX_LINGER_BYTES)`
/// bytes and `LINGER_TIMEOUT` overall. A well-behaved client that already sent
/// its body and waits for the status sees the status instead of a reset.
/// Once the declared remainder is drained, only strays from a pipelined second
/// request can arrive, so the wait drops to `LINGER_IDLE`: closing with unread
/// bytes in the kernel buffer would RST and could wipe the response.
pub(super) fn linger(stream: &TcpStream, remaining: u64) {
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let Ok(mut discard) = stream.try_clone() else {
        return;
    };
    let deadline = Instant::now() + LINGER_TIMEOUT;
    let mut left = remaining.min(MAX_LINGER_BYTES);
    let mut buffer = [0u8; 8192];
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let wait = if left == 0 {
            LINGER_IDLE.min(deadline - now)
        } else {
            deadline - now
        };
        let _ = discard.set_read_timeout(Some(wait));
        match discard.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => left = left.saturating_sub(n as u64),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn head(raw: &[u8]) -> Result<Head, Reject> {
        let mut reader = raw;
        read_head(&mut reader)
    }

    fn reject(raw: &[u8]) -> u16 {
        head(raw).err().expect("head must be rejected").status
    }

    #[test]
    fn read_head_parses_method_path_and_default_empty_body() {
        let parsed = head(b"GET /healthz HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.path, "/healthz");
        assert_eq!(parsed.content_length, 0);
        assert!(!parsed.expect_continue);
    }

    #[test]
    fn read_head_defaults_an_http10_request_to_an_empty_body() {
        let parsed = head(b"GET /healthz HTTP/1.0\r\n\r\n").unwrap();
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.content_length, 0);
    }

    #[test]
    fn read_head_reports_content_length_and_expect_continue() {
        let parsed =
            head(b"POST /mcp HTTP/1.1\r\nContent-Length: 7\r\nExpect: 100-continue\r\n\r\n")
                .unwrap();
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.content_length, 7);
        assert!(parsed.expect_continue);
    }

    #[test]
    fn read_head_only_flags_the_100_continue_expectation() {
        let parsed = head(b"POST /mcp HTTP/1.1\r\nExpect: 200-ok\r\n\r\n").unwrap();
        assert!(!parsed.expect_continue);
    }

    #[test]
    fn read_head_strips_the_query_from_the_target() {
        let parsed = head(b"GET /files/a.bin?download=1 HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(parsed.path, "/files/a.bin");
    }

    #[test]
    fn read_head_rejects_a_non_slash_target_with_400() {
        assert_eq!(reject(b"GET healthz HTTP/1.1\r\n\r\n"), 400);
    }

    #[test]
    fn read_head_rejects_an_incomplete_head_with_400() {
        // EOF without the empty line: the head is incomplete.
        assert_eq!(reject(b"POST /mcp HTTP/1.1\r\nContent-Length: 5\r\n"), 400);
        // A connection with nothing at all.
        assert_eq!(reject(b""), 400);
    }

    #[test]
    fn read_head_rejects_transfer_encoding_with_411() {
        assert_eq!(
            reject(b"POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            411
        );
    }

    #[test]
    fn read_head_rejects_a_non_decimal_content_length_with_400() {
        assert_eq!(
            reject(b"POST /mcp HTTP/1.1\r\nContent-Length: 0x10\r\n\r\n"),
            400
        );
        assert_eq!(
            reject(b"POST /mcp HTTP/1.1\r\nContent-Length: +7\r\n\r\n"),
            400
        );
        assert_eq!(
            reject(b"POST /mcp HTTP/1.1\r\nContent-Length: 1 7\r\n\r\n"),
            400
        );
    }

    #[test]
    fn read_head_rejects_disagreeing_content_length_fields_with_400() {
        assert_eq!(
            reject(b"POST /mcp HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 7\r\n\r\n"),
            400
        );
        // Repeated fields with the same value are allowed.
        let parsed =
            head(b"POST /mcp HTTP/1.1\r\nContent-Length: 5\r\ncontent-length: 5\r\n\r\n").unwrap();
        assert_eq!(parsed.content_length, 5);
    }

    #[test]
    fn read_head_allows_a_head_at_the_16_kib_bound() {
        let line = b"POST /mcp HTTP/1.1\r\n";
        let fixed = "X-Pad: ".len() + 2 + 2;
        let padding = MAX_HEAD_BYTES - line.len() - fixed;
        let mut raw = line.to_vec();
        raw.extend_from_slice(b"X-Pad: ");
        raw.extend(std::iter::repeat_n(b'x', padding));
        raw.extend_from_slice(b"\r\n\r\n");
        assert_eq!(raw.len(), MAX_HEAD_BYTES);
        assert!(head(&raw).is_ok());

        // One byte over the same head is 431.
        raw.insert(raw.len() - 4, b'x');
        assert_eq!(reject(&raw), 431);
    }

    #[test]
    fn read_head_rejects_more_than_64_headers_with_431() {
        let mut raw = String::from("POST /mcp HTTP/1.1\r\n");
        for index in 0..MAX_HEADERS {
            raw.push_str(&format!("X-H{index}: v\r\n"));
        }
        raw.push_str("\r\n");
        assert!(head(raw.as_bytes()).is_ok());

        let mut raw = String::from("POST /mcp HTTP/1.1\r\n");
        for index in 0..=MAX_HEADERS {
            raw.push_str(&format!("X-H{index}: v\r\n"));
        }
        raw.push_str("\r\n");
        assert_eq!(reject(raw.as_bytes()), 431);
    }

    #[test]
    fn read_head_tolerates_a_terminator_split_at_any_point() {
        // Regression: the empty-line check once looked only inside the current
        // chunk, so a read boundary that fell between a header value and its
        // CRLF read as the end of the head and a valid request got 400. Feed
        // the same head through every possible buffer capacity so the
        // terminator straddles a read boundary at every position.
        let mut raw = b"POST /mcp HTTP/1.1\r\nContent-Length: 4\r\nX-Pad: ".to_vec();
        raw.extend(std::iter::repeat_n(b'x', 8100));
        raw.extend_from_slice(b"\r\n\r\n");
        for capacity in 1..=raw.len() {
            let mut reader = BufReader::with_capacity(capacity, &raw[..]);
            let parsed = read_head(&mut reader).unwrap_or_else(|reject| {
                panic!(
                    "capacity {capacity} rejected a valid head: {}",
                    reject.status
                )
            });
            assert_eq!(parsed.method, "POST", "capacity {capacity}");
            assert_eq!(parsed.content_length, 4, "capacity {capacity}");
        }
    }

    #[test]
    fn reason_phrases_are_fixed_for_every_emitted_status() {
        for status in [100, 200, 202, 400, 404, 405, 411, 413, 431] {
            assert_ne!(reason_phrase(status), "Internal Server Error", "{status}");
        }
        assert_eq!(reason_phrase(500), "Internal Server Error");
    }

    #[test]
    fn write_response_frames_status_content_length_and_close() {
        let mut out = Vec::new();
        let mut body: &[u8] = b"hello";
        write_response(
            &mut out,
            200,
            &[("Content-Type", "application/json")],
            &mut body,
            5,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
        );
    }

    #[test]
    fn write_response_frames_an_empty_body() {
        let mut out = Vec::new();
        let mut body: &[u8] = b"";
        write_response(&mut out, 202, &[], &mut body, 0).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn write_response_sends_exactly_len_bytes() {
        let mut out = Vec::new();
        let mut body: &[u8] = b"0123456789";
        write_response(&mut out, 200, &[], &mut body, 4).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.ends_with("\r\n\r\n0123"), "{text}");
    }

    #[test]
    fn write_response_never_chunks_a_large_body() {
        let body = vec![b'a'; 128 * 1024];
        let mut out = Vec::new();
        let mut reader: &[u8] = &body;
        write_response(&mut out, 200, &[], &mut reader, body.len() as u64).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(!text.to_ascii_lowercase().contains("transfer-encoding"));
        assert_eq!(&out[out.len() - body.len()..], &body[..]);
    }

    #[test]
    fn write_continue_emits_the_interim_response() {
        let mut out = Vec::new();
        write_continue(&mut out).unwrap();
        assert_eq!(out, b"HTTP/1.1 100 Continue\r\n\r\n");
    }

    fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (server, client)
    }

    #[test]
    fn linger_binds_the_wait_on_a_sender_that_never_sends_the_body() {
        let (server, _client) = socket_pair();
        let start = Instant::now();
        linger(&server, u64::MAX);
        assert!(
            start.elapsed() < Duration::from_millis(2500),
            "linger must finish within 2 seconds"
        );
    }

    #[test]
    fn linger_returns_at_eof_when_the_client_already_sent_the_body() {
        let (server, mut client) = socket_pair();
        client.write_all(&[0u8; 16]).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let start = Instant::now();
        linger(&server, 1024);
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "linger must end immediately at EOF"
        );
    }

    #[test]
    fn linger_signals_eof_to_the_waiting_client() {
        let (server, mut client) = socket_pair();
        linger(&server, 0);
        let mut byte = [0u8; 1];
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let read = client.read(&mut byte);
        assert!(
            matches!(read, Ok(0)),
            "the client must see EOF after the write half closes: {read:?}"
        );
    }

    #[test]
    fn linger_drains_pipelined_strays_without_holding_the_thread() {
        let (server, mut client) = socket_pair();
        client.write_all(b"GET /healthz HTTP/1.1\r\n\r\n").unwrap();
        let start = Instant::now();
        linger(&server, 0);
        assert!(
            start.elapsed() < Duration::from_millis(1000),
            "draining strays must not wait the full timeout"
        );
    }
}
