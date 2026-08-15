//! `hwp mcp` — MCP(Model Context Protocol) stdio 서버.
//!
//! tokio/SDK 없이 serde_json만으로 동기 JSON-RPC 2.0(줄 단위 over stdio)을 구현한다.
//! 에이전트(Claude 등)가 도구 호출로 HWP를 **읽고·렌더해서 보고·편집·변환**하게 한다.
//! stdout은 프로토콜 전용(라이브러리 함수는 stdout 미오염, 로그는 stderr).
//!
//! 도구는 라이브러리 계층을 직접 감싼다(commands/*::run 아님 — 그건 stdout 출력).

use std::io::{BufRead, Read as _, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::cat::load_document;

/// Supported MCP protocol versions (newest first). Used in initialize negotiation.
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_REQUEST_LINE_BYTES: usize = 1024 * 1024;
const DEFAULT_READ_BYTES: usize = 256 * 1024;
const MAX_READ_BYTES: usize = 1024 * 1024;

/// Server context (default font directories for render/diff, `--root` file access sandbox).
pub struct Ctx {
    pub font_dirs: Vec<PathBuf>,
    /// Canonicalized allowed roots. Empty means unrestricted file access (previous behavior).
    pub roots: Vec<PathBuf>,
}

/// Canonicalize a path for sandbox authorization.
///
/// Keep Windows canonical paths in their verbatim spelling here. Lower-level
/// template and asset checks also use `std::fs::canonicalize`, so the roots in
/// `Ctx` must retain the same security identity. A sandbox-compatible spelling
/// is derived only after containment succeeds.
fn canonicalize_mcp_path(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

/// Derive the spelling used for downstream read-only filesystem I/O from an
/// already authorized canonical path.
fn sandbox_compatible_mcp_path(canonical: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        strip_windows_verbatim_prefix(canonical.to_path_buf())
    }
    #[cfg(not(windows))]
    {
        canonical.to_path_buf()
    }
}

/// Derive a spelling that remains ordinary even after the atomic writer adds
/// its private sibling workspace and staged filename.
fn sandbox_compatible_mcp_write_path(canonical: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;

        // Compared with the destination, StagedOutput's longest current path adds:
        // leading dot + marker + max u32 pid + max u64 sequence + separators +
        // 32-char random token + `.tmp` + workspace separator, then either repeats
        // the destination filename or uses `destination.backup`. Certification has
        // a deeper fixed report tree, so reserve its larger relative expansion too.
        const ATOMIC_STAGING_FIXED_OVERHEAD_UTF16: usize = 82;
        const ATOMIC_STAGING_MIN_CHILD_NAME_UTF16: usize = 18;
        let file_name_units = canonical
            .file_name()
            .map(|name| name.encode_wide().count())
            .unwrap_or(0);
        let output_staging_budget = ATOMIC_STAGING_FIXED_OVERHEAD_UTF16
            .saturating_add(file_name_units.max(ATOMIC_STAGING_MIN_CHILD_NAME_UTF16));
        strip_windows_verbatim_prefix_with_budget(
            canonical.to_path_buf(),
            output_staging_budget
                .max(hwp_cli::certification::WINDOWS_CERTIFICATION_TREE_OVERHEAD_UTF16),
        )
    }
    #[cfg(not(windows))]
    {
        canonical.to_path_buf()
    }
}

#[cfg(windows)]
fn windows_ordinary_component_is_safe(component: &std::ffi::OsStr) -> bool {
    let Some(text) = component.to_str() else {
        return false;
    };
    if text.is_empty() || text.ends_with('.') || text.ends_with(' ') {
        return false;
    }
    if text.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
    }) {
        return false;
    }

    let stem = text
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return false;
    }
    for prefix in ["COM", "LPT"] {
        if stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        }) {
            return false;
        }
    }
    true
}

#[cfg(windows)]
fn windows_verbatim_components_are_ordinary_safe(path: &Path) -> bool {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(_) => {}
            Prefix::VerbatimUNC(server, share) => {
                if !windows_ordinary_component_is_safe(server)
                    || !windows_ordinary_component_is_safe(share)
                {
                    return false;
                }
            }
            _ => return false,
        },
        _ => return false,
    }
    components.all(|component| match component {
        Component::RootDir => true,
        Component::Normal(component) => windows_ordinary_component_is_safe(component),
        _ => false,
    })
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    strip_windows_verbatim_prefix_with_budget(path, 0)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix_with_budget(
    path: PathBuf,
    additional_utf16_units: usize,
) -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    // Rust's Windows path layer switches ordinary absolute paths back to verbatim
    // spelling at this legacy directory-path threshold. Leave room for the NUL
    // terminator and for any downstream path expansion supplied by the caller.
    const LEGACY_MAX_PATH_UTF16: usize = 248;
    const SLASH: u16 = b'\\' as u16;
    const VERBATIM: [u16; 4] = [SLASH, SLASH, b'?' as u16, SLASH];
    const VERBATIM_UNC: [u16; 8] = [
        SLASH,
        SLASH,
        b'?' as u16,
        SLASH,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        SLASH,
    ];

    if !windows_verbatim_components_are_ordinary_safe(&path) {
        return path;
    }

    let fits_ordinary_io = |ordinary_units: usize| {
        ordinary_units
            .checked_add(additional_utf16_units)
            .and_then(|units| units.checked_add(1))
            .is_some_and(|units_with_nul| units_with_nul < LEGACY_MAX_PATH_UTF16)
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.starts_with(&VERBATIM_UNC) {
        let mut ordinary = Vec::with_capacity(wide.len() - VERBATIM_UNC.len() + 2);
        ordinary.extend_from_slice(&[SLASH, SLASH]);
        ordinary.extend_from_slice(&wide[VERBATIM_UNC.len()..]);
        if fits_ordinary_io(ordinary.len()) {
            return PathBuf::from(OsString::from_wide(&ordinary));
        }
        return path;
    }
    if wide.starts_with(&VERBATIM)
        && wide.get(5) == Some(&(b':' as u16))
        && wide
            .get(4)
            .is_some_and(|letter| matches!(*letter, 65..=90 | 97..=122))
    {
        let ordinary = &wide[VERBATIM.len()..];
        if fits_ordinary_io(ordinary.len()) {
            return PathBuf::from(OsString::from_wide(ordinary));
        }
    }
    path
}

/// stdio JSON-RPC 루프. EOF까지 한 줄씩 처리한다.
pub fn run(font_dirs: Vec<PathBuf>, roots: Vec<PathBuf>) -> anyhow::Result<()> {
    let mut canonical_roots = Vec::with_capacity(roots.len());
    for root in &roots {
        let canonical = canonicalize_mcp_path(root).map_err(|error| {
            anyhow::anyhow!(
                "--root 경로를 확인할 수 없습니다: {} ({error})",
                root.display()
            )
        })?;
        canonical_roots.push(canonical);
    }
    if canonical_roots.is_empty() {
        eprintln!("경고: --root 미지정 — MCP 서버의 파일 접근이 제한되지 않습니다");
    }
    let ctx = Ctx {
        font_dirs,
        roots: canonical_roots,
    };
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        if read_line_bounded(&mut reader, &mut line, MAX_REQUEST_LINE_BYTES)? == 0 {
            break; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(resp) = handle_request(trimmed, &ctx) {
            out.write_all(resp.as_bytes())?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
    }
    Ok(())
}

fn read_line_bounded(
    reader: &mut impl BufRead,
    line: &mut String,
    max_bytes: usize,
) -> std::io::Result<usize> {
    let mut limited = reader.take((max_bytes as u64).saturating_add(1));
    let read = limited.read_line(line)?;
    if line.len() > max_bytes || (read == max_bytes.saturating_add(1) && !line.ends_with('\n')) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("MCP 요청 한 줄이 {max_bytes} bytes 제한을 넘었습니다"),
        ));
    }
    Ok(read)
}

/// 한 줄 JSON-RPC 요청 → 응답 JSON 문자열. 알림(id 없음)이면 None.
pub fn handle_request(line: &str, ctx: &Ctx) -> Option<String> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            ));
        }
    };
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let is_notification = id.is_none();

    match method {
        "initialize" => {
            // Protocol negotiation: echo the client-requested version if supported, otherwise respond with the newest version.
            let requested = req
                .get("params")
                .and_then(|params| params.get("protocolVersion"))
                .and_then(Value::as_str);
            let protocol_version = match requested {
                Some(version) if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) => version,
                _ => SUPPORTED_PROTOCOL_VERSIONS[0],
            };
            Some(result_response(
                id_or_null(id),
                json!({
                    "protocolVersion": protocol_version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "hwp-cli", "version": env!("CARGO_PKG_VERSION")},
                }),
            ))
        }
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(result_response(id_or_null(id), json!({}))),
        "tools/list" => Some(result_response(
            id_or_null(id),
            json!({ "tools": tool_defs() }),
        )),
        "tools/call" => {
            if is_notification {
                return None;
            }
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Some(result_response(id_or_null(id), call_tool(name, &args, ctx)))
        }
        _ => {
            if is_notification {
                return None;
            }
            Some(error_response(
                id_or_null(id),
                -32601,
                &format!("method not found: {method}"),
            ))
        }
    }
}

fn id_or_null(id: Option<Value>) -> Value {
    id.unwrap_or(Value::Null)
}

fn result_response(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// 도구를 실행해 `tools/call` result를 만든다. 실행 오류는 isError=true content로.
fn call_tool(name: &str, args: &Value, ctx: &Ctx) -> Value {
    let result: Result<Vec<Value>, String> = match name {
        "hwp_info" => tool_info(args, ctx),
        "hwp_read" => tool_read(args, ctx),
        "hwp_grep" => tool_grep(args, ctx),
        "hwp_list_fields" => tool_list_fields(args, ctx),
        "hwp_list_bookmarks" => tool_list_bookmarks(args, ctx),
        "hwp_render" => tool_render(args, ctx),
        "hwp_edit" => tool_edit(args, ctx),
        "hwp_convert" => tool_convert(args, ctx),
        "hwp_new" => tool_new(args, ctx),
        "hwp_compose" => tool_compose(args, ctx),
        "hwp_template" => tool_template(args, ctx),
        "hwp_diff" => tool_diff(args, ctx),
        "hwp_slots" => tool_slots(args, ctx),
        "hwp_fill" => tool_fill(args, ctx),
        "hwp_validate" => tool_validate(args, ctx),
        "hwp_certify" => tool_certify(args, ctx),
        other => Err(format!("알 수 없는 도구: {other}")),
    };
    match result {
        Ok(content) => json!({"content": content, "isError": false}),
        Err(e) => json!({"content": [text_content(&format!("오류: {e}"))], "isError": true}),
    }
}

// ---- content/인자 헬퍼 ----

fn text_content(s: &str) -> Value {
    json!({"type": "text", "text": s})
}

fn image_content(png: &[u8]) -> Value {
    json!({"type": "image", "data": hwp_convert::base64::encode(png), "mimeType": "image/png"})
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("필수 인자 누락: {key}"))
}

fn arg_str_opt<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    args.get(key)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("{key}는 문자열이어야 합니다"))
        })
        .transpose()
}

fn arg_u64(args: &Value, key: &str, default: u64) -> Result<u64, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{key}는 0 이상의 정수여야 합니다")),
    }
}

fn arg_f64(args: &Value, key: &str, default: f64) -> Result<f64, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_f64()
            .ok_or_else(|| format!("{key}는 숫자여야 합니다")),
    }
}

fn arg_bool(args: &Value, key: &str, default: bool) -> Result<bool, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("{key}는 boolean이어야 합니다")),
    }
}

fn arg_array<'a>(args: &'a Value, key: &str) -> Result<&'a [Value], String> {
    match args.get(key) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("{key}는 배열이어야 합니다")),
    }
}

fn required_item_str<'a>(item: &'a Value, operation: &str, key: &str) -> Result<&'a str, String> {
    item.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{operation} 항목에 {key} 필요"))
}

fn required_item_u64(item: &Value, operation: &str, key: &str) -> Result<u64, String> {
    item.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{operation} 항목에 {key} 필요"))
}

fn required_item_usize(item: &Value, operation: &str, key: &str) -> Result<usize, String> {
    usize::try_from(required_item_u64(item, operation, key)?)
        .map_err(|_| format!("{operation} 항목의 {key}가 플랫폼 범위를 넘습니다"))
}

fn required_item_u16(item: &Value, operation: &str, key: &str) -> Result<u16, String> {
    u16::try_from(required_item_u64(item, operation, key)?).map_err(|_| {
        format!(
            "{operation} 항목의 {key}는 0..={} 범위여야 합니다",
            u16::MAX
        )
    })
}

fn optional_item_u16(item: &Value, operation: &str, key: &str) -> Result<Option<u16>, String> {
    item.get(key)
        .map(|_| required_item_u16(item, operation, key))
        .transpose()
}

fn optional_item_usize(item: &Value, operation: &str, key: &str) -> Result<Option<usize>, String> {
    item.get(key)
        .map(|_| required_item_usize(item, operation, key))
        .transpose()
}

fn optional_item_str<'a>(
    item: &'a Value,
    operation: &str,
    key: &str,
) -> Result<Option<&'a str>, String> {
    item.get(key)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("{operation}.{key}는 문자열이어야 합니다"))
        })
        .transpose()
}

fn optional_item_bool(item: &Value, operation: &str, key: &str) -> Result<Option<bool>, String> {
    item.get(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("{operation}.{key}는 boolean이어야 합니다"))
        })
        .transpose()
}

fn optional_item_f32(item: &Value, operation: &str, key: &str) -> Result<Option<f32>, String> {
    item.get(key)
        .map(|value| {
            let number = value
                .as_f64()
                .ok_or_else(|| format!("{operation}.{key}는 숫자여야 합니다"))?;
            let number = number as f32;
            if !number.is_finite() {
                return Err(format!("{operation}.{key}는 유한한 f32 범위여야 합니다"));
            }
            Ok(number)
        })
        .transpose()
}

// ---- Path sandbox (`--root`) ----

/// Checks that a canonical path sits below one of the allowed roots.
fn under_any_root(ctx: &Ctx, canonical: &Path, raw: &str) -> Result<PathBuf, String> {
    if ctx
        .roots
        .iter()
        .any(|root| canonical_path_starts_with(canonical, root))
    {
        Ok(canonical.to_path_buf())
    } else {
        Err(format!(
            "허용된 --root 밖 경로라 거부합니다: {raw} ({}으로 확인됨)",
            canonical.display()
        ))
    }
}

fn canonical_path_starts_with(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        if path.starts_with(root) {
            return true;
        }
        let path = strip_windows_verbatim_prefix(path.to_path_buf());
        let root = strip_windows_verbatim_prefix(root.to_path_buf());
        path.starts_with(root)
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

/// Read-path validation: the path must exist (canonicalize) and the canonical result
/// must sit below a root. Empty roots pass without a check (previous behavior).
fn checked_read_path(ctx: &Ctx, raw: &str) -> Result<PathBuf, String> {
    if ctx.roots.is_empty() {
        return Ok(PathBuf::from(raw));
    }
    let canonical = canonicalize_mcp_path(Path::new(raw))
        .map_err(|error| format!("경로를 확인할 수 없습니다: {raw} ({error})"))?;
    let authorized = under_any_root(ctx, &canonical, raw)?;
    Ok(sandbox_compatible_mcp_path(&authorized))
}

/// Write-path validation: rejects `..` components and a missing file name, then
/// canonicalizes an existing file (blocking symlink-overwrite bypasses) or, for a new
/// file, canonicalizes the parent and rejoins, before the root check.
/// Empty roots pass without a check (previous behavior).
fn checked_write_path(ctx: &Ctx, raw: &str) -> Result<PathBuf, String> {
    if ctx.roots.is_empty() {
        return Ok(PathBuf::from(raw));
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!("'..'를 포함한 출력 경로는 거부합니다: {raw}"));
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("출력 경로에 파일 이름이 없습니다: {raw}"))?;
    let resolved = if path.exists() {
        canonicalize_mcp_path(path)
            .map_err(|error| format!("출력 경로를 확인할 수 없습니다: {raw} ({error})"))?
    } else {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_parent = canonicalize_mcp_path(parent).map_err(|error| {
            format!(
                "출력 경로의 부모 디렉터리를 확인할 수 없습니다: {} ({error})",
                parent.display()
            )
        })?;
        canonical_parent.join(file_name)
    };
    let authorized = under_any_root(ctx, &resolved, raw)?;
    Ok(sandbox_compatible_mcp_write_path(&authorized))
}

fn font_dirs_for(args: &Value, ctx: &Ctx) -> Result<Vec<PathBuf>, String> {
    let mut dirs = ctx.font_dirs.clone();
    if let Some(d) = arg_str_opt(args, "font_dir")? {
        // Per-call font_dir is subject to the sandbox check (startup --font-dir is trusted).
        dirs.push(checked_read_path(ctx, d)?);
    }
    Ok(dirs)
}

// ---- 도구 핸들러 ----

fn tool_info(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let v = crate::commands::info::info_json(&path).map_err(|e| e.to_string())?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&v).unwrap_or_default(),
    )])
}

fn tool_read(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let format = arg_str_opt(args, "format")?.unwrap_or("plain");
    let with_header_footer = arg_bool(args, "with_header_footer", false)?;
    let with_hidden = arg_bool(args, "with_hidden", false)?;
    let with_segments = arg_bool(args, "with_segments", false)?;
    // Same contract as cat: with_segments is markdown-only, and the with_* flags apply
    // only to plain/markdown (html/json/csv take no options — they are ignored if given).
    if with_segments && !matches!(format, "markdown" | "md") {
        return Err(format!(
            "with_segments는 format=markdown 전용입니다 (요청: {format})"
        ));
    }
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    let text_options = || hwp_model::TextOptions {
        include_header_footer: with_header_footer,
        include_hidden: with_hidden,
    };
    let md_options = || hwp_convert::MarkdownOptions {
        text: text_options(),
        ..Default::default()
    };
    // With with_segments, also collect the character-range segment map alongside the markdown.
    let (text, segments) = match format {
        "plain" => (doc.plain_text_with(&text_options()), None),
        "markdown" | "md" if with_segments => {
            let (markdown, segments) = hwp_convert::to_markdown_with_segments(&doc, &md_options())
                .map_err(|e| e.to_string())?;
            (markdown, Some(segments))
        }
        "markdown" | "md" => (
            hwp_convert::to_markdown_with(&doc, &md_options()).map_err(|e| e.to_string())?,
            None,
        ),
        "json" => (
            hwp_convert::to_json(&doc, true, false).map_err(|e| e.to_string())?,
            None,
        ),
        "html" => (hwp_convert::to_html(&doc), None),
        "csv" => (hwp_convert::to_csv(&doc), None),
        other => {
            return Err(format!(
                "알 수 없는 format: {other} (plain|markdown|json|html|csv)"
            ));
        }
    };
    let offset = usize::try_from(arg_u64(args, "offset", 0)?)
        .map_err(|_| "offset이 플랫폼 범위를 넘습니다".to_string())?;
    let max_bytes = usize::try_from(arg_u64(args, "max_bytes", DEFAULT_READ_BYTES as u64)?)
        .map_err(|_| "max_bytes가 플랫폼 범위를 넘습니다".to_string())?;
    if max_bytes == 0 || max_bytes > MAX_READ_BYTES {
        return Err(format!("max_bytes는 1..={MAX_READ_BYTES} 범위여야 합니다"));
    }
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err(format!(
            "offset은 UTF-8 경계인 0..={} byte 범위여야 합니다",
            text.len()
        ));
    }
    let mut end = offset.saturating_add(max_bytes).min(text.len());
    while end > offset && !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = end < text.len();
    let metadata = json!({
        "offset": offset,
        "returned_bytes": end - offset,
        "total_bytes": text.len(),
        "truncated": truncated,
        "next_offset": truncated.then_some(end),
    });
    let content = match segments {
        // Same shape as cat's single-line JSON envelope. The markdown holds only the
        // returned window, and segments are filtered to those intersecting the window
        // while offsets stay absolute against the full markdown (unicode characters).
        Some(segments) => {
            let char_start = text[..offset].chars().count();
            let char_end = text[..end].chars().count();
            let segments: Vec<Value> = segments
                .iter()
                .filter(|s| s.start < char_end && s.end > char_start)
                .map(|s| {
                    json!({
                        "kind": "para",
                        "section": s.section,
                        "para": s.para,
                        "start": s.start,
                        "end": s.end,
                    })
                })
                .collect();
            let envelope = json!({
                "markdown": &text[offset..end],
                "segments": segments,
            });
            serde_json::to_string(&envelope).map_err(|e| e.to_string())?
        }
        None => text[offset..end].to_string(),
    };
    Ok(vec![
        text_content(&content),
        text_content(&serde_json::to_string(&metadata).unwrap_or_default()),
    ])
}

fn tool_list_fields(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    let fields: Vec<Value> = hwp_convert::list_fields(&doc)
        .iter()
        .map(|f| {
            json!({
                "kind": f.kind, "ctrl_id": f.ctrl_id,
                "name": f.name, "command": f.command, "value": f.value,
            })
        })
        .collect();
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&fields).unwrap_or_default(),
    )])
}

fn tool_list_bookmarks(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    let bookmarks: Vec<Value> = hwp_convert::list_bookmarks(&doc)
        .iter()
        .map(|b| json!({ "name": b.name }))
        .collect();
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&bookmarks).unwrap_or_default(),
    )])
}

fn tool_slots(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    let items: Vec<Value> = hwp_convert::scan_placeholders(&doc)
        .iter()
        .map(|p| json!({ "name": p.name, "occurrences": p.occurrences }))
        .collect();
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({ "placeholders": items })).unwrap_or_default(),
    )])
}

fn tool_fill(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    let values_obj = args
        .get("values")
        .and_then(Value::as_object)
        .ok_or("필수 인자 누락: values (객체 {이름:값})")?;
    let values: std::collections::BTreeMap<String, String> = values_obj
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect();
    // parts(선택): {앵커이름: 부분 파일 경로} — 부분(md+HTML) 블록을 앵커 문단에 이식.
    let parts_obj = args.get("parts").and_then(Value::as_object);
    let report = if let Some(parts) = parts_obj {
        if parts.is_empty() && values.is_empty() {
            return Err("values와 parts가 모두 비어 있습니다".into());
        }
        let mut set: Vec<String> = values.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for (k, v) in parts {
            let path = v
                .as_str()
                .ok_or("parts 값은 부분 파일 경로 문자열이어야 합니다")?;
            let path = checked_read_path(ctx, path)?;
            set.push(format!("{k}=@{}", path.display()));
        }
        crate::commands::fill::execute(
            &input,
            &output,
            &set,
            None,
            arg_bool(args, "allow_partial", false)?,
            &ctx.roots,
        )
        .map_err(|error| format!("{error:#}"))?
    } else {
        crate::commands::fill::execute_values(
            &input,
            &output,
            &values,
            arg_bool(args, "allow_partial", false)?,
        )
        .map_err(|error| format!("{error:#}"))?
    };
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&crate::commands::fill::report_json(&report))
            .unwrap_or_default(),
    )])
}

fn tool_validate(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let v = crate::commands::validate::validate_json(&path);
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&v).unwrap_or_default(),
    )])
}

fn tool_certify(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "input" | "policy" | "report"))
    {
        return Err(format!("알 수 없는 hwp_certify 인자: {unknown}"));
    }
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let policy = checked_read_path(ctx, arg_str(args, "policy")?)?;
    let report = checked_write_path(ctx, arg_str(args, "report")?)?;
    let outcome = hwp_cli::certification::execute(&input, &policy, &report)
        .map_err(|error| format!("{error:#}"))?;
    let summary = serde_json::to_string_pretty(&json!({
        "overall": outcome.overall,
        "report": outcome.report_dir,
    }))
    .map_err(|error| error.to_string())?;
    if outcome.overall != hwp_cli::certification::OverallStatus::Passed {
        return Err(summary);
    }
    Ok(vec![text_content(&summary)])
}

fn tool_render(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    if args.get("page").is_some() && args.get("pages").is_some() {
        return Err("page와 pages는 함께 지정할 수 없습니다".into());
    }
    let format = match arg_str_opt(args, "format")?.unwrap_or("png") {
        "png" => hwp_cli::cli::RenderFormat::Png,
        "svg" => hwp_cli::cli::RenderFormat::Svg,
        "pdf" => hwp_cli::cli::RenderFormat::Pdf,
        other => return Err(format!("알 수 없는 format: {other} (png|svg|pdf)")),
    };
    let page = usize::try_from(arg_u64(args, "page", 1)?)
        .map_err(|_| "page가 플랫폼 범위를 넘습니다".to_string())?;
    let dpi = crate::commands::render::validated_dpi(arg_f64(args, "dpi", 120.0)?)
        .map_err(|error| error.to_string())?;
    let output_path = arg_str_opt(args, "output_path")?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    let opts = hwp_render::RenderOptions {
        dpi,
        font_dirs: font_dirs_for(args, ctx)?,
    };
    let pages_spec = arg_str_opt(args, "pages")?;

    // base64 return path: png without output_path. Only a single-page selection is allowed.
    if matches!(format, hwp_cli::cli::RenderFormat::Png) && output_path.is_none() {
        let selected = match pages_spec {
            // Legacy contract: page keeps the same selection semantics as render_document_pages.
            None => vec![page],
            Some(spec) => {
                let total = hwp_render::count_pages(&doc, &opts);
                crate::commands::render::parse_pages(spec, total)
                    .map_err(|error| error.to_string())?
            }
        };
        if selected.len() != 1 {
            return Err(
                "다중 페이지 렌더는 output_path가 필요합니다 (페이지별 파일로 저장)".into(),
            );
        }
        let page = selected[0];
        let out = hwp_render::render_document_pages(&doc, &opts, Some(&[page]))
            .map_err(|e| e.to_string())?;
        let pixmap = &out.pages[0];
        let png = pixmap
            .encode_png()
            .ok()
            .ok_or_else(|| "PNG 인코딩 실패".to_string())?;
        const MAX_MCP_PNG_BYTES: usize = 16 * 1024 * 1024;
        if png.len() > MAX_MCP_PNG_BYTES {
            return Err(format!(
                "MCP 렌더 PNG가 응답 상한 {MAX_MCP_PNG_BYTES} bytes를 초과합니다: {} bytes",
                png.len()
            ));
        }
        let summary = format!(
            "페이지 {page}/{} 렌더 ({}×{}px, {dpi}dpi). issues={}, info={}, complete={}, sha256={}",
            out.total_pages,
            pixmap.width(),
            pixmap.height(),
            out.report.issue_count,
            out.report.info_count,
            out.report.complete,
            out.report.sha256,
        );
        return Ok(vec![text_content(&summary), image_content(&png)]);
    }

    // File-publish path: svg/pdf, multiple pages, or an explicit output_path. Goes through
    // the same atomic publish transaction as the CLI render and returns JSON metadata.
    let output = checked_write_path(
        ctx,
        output_path.ok_or(
            "svg/pdf 또는 output_path 없는 다중 페이지 렌더는 output_path 인자가 필요합니다",
        )?,
    )?;
    // Each path derived from the CLI's per-page filename rule is also sandbox-checked.
    let checked_derived = |base: &Path, page_no: usize, multi: bool| -> Result<PathBuf, String> {
        let derived = crate::commands::render::page_path(base, page_no, multi);
        let raw = derived
            .to_str()
            .ok_or_else(|| format!("출력 경로가 UTF-8이 아닙니다: {}", derived.display()))?;
        checked_write_path(ctx, raw)
    };
    let (files, selected): (Vec<PathBuf>, Vec<usize>) = match format {
        hwp_cli::cli::RenderFormat::Png => {
            let total = hwp_render::count_pages(&doc, &opts);
            let selected = match pages_spec {
                Some(spec) => crate::commands::render::parse_pages(spec, total)
                    .map_err(|error| error.to_string())?,
                None => crate::commands::render::parse_pages(&page.to_string(), total)
                    .map_err(|error| error.to_string())?,
            };
            let result = hwp_render::render_document_pages(&doc, &opts, Some(&selected))
                .map_err(|e| e.to_string())?;
            let multi = selected.len() > 1;
            let mut outputs = Vec::with_capacity(selected.len());
            let mut files = Vec::with_capacity(selected.len());
            for (&page_no, pixmap) in selected.iter().zip(&result.pages) {
                let derived = checked_derived(&output, page_no, multi)?;
                let png = pixmap
                    .encode_png()
                    .map_err(|error| format!("PNG 인코딩 실패 ({}): {error}", derived.display()))?;
                files.push(derived.clone());
                outputs.push((derived, png));
            }
            crate::commands::render::publish_render_set(&outputs, &path)
                .map_err(|error| error.to_string())?;
            (files, selected)
        }
        hwp_cli::cli::RenderFormat::Svg => {
            let result = hwp_render::render_document_svg(&doc, &opts);
            let selected = match pages_spec {
                Some(spec) => crate::commands::render::parse_pages(spec, result.pages.len())
                    .map_err(|error| error.to_string())?,
                None => crate::commands::render::parse_pages(&page.to_string(), result.pages.len())
                    .map_err(|error| error.to_string())?,
            };
            let multi = selected.len() > 1;
            let mut outputs = Vec::with_capacity(selected.len());
            let mut files = Vec::with_capacity(selected.len());
            for &page_no in &selected {
                let derived = checked_derived(&output, page_no, multi)?;
                files.push(derived.clone());
                outputs.push((derived, result.pages[page_no - 1].as_bytes().to_vec()));
            }
            crate::commands::render::publish_render_set(&outputs, &path)
                .map_err(|error| error.to_string())?;
            (files, selected)
        }
        hwp_cli::cli::RenderFormat::Pdf => {
            let total = hwp_render::count_pages(&doc, &opts);
            let selected = match pages_spec {
                Some(spec) => crate::commands::render::parse_pages(spec, total)
                    .map_err(|error| error.to_string())?,
                None => crate::commands::render::parse_pages(&page.to_string(), total)
                    .map_err(|error| error.to_string())?,
            };
            // Unlike PNG/SVG, PDF is a single multi-page file (no per-page split).
            let result = hwp_render::render_document_pdf(&doc, &opts, Some(&selected))
                .map_err(|e| e.to_string())?;
            crate::commands::render::write_render_bytes(&output, &path, &result.data)
                .map_err(|error| error.to_string())?;
            (vec![output.clone()], selected)
        }
    };
    let metadata = json!({
        "files": files,
        "pages": selected,
        "dpi": dpi,
    });
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&metadata).unwrap_or_default(),
    )])
}

/// hwp_grep match return cap — excess matches are cut and marked truncated=true.
/// count is always the full pre-truncation total.
const MAX_GREP_MATCHES: usize = 200;

fn grep_result(matches: Vec<String>) -> Value {
    grep_result_capped(matches, MAX_GREP_MATCHES)
}

fn grep_result_capped(matches: Vec<String>, cap: usize) -> Value {
    let count = matches.len();
    let truncated = count > cap;
    let matches: Vec<String> = matches.into_iter().take(cap).collect();
    json!({
        "matches": matches,
        "count": count,
        "truncated": truncated,
    })
}

fn tool_grep(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let pattern = arg_str(args, "pattern")?;
    let ignore_case = arg_bool(args, "ignore_case", false)?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    // Zero matches are a normal result, not an error (unlike the CLI grep exit(1) contract).
    let matches =
        crate::commands::grep::search(&doc, pattern, ignore_case).map_err(|e| e.to_string())?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&grep_result(matches)).unwrap_or_default(),
    )])
}

fn tool_edit(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    use crate::commands::edit::TypedEditOperation as Op;

    let mut operations = Vec::new();

    for item in arg_array(args, "replace")? {
        operations.push(Op::Replace {
            from: required_item_str(item, "replace", "from")?.to_string(),
            to: required_item_str(item, "replace", "to")?.to_string(),
        });
    }
    for item in arg_array(args, "set_cell")? {
        operations.push(Op::SetCell {
            table: required_item_usize(item, "set_cell", "table")?,
            row: required_item_u16(item, "set_cell", "row")?,
            col: required_item_u16(item, "set_cell", "col")?,
            text: required_item_str(item, "set_cell", "text")?.to_string(),
        });
    }
    for item in arg_array(args, "set_field")? {
        operations.push(Op::SetField {
            name: required_item_str(item, "set_field", "name")?.to_string(),
            value: required_item_str(item, "set_field", "value")?.to_string(),
        });
    }
    for item in arg_array(args, "set_meta")? {
        operations.push(Op::SetMeta {
            key: required_item_str(item, "set_meta", "key")?.to_string(),
            value: required_item_str(item, "set_meta", "value")?.to_string(),
        });
    }
    for item in arg_array(args, "create_field")? {
        operations.push(Op::CreateField {
            anchor: required_item_str(item, "create_field", "anchor")?.to_string(),
            name: required_item_str(item, "create_field", "name")?.to_string(),
            value: optional_item_str(item, "create_field", "value")?
                .unwrap_or("")
                .to_string(),
        });
    }
    for item in arg_array(args, "create_bookmark")? {
        operations.push(Op::CreateBookmark {
            anchor: required_item_str(item, "create_bookmark", "anchor")?.to_string(),
            name: required_item_str(item, "create_bookmark", "name")?.to_string(),
        });
    }
    for item in arg_array(args, "create_hyperlink")? {
        let url = required_item_str(item, "create_hyperlink", "url")?.to_string();
        operations.push(Op::CreateHyperlink {
            anchor: required_item_str(item, "create_hyperlink", "anchor")?.to_string(),
            display: optional_item_str(item, "create_hyperlink", "display")?
                .unwrap_or(&url)
                .to_string(),
            url,
        });
    }
    for item in arg_array(args, "insert_image")? {
        let size_mm = match (
            optional_item_f32(item, "insert_image", "width_mm")?,
            optional_item_f32(item, "insert_image", "height_mm")?,
        ) {
            (Some(width), Some(height)) => Some((width, height)),
            (None, None) => None,
            _ => {
                return Err(
                    "insert_image는 유한한 width_mm와 height_mm를 함께 지정해야 합니다".into(),
                );
            }
        };
        operations.push(Op::InsertImage {
            anchor: required_item_str(item, "insert_image", "anchor")?.to_string(),
            path: checked_read_path(ctx, required_item_str(item, "insert_image", "path")?)?,
            size_mm,
        });
    }
    for item in arg_array(args, "seal")? {
        let size_mm = optional_item_f32(item, "seal", "size_mm")?;
        operations.push(Op::Seal {
            anchor: required_item_str(item, "seal", "anchor")?.to_string(),
            path: checked_read_path(ctx, required_item_str(item, "seal", "path")?)?,
            size_mm,
        });
    }
    for item in arg_array(args, "set_format")? {
        let mut format = hwp_convert::CharFormat {
            bold: optional_item_bool(item, "set_format", "bold")?,
            italic: optional_item_bool(item, "set_format", "italic")?,
            underline: optional_item_bool(item, "set_format", "underline")?,
            strike: optional_item_bool(item, "set_format", "strike")?,
            ..Default::default()
        };
        if let Some(value) = optional_item_f32(item, "set_format", "size")? {
            format.size_pt = Some(value);
        }
        if let Some(value) = optional_item_str(item, "set_format", "color")? {
            format.color = Some(
                crate::commands::edit::parse_color(value)
                    .ok_or_else(|| format!("set_format.color를 해석할 수 없습니다: {value:?}"))?,
            );
        }
        operations.push(Op::SetFormat {
            pattern: required_item_str(item, "set_format", "pattern")?.to_string(),
            format,
        });
    }
    for item in arg_array(args, "set_align")? {
        operations.push(Op::SetAlign {
            pattern: required_item_str(item, "set_align", "pattern")?.to_string(),
            align: crate::commands::edit::parse_align(required_item_str(
                item,
                "set_align",
                "align",
            )?)
            .map_err(|error| error.to_string())?,
        });
    }
    for item in arg_array(args, "insert_para")? {
        operations.push(Op::InsertPara {
            anchor: required_item_str(item, "insert_para", "anchor")?.to_string(),
            text: required_item_str(item, "insert_para", "text")?.to_string(),
            before: optional_item_bool(item, "insert_para", "before")?.unwrap_or(false),
        });
    }
    for item in arg_array(args, "delete_para")? {
        operations.push(Op::DeletePara {
            matching: required_item_str(item, "delete_para", "matching")?.to_string(),
        });
    }
    for item in arg_array(args, "add_row")? {
        let count = optional_item_u16(item, "add_row", "count")?;
        if count == Some(0) {
            return Err("add_row: count는 1 이상이어야 합니다".to_string());
        }
        operations.push(Op::AddRow {
            table: required_item_usize(item, "add_row", "table")?,
            at: optional_item_u16(item, "add_row", "at")?,
            count: count.map(usize::from).unwrap_or(1),
            template_row: optional_item_u16(item, "add_row", "template_row")?,
        });
    }
    for item in arg_array(args, "add_col")? {
        let count = optional_item_u16(item, "add_col", "count")?;
        if count == Some(0) {
            return Err("add_col: count는 1 이상이어야 합니다".to_string());
        }
        operations.push(Op::AddCol {
            table: required_item_usize(item, "add_col", "table")?,
            at: optional_item_u16(item, "add_col", "at")?,
            count: count.unwrap_or(1),
        });
    }
    for item in arg_array(args, "delete_row")? {
        operations.push(Op::DeleteRow {
            table: required_item_usize(item, "delete_row", "table")?,
            row: required_item_u16(item, "delete_row", "row")?,
        });
    }
    for item in arg_array(args, "delete_col")? {
        operations.push(Op::DeleteCol {
            table: required_item_usize(item, "delete_col", "table")?,
            col: required_item_u16(item, "delete_col", "col")?,
        });
    }
    for item in arg_array(args, "merge_cells")? {
        operations.push(Op::MergeCells {
            table: required_item_usize(item, "merge_cells", "table")?,
            r1: required_item_u16(item, "merge_cells", "r1")?,
            c1: required_item_u16(item, "merge_cells", "c1")?,
            r2: required_item_u16(item, "merge_cells", "r2")?,
            c2: required_item_u16(item, "merge_cells", "c2")?,
        });
    }
    for item in arg_array(args, "split_cell")? {
        operations.push(Op::SplitCell {
            table: required_item_usize(item, "split_cell", "table")?,
            row: required_item_u16(item, "split_cell", "row")?,
            col: required_item_u16(item, "split_cell", "col")?,
        });
    }
    for item in arg_array(args, "add_table")? {
        // The JSON boundary validates only the shape (an array of string arrays) — content
        // problems like empty row data are rejected by the library (add_table), whose error propagates as-is.
        let rows_value = item
            .get("rows")
            .and_then(Value::as_array)
            .ok_or_else(|| "add_table 항목에 rows 필요".to_string())?;
        let mut rows = Vec::with_capacity(rows_value.len());
        for row in rows_value {
            let cells = row
                .as_array()
                .ok_or_else(|| "add_table.rows는 문자열 배열의 배열이어야 합니다".to_string())?;
            let mut parsed_row = Vec::with_capacity(cells.len());
            for cell in cells {
                parsed_row.push(
                    cell.as_str()
                        .ok_or_else(|| "add_table.rows의 셀은 문자열이어야 합니다".to_string())?
                        .to_string(),
                );
            }
            rows.push(parsed_row);
        }
        operations.push(Op::AddTable {
            anchor: required_item_str(item, "add_table", "anchor")?.to_string(),
            rows,
        });
    }
    for item in arg_array(args, "clone_table")? {
        let text_mode = match item.get("text_mode").and_then(Value::as_str).map(str::trim) {
            None | Some("") | Some("blank") => hwp_convert::CloneTextMode::Blank,
            Some("keep") => hwp_convert::CloneTextMode::Keep,
            Some(_) => {
                return Err("clone_table: text_mode는 blank|keep 이어야 합니다".to_string());
            }
        };
        operations.push(Op::CloneTable {
            source_table: required_item_usize(item, "clone_table", "source_table")?,
            anchor: required_item_str(item, "clone_table", "anchor")?.to_string(),
            text_mode,
        });
    }
    for item in arg_array(args, "set_para")? {
        // The CLI line-spacing (% integer | Npt) split into two numeric arguments — mutually exclusive.
        let line_spacing = match (
            optional_item_f32(item, "set_para", "line_spacing_pct")?,
            optional_item_f32(item, "set_para", "line_spacing_pt")?,
        ) {
            (Some(_), Some(_)) => {
                return Err(
                    "set_para는 line_spacing_pct와 line_spacing_pt를 함께 지정할 수 없습니다"
                        .into(),
                );
            }
            (Some(pct), None) => Some((0, pct as i32)),
            (None, Some(pt)) => Some((1, (pt * 100.0).round() as i32)),
            (None, None) => None,
        };
        let props = hwp_convert::ParaProps {
            line_spacing,
            indent: optional_item_f32(item, "set_para", "indent_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_left: optional_item_f32(item, "set_para", "left_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_right: optional_item_f32(item, "set_para", "right_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            spacing_top: optional_item_f32(item, "set_para", "top_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            spacing_bottom: optional_item_f32(item, "set_para", "bottom_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
        };
        operations.push(Op::SetPara {
            pattern: required_item_str(item, "set_para", "pattern")?.to_string(),
            props,
        });
    }
    // Like the CLI's cumulative --set-page flags, a single object is merged into one PageProps and applied.
    if let Some(item) = args.get("set_page") {
        if !item.is_object() {
            return Err("set_page는 단일 객체여야 합니다".into());
        }
        let props = hwp_convert::PageProps {
            width: optional_item_f32(item, "set_page", "width_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            height: optional_item_f32(item, "set_page", "height_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_left: optional_item_f32(item, "set_page", "margin_left_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_right: optional_item_f32(item, "set_page", "margin_right_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_top: optional_item_f32(item, "set_page", "margin_top_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_bottom: optional_item_f32(item, "set_page", "margin_bottom_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            landscape: optional_item_str(item, "set_page", "orientation")?
                .map(|value| match value.trim().to_ascii_lowercase().as_str() {
                    "landscape" | "가로" => Ok(true),
                    "portrait" | "세로" => Ok(false),
                    other => Err(format!(
                        "알 수 없는 용지 방향: {other:?} (portrait/landscape)"
                    )),
                })
                .transpose()?,
        };
        operations.push(Op::SetPage { props });
    }
    for item in arg_array(args, "delete_image")? {
        operations.push(Op::DeleteImage {
            anchor: required_item_str(item, "delete_image", "anchor")?.to_string(),
        });
    }
    for item in arg_array(args, "delete_table")? {
        let index = optional_item_usize(item, "delete_table", "index")?;
        let anchor = optional_item_str(item, "delete_table", "anchor")?.map(str::to_string);
        match (&index, &anchor) {
            (Some(_), Some(_)) => {
                return Err("delete_table 항목은 index와 anchor 중 하나만 지정해야 합니다".into());
            }
            (None, None) => {
                return Err("delete_table 항목에 index 또는 anchor가 필요합니다".into());
            }
            _ => {}
        }
        operations.push(Op::DeleteTable { index, anchor });
    }
    for item in arg_array(args, "delete_field")? {
        operations.push(Op::DeleteField {
            name: required_item_str(item, "delete_field", "name")?.to_string(),
        });
    }
    for item in arg_array(args, "delete_bookmark")? {
        operations.push(Op::DeleteBookmark {
            name: required_item_str(item, "delete_bookmark", "name")?.to_string(),
        });
    }

    let plan = crate::commands::edit::EditPlan::from_typed(
        operations,
        true,
        arg_bool(args, "allow_partial", false)?,
    );
    let report = crate::commands::edit::execute(&input, &output, &plan)
        .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "input": input,
            "output": report.output,
            "applied": report.applied,
            "warnings": report.warnings,
            "preservation": report.preservation,
        }))
        .unwrap_or_default(),
    )])
}

/// Maps hwp_convert arguments to convert::execute parameters (a test seam verified without rendering).
#[derive(Debug)]
struct ConvertRequest {
    input: PathBuf,
    output: PathBuf,
    to: Option<hwp_cli::cli::ConvertFormat>,
    strict: bool,
    embed_bin: bool,
    media_dir: Option<PathBuf>,
    with_header_footer: bool,
    with_hidden: bool,
    font_dirs: Vec<PathBuf>,
}

fn parse_convert_format(value: &str) -> Result<hwp_cli::cli::ConvertFormat, String> {
    use hwp_cli::cli::ConvertFormat as F;
    match value {
        "hwp" => Ok(F::Hwp),
        "hwpx" => Ok(F::Hwpx),
        "md" | "markdown" => Ok(F::Md),
        "json" => Ok(F::Json),
        "html" => Ok(F::Html),
        "pdf" => Ok(F::Pdf),
        "odt" => Ok(F::Odt),
        "txt" => Ok(F::Txt),
        "csv" => Ok(F::Csv),
        "docx" => Ok(F::Docx),
        other => Err(format!(
            "알 수 없는 to: {other} (hwp|hwpx|md|json|html|pdf|odt|txt|csv|docx)"
        )),
    }
}

fn convert_request(args: &Value, ctx: &Ctx) -> Result<ConvertRequest, String> {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    // An explicit to wins over output-extension inference, like the CLI --to.
    let to = arg_str_opt(args, "to")?
        .map(parse_convert_format)
        .transpose()?;
    // media_dir is the markdown image-extraction directory — checked as a write path.
    let media_dir = arg_str_opt(args, "media_dir")?
        .map(|raw| checked_write_path(ctx, raw))
        .transpose()?;
    Ok(ConvertRequest {
        input,
        output,
        to,
        strict: arg_bool(args, "strict", true)?,
        embed_bin: arg_bool(args, "embed_bin", false)?,
        media_dir,
        with_header_footer: arg_bool(args, "with_header_footer", false)?,
        with_hidden: arg_bool(args, "with_hidden", false)?,
        // Passes the merged startup --font-dir + per-call font_dir list through — previously
        // the list was always empty, so fonts set at MCP server startup never applied to PDF conversion.
        font_dirs: font_dirs_for(args, ctx)?,
    })
}

fn tool_convert(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let request = convert_request(args, ctx)?;
    let report = crate::commands::convert::execute(
        &request.input,
        &request.output,
        request.to,
        request.strict,
        None,
        false,
        request.embed_bin,
        &crate::commands::convert::MdOpts {
            media_dir: request.media_dir.as_deref(),
            with_header_footer: request.with_header_footer,
            with_hidden: request.with_hidden,
        },
        request.font_dirs,
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "input": request.input,
            "output": request.output,
            "strict": request.strict,
            "warnings": report.warnings,
            "preservation": report.preservation,
        }))
        .unwrap_or_default(),
    )])
}

fn tool_new(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    let input = match (arg_str_opt(args, "markdown")?, arg_str_opt(args, "json")?) {
        (Some(_), Some(_)) => return Err("markdown과 json은 동시에 지정할 수 없습니다".into()),
        (Some(markdown), None) => crate::commands::new::NewInput::Markdown {
            text: markdown,
            base_dir: None,
            // Bind image references inside the markdown to the sandbox roots (#56).
            roots: &ctx.roots,
        },
        (None, Some(document_json)) => crate::commands::new::NewInput::Json(document_json),
        (None, None) => crate::commands::new::NewInput::Empty,
    };
    let metadata = arg_array(args, "set_meta")?
        .iter()
        .map(|item| {
            Ok(format!(
                "{}={}",
                required_item_str(item, "set_meta", "key")?,
                required_item_str(item, "set_meta", "value")?
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let report = crate::commands::new::execute(&output, input, &metadata, None, false)
        .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "output": report.output,
            "warnings": report.warnings,
            "preservation": report.preservation,
        }))
        .unwrap_or_default(),
    )])
}

fn tool_compose(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    const ALLOWED: &[&str] = &[
        "spec",
        "spec_path",
        "format",
        "base_dir",
        "output",
        "dry_run",
        "allow_visual_fallback",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("알 수 없는 hwp_compose 인자: {unknown}"));
    }

    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    let explicit_format = arg_str_opt(args, "format")?
        .map(parse_spec_format)
        .transpose()?;
    let (input, format, base_dir, source_path) =
        match (args.get("spec"), arg_str_opt(args, "spec_path")?) {
            (Some(_), Some(_)) => return Err("spec과 spec_path는 동시에 지정할 수 없습니다".into()),
            (None, None) => return Err("spec 또는 spec_path 중 하나가 필요합니다".into()),
            (Some(spec), None) => {
                let input = match spec {
                    Value::String(text) => text.clone(),
                    Value::Object(_) => serde_json::to_string(spec).map_err(|e| e.to_string())?,
                    _ => {
                        return Err(
                            "spec은 DocumentSpec 객체 또는 JSON/YAML 문자열이어야 합니다".into(),
                        );
                    }
                };
                let format =
                    explicit_format.unwrap_or(hwp_cli::document_spec::SpecInputFormat::Json);
                let base_dir = match arg_str_opt(args, "base_dir")? {
                    Some(raw) => checked_read_path(ctx, raw)?,
                    None => checked_read_path(ctx, ".")?,
                };
                (input, format, base_dir, None)
            }
            (None, Some(spec_path)) => {
                if args.get("base_dir").is_some() {
                    return Err("spec_path 사용 시 base_dir는 지정할 수 없습니다".into());
                }
                let path = checked_read_path(ctx, spec_path)?;
                let input = crate::commands::compose::read_bounded(&path)
                    .map_err(|error| format!("{error:#}"))?;
                let format = explicit_format
                    .map(Ok)
                    .unwrap_or_else(|| hwp_cli::document_spec::infer_input_format(&path))
                    .map_err(|error| error.to_string())?;
                let base_dir = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
                (input, format, base_dir, Some(path))
            }
        };
    let report = crate::commands::compose::execute_text_with_source(
        &input,
        format,
        &base_dir,
        &output,
        arg_bool(args, "dry_run", false)?,
        arg_bool(args, "allow_visual_fallback", false)?,
        source_path.as_deref(),
        &ctx.roots,
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&report).unwrap_or_default(),
    )])
}

fn tool_template(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    const ALLOWED: &[&str] = &[
        "template",
        "template_path",
        "template_format",
        "data",
        "data_path",
        "data_format",
        "base_dir",
        "output",
        "dry_run",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("알 수 없는 hwp_template 인자: {unknown}"));
    }
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    let explicit_template_format = arg_str_opt(args, "template_format")?
        .map(parse_spec_format)
        .transpose()?;
    let explicit_data_format = arg_str_opt(args, "data_format")?
        .map(parse_spec_format)
        .transpose()?;

    let mut source_paths = Vec::new();
    let (template_input, template_format, base_dir) =
        match (args.get("template"), arg_str_opt(args, "template_path")?) {
            (Some(_), Some(_)) => {
                return Err("template과 template_path는 동시에 지정할 수 없습니다".into());
            }
            (None, None) => return Err("template 또는 template_path 중 하나가 필요합니다".into()),
            (Some(template), None) => {
                let input = inline_contract_input(template, "template")?;
                let format = explicit_template_format
                    .unwrap_or(hwp_cli::document_spec::SpecInputFormat::Json);
                let base = match arg_str_opt(args, "base_dir")? {
                    Some(raw) => checked_read_path(ctx, raw)?,
                    None => checked_read_path(ctx, ".")?,
                };
                (input, format, base)
            }
            (None, Some(path)) => {
                if args.get("base_dir").is_some() {
                    return Err("template_path 사용 시 base_dir는 지정할 수 없습니다".into());
                }
                let path = checked_read_path(ctx, path)?;
                let input = crate::commands::template::read_bounded(
                    &path,
                    hwp_cli::template_spec::MAX_TEMPLATE_BYTES,
                    "TemplateSpec",
                )
                .map_err(|error| format!("{error:#}"))?;
                let format = explicit_template_format
                    .map(Ok)
                    .unwrap_or_else(|| hwp_cli::template_spec::infer_input_format(&path))
                    .map_err(|error| error.to_string())?;
                let base = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
                source_paths.push(path);
                (input, format, base)
            }
        };
    let (data_input, data_format) = match (args.get("data"), arg_str_opt(args, "data_path")?) {
        (Some(_), Some(_)) => return Err("data와 data_path는 동시에 지정할 수 없습니다".into()),
        (None, None) => return Err("data 또는 data_path 중 하나가 필요합니다".into()),
        (Some(data), None) => (
            inline_contract_input(data, "data")?,
            explicit_data_format.unwrap_or(hwp_cli::document_spec::SpecInputFormat::Json),
        ),
        (None, Some(path)) => {
            let path = checked_read_path(ctx, path)?;
            let input = crate::commands::template::read_bounded(
                &path,
                hwp_cli::template_spec::MAX_DATA_BYTES,
                "TemplateData",
            )
            .map_err(|error| format!("{error:#}"))?;
            let format = explicit_data_format
                .map(Ok)
                .unwrap_or_else(|| hwp_cli::template_spec::infer_input_format(&path))
                .map_err(|error| error.to_string())?;
            source_paths.push(path);
            (input, format)
        }
    };

    let report = crate::commands::template::execute_text(
        &template_input,
        template_format,
        &data_input,
        data_format,
        &base_dir,
        &output,
        arg_bool(args, "dry_run", false)?,
        &source_paths,
        &ctx.roots,
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &crate::commands::template::serialize_report(&report)
            .map_err(|error| format!("{error:#}"))?,
    )])
}

fn inline_contract_input(value: &Value, label: &str) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Object(_) => serde_json::to_string(value).map_err(|error| error.to_string()),
        _ => Err(format!("{label}은 객체 또는 JSON/YAML 문자열이어야 합니다")),
    }
}

fn parse_spec_format(value: &str) -> Result<hwp_cli::document_spec::SpecInputFormat, String> {
    match value {
        "json" => Ok(hwp_cli::document_spec::SpecInputFormat::Json),
        "yaml" => Ok(hwp_cli::document_spec::SpecInputFormat::Yaml),
        other => Err(format!("알 수 없는 format: {other} (json|yaml)")),
    }
}

fn tool_diff(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let reference = checked_read_path(ctx, arg_str(args, "ref")?)?;
    let page = usize::try_from(arg_u64(args, "page", 1)?)
        .map_err(|_| "page가 플랫폼 범위를 넘습니다".to_string())?;
    let dpi = crate::commands::render::validated_dpi(arg_f64(args, "dpi", 120.0)?)
        .map_err(|error| error.to_string())?;
    let doc = load_document(&input).map_err(|e| e.to_string())?;
    let out = hwp_render::render_document_pages(
        &doc,
        &hwp_render::RenderOptions {
            dpi,
            font_dirs: font_dirs_for(args, ctx)?,
        },
        Some(&[page]),
    )
    .map_err(|e| e.to_string())?;
    let refpx = hwp_render::load_png(&reference).map_err(|e| e.to_string())?;
    let (rep, _) = hwp_render::compare(&out.pages[0], &refpx, 16)?;
    let v = json!({
        "ink_ratio": rep.ink_ratio,
        "dx": rep.dx,
        "dy": rep.dy,
        "bad_pixel_pct": rep.bad_pixel_pct,
        "mae": rep.mae,
    });
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&v).unwrap_or_default(),
    )])
}

// ---- 도구 정의 (tools/list) ----

fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "hwp_info",
            "description": "HWP/HWPX 파일의 포맷·버전·속성·스트림 목록을 JSON으로 진단(본문 파싱 불필요).",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string", "description": "hwp/hwpx 파일 경로"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_read",
            "description": "본문을 추출한다. format=json이면 전체 IR(구조)을, markdown/plain이면 텍스트를, html/csv면 해당 직렬화를 반환. with_header_footer/with_hidden은 plain/markdown에만 적용(cat과 동일). with_segments는 markdown 전용으로 {markdown, segments} JSON 봉투를 반환(오프셋은 전체 기준 절대 문자 위치).",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "format": {"type": "string", "enum": ["plain", "markdown", "json", "html", "csv"], "description": "기본 plain"},
                "with_header_footer": {"type": "boolean", "description": "머리말/꼬리말 포함(plain/markdown, 기본 false)"},
                "with_hidden": {"type": "boolean", "description": "숨은 설명 포함(plain/markdown, 기본 false)"},
                "with_segments": {"type": "boolean", "description": "markdown 전용. 문단 원본 좌표 세그먼트 맵 포함"},
                "offset": {"type": "integer", "minimum": 0, "description": "UTF-8 byte offset, 기본 0"},
                "max_bytes": {"type": "integer", "minimum": 1, "maximum": 1048576, "description": "반환 byte 상한, 기본 262144"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_grep",
            "description": "문단 텍스트 검색(본문·표 셀·글상자 재귀). {matches, count, truncated} 반환 — matches는 최대 200건, count는 전체 매칭 수, 0건 매칭도 정상 결과.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "pattern": {"type": "string", "description": "검색할 부분 문자열"},
                "ignore_case": {"type": "boolean", "description": "대소문자 무시, 기본 false"}
            }, "required": ["path", "pattern"]}
        }),
        json!({
            "name": "hwp_list_fields",
            "description": "필드/누름틀 목록(이름·종류·값·명령)을 JSON으로. 누름틀(%clk)은 name으로 채울 수 있다.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_list_bookmarks",
            "description": "책갈피(bokm) 목록(이름)을 JSON으로.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_render",
            "description": "페이지를 렌더한다. 기본은 단일 페이지 PNG를 base64 이미지로 반환(에이전트가 문서를 직접 본다). format=svg/pdf 또는 다중 페이지 선택(pages)은 output_path가 필요하고, 파일로 저장 뒤 {files, pages, dpi} JSON을 반환한다(16MiB 응답 상한 우회). 페이지별 파일명은 CLI와 같이 <stem>-<N>.<ext>(단일 페이지면 경로 그대로), pdf는 단일 멀티페이지 파일.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "page": {"type": "integer", "description": "1-기반, 기본 1. pages와 함께 지정 불가"},
                "pages": {"type": "string", "description": "페이지 범위 spec: \"1\", \"1-3\", \"all\". page와 함께 지정 불가"},
                "format": {"type": "string", "enum": ["png", "svg", "pdf"], "description": "기본 png"},
                "output_path": {"type": "string", "description": "출력 파일 경로. svg/pdf·다중 페이지 필수. png 다중 페이지는 페이지별 <stem>-<N>.png"},
                "dpi": {"type": "number", "minimum": hwp_render::MIN_DPI, "maximum": hwp_render::MAX_DPI, "description": "기본 120"},
                "font_dir": {"type": "string", "description": "추가 폰트 디렉터리(선택)"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_edit",
            "description": "CLI와 같은 strict·atomic·재읽기 검증 경로로 기존 문서를 편집한다. 기본은 미적용 요청 하나라도 있으면 실패.",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"},
                "output": {"type": "string"},
                "replace": {"type": "array", "items": {"type": "object", "properties": {
                    "from": {"type": "string"}, "to": {"type": "string"}}, "required": ["from", "to"]},
                    "description": "텍스트 치환(모든 일치)"},
                "set_cell": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "row": {"type": "integer"},
                    "col": {"type": "integer"}, "text": {"type": "string"}},
                    "required": ["table", "row", "col", "text"]}, "description": "표 셀 설정(0-기반)"},
                "set_field": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["name", "value"]}, "description": "필드/누름틀 채우기(이름으로)"},
                "create_field": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "name": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["anchor", "name"]}, "description": "앵커 텍스트 뒤에 %clk 누름틀 생성(이름·선택 표시값; set_field로 채움)"},
                "create_bookmark": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "name": {"type": "string"}},
                    "required": ["anchor", "name"]}, "description": "앵커 텍스트 뒤에 책갈피(bokm 지점 표식) 생성"},
                "create_hyperlink": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "url": {"type": "string"}, "display": {"type": "string"}},
                    "required": ["anchor", "url"]}, "description": "앵커 텍스트 뒤에 하이퍼링크(%hlk) 생성(display 생략 시 URL 표시)"},
                "insert_image": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "path": {"type": "string"},
                    "width_mm": {"type": "number"}, "height_mm": {"type": "number"}},
                    "required": ["anchor", "path"]}, "description": "앵커 텍스트 뒤에 이미지(png/jpg/bmp/gif) 삽입(width_mm/height_mm 생략 시 원본 크기)"},
                "seal": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "path": {"type": "string"},
                    "size_mm": {"type": "number"}},
                    "required": ["anchor", "path"]}, "description": "앵커 문구 위에 도장 이미지 부유 배치"},
                "set_meta": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}, "description": "title/author/subject/keywords 메타데이터"},
                "set_format": {"type": "array", "items": {"type": "object", "properties": {
                    "pattern": {"type": "string"}, "bold": {"type": "boolean"},
                    "italic": {"type": "boolean"}, "underline": {"type": "boolean"},
                    "strike": {"type": "boolean"}, "size": {"type": "number", "description": "pt"},
                    "color": {"type": "string", "description": "#RRGGBB 또는 색이름"}},
                    "required": ["pattern"]}, "description": "글자 서식(매칭 텍스트)"},
                "set_align": {"type": "array", "items": {"type": "object", "properties": {
                    "pattern": {"type": "string"},
                    "align": {"type": "string", "enum": ["left", "right", "center", "justify", "distribute", "divide"]}},
                    "required": ["pattern", "align"]}, "description": "문단 정렬(매칭 문단)"},
                "insert_para": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "text": {"type": "string"},
                    "before": {"type": "boolean", "description": "true면 앵커 문단 앞(기본 뒤)"}},
                    "required": ["anchor", "text"]}, "description": "문단 삽입(앵커 문단 앞/뒤, 모양 상속)"},
                "delete_para": {"type": "array", "items": {"type": "object", "properties": {
                    "matching": {"type": "string"}},
                    "required": ["matching"]}, "description": "매칭 텍스트가 든 문단 삭제(최소 1문단 유지)"},
                "add_row": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"},
                    "at": {"type": "integer", "minimum": 0, "maximum": 65535, "description": "삽입 경계(생략 시 끝, 0-기반)"},
                    "count": {"type": "integer", "minimum": 1, "maximum": 65535},
                    "template_row": {"type": "integer", "minimum": 0, "maximum": 65535, "description": "행 높이·셀 서식 기증 행(텍스트는 복제 안 함)"}},
                    "required": ["table"]}, "description": "N번째 표의 at 경계 앞에 빈 행 count개 삽입(생략 시 끝에 1개, 0-기반, 병합 표도 지원)"},
                "add_col": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"},
                    "at": {"type": "integer", "minimum": 0, "maximum": 65535},
                    "count": {"type": "integer", "minimum": 1, "maximum": 65535}},
                    "required": ["table"]}, "description": "N번째 표의 at 위치(생략 시 끝)에 열 count개 추가(0-기반, 전체 폭 유지, 병합 표도 지원)"},
                "delete_row": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "row": {"type": "integer"}},
                    "required": ["table", "row"]}, "description": "N번째 표의 R행 삭제(0-기반, 병합 행은 거부)"},
                "delete_col": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "col": {"type": "integer"}},
                    "required": ["table", "col"]}},
                "merge_cells": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "r1": {"type": "integer"},
                    "c1": {"type": "integer"}, "r2": {"type": "integer"}, "c2": {"type": "integer"}},
                    "required": ["table", "r1", "c1", "r2", "c2"]}},
                "split_cell": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "row": {"type": "integer"}, "col": {"type": "integer"}},
                    "required": ["table", "row", "col"]}},
                "add_table": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"},
                    "rows": {"type": "array", "items": {"type": "array", "items": {"type": "string"}}}},
                    "required": ["anchor", "rows"]},
                    "description": "앵커 문단 뒤에 균일 표 삽입(rows는 행(문자열 배열)의 배열)"},
                "clone_table": {"type": "array", "items": {"type": "object", "properties": {
                    "source_table": {"type": "integer", "minimum": 0, "description": "원본 표(0-기반, 재귀 순서)"},
                    "anchor": {"type": "string", "description": "복제본을 삽입할 앵커 문단 텍스트(그 뒤에 삽입)"},
                    "text_mode": {"type": "string", "enum": ["blank", "keep"],
                        "description": "blank(기본)=구조·서식만 복제하고 셀 내용은 빈 문단 1개, keep=지원 콘텐츠(중첩 표·그림)까지 복제(id 재부여, 안전하게 재매핑할 수 없는 개체가 있으면 실패)"}},
                    "required": ["source_table", "anchor"]},
                    "description": "N번째 표를 깊은 복제해 앵커 문단 뒤에 삽입(구조·병합·테두리·채우기·서식 보존)"},
                "set_para": {"type": "array", "items": {"type": "object", "properties": {
                    "pattern": {"type": "string"},
                    "line_spacing_pct": {"type": "number", "description": "줄간격 비율(%)"},
                    "line_spacing_pt": {"type": "number", "description": "고정 줄간격(pt) — pct와 함께 지정 불가"},
                    "indent_mm": {"type": "number"}, "left_mm": {"type": "number"},
                    "right_mm": {"type": "number"}, "top_mm": {"type": "number"},
                    "bottom_mm": {"type": "number"}},
                    "required": ["pattern"]},
                    "description": "문단모양(매칭 문단): 줄간격(비율% 또는 고정pt)·들여쓰기·여백(mm)"},
                "set_page": {"type": "object", "properties": {
                    "width_mm": {"type": "number"}, "height_mm": {"type": "number"},
                    "margin_left_mm": {"type": "number"}, "margin_right_mm": {"type": "number"},
                    "margin_top_mm": {"type": "number"}, "margin_bottom_mm": {"type": "number"},
                    "orientation": {"type": "string", "enum": ["portrait", "landscape", "가로", "세로"]}},
                    "description": "페이지 설정(모든 구역 정의에 적용): 용지 크기·여백(mm)·방향"},
                "delete_image": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}}, "required": ["anchor"]},
                    "description": "앵커 문단의 그림 삭제"},
                "delete_table": {"type": "array", "items": {"type": "object", "properties": {
                    "index": {"type": "integer", "description": "0-기반 표 인덱스"},
                    "anchor": {"type": "string", "description": "앵커 텍스트가 든 문단의 표"}}},
                    "description": "표 삭제 — index와 anchor 중 정확히 하나"},
                "delete_field": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}}, "required": ["name"]},
                    "description": "이름으로 필드 삭제(hwp_list_fields로 이름 확인)"},
                "delete_bookmark": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}}, "required": ["name"]},
                    "description": "이름으로 책갈피 삭제(hwp_list_bookmarks로 이름 확인)"},
                "allow_partial": {"type": "boolean", "description": "true면 일치한 요청만 게시; 기본 false"}
            }, "required": ["input", "output"]}
        }),
        json!({
            "name": "hwp_convert",
            "description": "포맷 변환. 기본은 출력 확장자(.hwp/.hwpx/.json/.md/.html/.pdf/.odt/.txt/.csv/.docx)로 결정하고 to가 있으면 CLI --to처럼 확장자보다 우선한다. pdf는 텍스트 선택가능 벡터(이미지 포함). embed_bin이면 JSON에 이미지 base64 임베드. media_dir/with_header_footer/with_hidden은 markdown 출력 전용.",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"}, "output": {"type": "string"},
                "to": {"type": "string", "enum": ["hwp", "hwpx", "md", "json", "html", "pdf", "odt", "txt", "csv", "docx"], "description": "대상 포맷(선택). 지정 시 출력 확장자 추론보다 우선"},
                "media_dir": {"type": "string", "description": "markdown 이미지 추출 디렉터리(선택, 기본 \"<출력스템>.media\")"},
                "with_header_footer": {"type": "boolean", "description": "markdown에 머리말/꼬리말 포함, 기본 false"},
                "with_hidden": {"type": "boolean", "description": "markdown에 숨은 설명 포함, 기본 false"},
                "font_dir": {"type": "string", "description": "추가 폰트 디렉터리(선택) — pdf 렌더에 적용"},
                "embed_bin": {"type": "boolean"},
                "strict": {"type": "boolean", "description": "HWP/HWPX DROP 경고를 실패 처리; MCP 기본 true"}
            }, "required": ["input", "output"]}
        }),
        json!({
            "name": "hwp_new",
            "description": "CLI와 같은 strict·atomic·재읽기 검증 경로로 .hwp/.hwpx 새 문서를 생성.",
            "inputSchema": {"type": "object", "properties": {
                "output": {"type": "string"},
                "markdown": {"type": "string", "description": "markdown 본문(선택)"},
                "json": {"type": "string", "description": "IR JSON 본문(선택)"},
                "set_meta": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}}
            }, "required": ["output"]}
        }),
        json!({
            "name": "hwp_compose",
            "description": "DocumentSpec v1/v2 객체/JSON/YAML을 검증하고 CLI와 같은 deterministic·strict·atomic·재읽기 검증 경로로 .hwp/.hwpx 문서를 합성.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "spec": {
                        "oneOf": [{"type": "object"}, {"type": "string"}],
                        "description": "DocumentSpec v1/v2 객체 또는 JSON/YAML 문자열"
                    },
                    "spec_path": {"type": "string", "description": "DocumentSpec v1/v2 파일 경로; spec과 상호 배타적"},
                    "format": {"type": "string", "enum": ["json", "yaml"], "description": "문자열 spec 또는 확장자 없는 입력의 포맷"},
                    "base_dir": {"type": "string", "description": "inline spec의 상대 asset 기준 디렉터리"},
                    "output": {"type": "string", "description": "출력 .hwp/.hwpx 경로"},
                    "dry_run": {"type": "boolean", "description": "검증·컴파일 보고서만 반환하고 파일을 쓰지 않음"},
                    "allow_visual_fallback": {"type": "boolean", "deprecated": true, "description": "[deprecated] v1 compatibility only; DocumentSpec v2가 true를 받으면 policy_conflict로 거부"}
                },
                "required": ["output"],
                "oneOf": [
                    {"required": ["spec"], "not": {"required": ["spec_path"]}},
                    {"required": ["spec_path"], "not": {"required": ["spec"]}}
                ]
            }
        }),
        json!({
            "name": "hwp_template",
            "description": "TemplateSpec/Data v1의 typed AST를 CLI와 같은 bounded·deterministic·strict 경로로 확장하고 native HWP/HWPX를 생성한다. reference_hwpx는 package-surgical 보존 모드다.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "template": {
                        "oneOf": [{"type": "object"}, {"type": "string"}],
                        "description": "TemplateSpec v1 객체 또는 JSON/YAML 문자열"
                    },
                    "template_path": {"type": "string"},
                    "template_format": {"type": "string", "enum": ["json", "yaml"]},
                    "data": {
                        "oneOf": [{"type": "object"}, {"type": "string"}],
                        "description": "TemplateData v1 객체 또는 JSON/YAML 문자열"
                    },
                    "data_path": {"type": "string"},
                    "data_format": {"type": "string", "enum": ["json", "yaml"]},
                    "base_dir": {"type": "string", "description": "inline template의 상대 reference/asset 기준 디렉터리"},
                    "output": {"type": "string", "description": "출력 .hwp/.hwpx 경로"},
                    "dry_run": {"type": "boolean", "description": "실제 확장·writer·검증 후 게시만 생략"}
                },
                "required": ["output"],
                "allOf": [
                    {"oneOf": [
                        {"required": ["template"], "not": {"required": ["template_path"]}},
                        {"required": ["template_path"], "not": {"required": ["template"]}}
                    ]},
                    {"oneOf": [
                        {"required": ["data"], "not": {"required": ["data_path"]}},
                        {"required": ["data_path"], "not": {"required": ["data"]}}
                    ]}
                ]
            }
        }),
        json!({
            "name": "hwp_diff",
            "description": "렌더 결과를 기준 PNG와 비교해 오차(잉크 적용률·위치 오프셋·픽셀 차이율)를 측정.",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"}, "ref": {"type": "string", "description": "기준 PNG 경로"},
                "page": {"type": "integer"}, "dpi": {"type": "number", "minimum": hwp_render::MIN_DPI, "maximum": hwp_render::MAX_DPI},
                "font_dir": {"type": "string"}
            }, "required": ["input", "ref"]}
        }),
        json!({
            "name": "hwp_slots",
            "description": "`{{name}}` 텍스트 자리표시자(템플릿 슬롯) 목록을 등장 순서로 반환.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_fill",
            "description": "템플릿의 `{{name}}`를 채운다. values는 평문 치환(hwpx 패키지 보존), parts는 부분(md+HTML, 계약 docs/design/18) 파일을 앵커 문단에 블록 이식(.hwp/.hwpx).",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"}, "output": {"type": "string"},
                "values": {"type": "object", "additionalProperties": {"type": "string"},
                    "description": "{자리표시자이름: 값} 객체"},
                "parts": {"type": "object", "additionalProperties": {"type": "string"},
                    "description": "{앵커이름: 부분 파일 경로(md+HTML)} 객체 — 앵커 문단을 부분 블록으로 교체"},
                "allow_partial": {"type": "boolean", "description": "미발견 키가 있어도 일치한 값만 게시; 기본 false"}
            }, "required": ["input", "output", "values"]}
        }),
        json!({
            "name": "hwp_validate",
            "description": "구조 검증(mimetype·필수 엔트리·XML 파싱). {valid, errors, warnings} 반환.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_certify",
            "description": "versioned policy로 package/반복 import/native render/선택적 LibreOffice+H2Orestart 독립 import를 인증하고 새 artifact 디렉터리를 원자적으로 게시한다. passed만 성공이다.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "input": {"type": "string"},
                    "policy": {"type": "string", "description": "hwp-certification-policy-v1 JSON/YAML 경로"},
                    "report": {"type": "string", "description": "존재하지 않는 artifact 디렉터리 경로"}
                },
                "required": ["input", "policy", "report"]
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_mcp_paths_use_sandbox_compatible_drive_spelling() {
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"\\?\C:\Temp\document.hwpx")),
            PathBuf::from(r"C:\Temp\document.hwpx")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_paths_use_sandbox_compatible_unc_spelling() {
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\document.hwpx")),
            PathBuf::from(r"\\server\share\document.hwpx")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_paths_retain_verbatim_spelling_when_semantics_can_change() {
        for raw in [
            r"\\?\C:\root.\document.hwpx",
            r"\\?\C:\root \document.hwpx",
            r"\\?\C:\NUL\document.hwpx",
            r"\\?\C:\CON.txt\document.hwpx",
            r"\\?\C:\COM1\document.hwpx",
            r"\\?\C:\LPT9.log\document.hwpx",
            r"\\?\UNC\server\share.\document.hwpx",
        ] {
            let path = PathBuf::from(raw);
            assert_eq!(strip_windows_verbatim_prefix(path.clone()), path, "{raw}");
        }

        let long = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(260)));
        assert_eq!(strip_windows_verbatim_prefix(long.clone()), long);
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_paths_reserve_ordinary_length_for_atomic_staging() {
        let below_direct_limit = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(243)));
        assert_ne!(
            strip_windows_verbatim_prefix(below_direct_limit.clone()),
            below_direct_limit
        );
        let at_direct_limit = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(244)));
        assert_eq!(
            strip_windows_verbatim_prefix(at_direct_limit.clone()),
            at_direct_limit
        );

        let below_write_limit = PathBuf::from(format!(r"\\?\C:\{}\x", "a".repeat(140)));
        assert_ne!(
            sandbox_compatible_mcp_write_path(&below_write_limit),
            below_write_limit
        );
        let at_write_limit = PathBuf::from(format!(r"\\?\C:\{}\x", "a".repeat(141)));
        assert_eq!(
            sandbox_compatible_mcp_write_path(&at_write_limit),
            at_write_limit
        );

        let long_filename_output = PathBuf::from(format!(r"\\?\C:\{}\out.hwpx", "a".repeat(180)));
        assert_ne!(
            strip_windows_verbatim_prefix(long_filename_output.clone()),
            long_filename_output
        );
        assert_eq!(
            sandbox_compatible_mcp_write_path(&long_filename_output),
            long_filename_output
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_root_check_accepts_only_semantically_equivalent_spellings() {
        assert!(canonical_path_starts_with(
            Path::new(r"C:\Temp\workspace\document.hwpx"),
            Path::new(r"\\?\C:\Temp\workspace")
        ));
        assert!(!canonical_path_starts_with(
            Path::new(r"C:\root\document.hwpx"),
            Path::new(r"\\?\C:\root.")
        ));
        assert!(canonical_path_starts_with(
            Path::new(r"\\?\C:\root.\document.hwpx"),
            Path::new(r"\\?\C:\root.")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_roots_keep_canonical_identity_for_downstream_checks() {
        let base =
            std::env::temp_dir().join(format!("hwp-cli-mcp-canonical-root-{}", std::process::id()));
        let root = base.join("root");
        let document = root.join("document.hwpx");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&document, b"test").unwrap();

        let canonical_root = canonicalize_mcp_path(&root).unwrap();
        let canonical_document = std::fs::canonicalize(&document).unwrap();
        assert!(canonical_document.starts_with(&canonical_root));

        let ctx = ctx_with_roots(vec![canonical_root.clone()]);
        assert_eq!(
            checked_read_path(&ctx, document.to_str().unwrap()).unwrap(),
            strip_windows_verbatim_prefix(canonical_document)
        );

        let unsafe_output = canonical_root.join("report.hwpx.");
        assert_eq!(
            checked_write_path(&ctx, unsafe_output.to_str().unwrap()).unwrap(),
            unsafe_output
        );
        let _ = std::fs::remove_dir_all(base);
    }

    fn ctx() -> Ctx {
        Ctx {
            font_dirs: vec![PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../fonts"
            ))],
            roots: Vec::new(),
        }
    }

    /// Sandbox context allowing only the given root (canonicalized by the caller).
    fn ctx_with_roots(roots: Vec<PathBuf>) -> Ctx {
        Ctx {
            font_dirs: Vec::new(),
            roots,
        }
    }

    fn fixture(rel: &str) -> String {
        format!("{}/../../fixtures/{rel}", env!("CARGO_MANIFEST_DIR"))
    }

    /// fixture 바이너리는 저장소에서 제외된다(로컬 전용). 없으면 `true`(스킵).
    fn skip_if_no_fixtures() -> bool {
        if std::path::Path::new(&fixture("hwp5/hello_world.hwp")).exists() {
            return false;
        }
        eprintln!("스킵: fixtures 없음 — fixtures/README.md 참고");
        true
    }

    fn call(line: &str) -> Value {
        let resp = handle_request(line, &ctx()).expect("응답 있어야 함");
        serde_json::from_str(&resp).unwrap()
    }

    #[test]
    fn initialize_응답() {
        let v = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(
            v["result"]["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
        assert!(v["result"]["serverInfo"]["name"].is_string());
    }

    #[test]
    fn initialize_프로토콜_버전_협상() {
        // A supported version is echoed as-is.
        for version in SUPPORTED_PROTOCOL_VERSIONS {
            let v = call(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{version}"}}}}"#
            ));
            assert_eq!(v["result"]["protocolVersion"], version);
        }
        // An unsupported version gets the newest version in response.
        let v = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(
            v["result"]["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
        // A missing protocolVersion parameter also gets the newest version.
        let v = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        assert_eq!(
            v["result"]["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
    }

    #[test]
    fn 알림은_응답없음() {
        assert!(
            handle_request(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &ctx()
            )
            .is_none()
        );
    }

    #[test]
    fn 미지원_메서드_에러() {
        let v = call(r#"{"jsonrpc":"2.0","id":2,"method":"no_such_method"}"#);
        assert_eq!(v["error"]["code"], -32601);
    }

    #[test]
    fn tools_list_도구_노출() {
        let v = call(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
        let tools = v["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "hwp_info",
            "hwp_read",
            "hwp_grep",
            "hwp_render",
            "hwp_edit",
            "hwp_convert",
            "hwp_new",
            "hwp_compose",
            "hwp_template",
            "hwp_diff",
            "hwp_slots",
            "hwp_fill",
            "hwp_validate",
            "hwp_certify",
            "hwp_list_bookmarks",
        ] {
            assert!(names.contains(&expected), "{expected} 누락");
        }
    }

    #[test]
    fn tools_list_exposes_read_bounds_and_add_col_position() {
        let tools = tool_defs();
        let read = tools
            .iter()
            .find(|tool| tool["name"] == "hwp_read")
            .unwrap();
        assert_eq!(
            read["inputSchema"]["properties"]["max_bytes"]["maximum"],
            MAX_READ_BYTES
        );
        let edit = tools
            .iter()
            .find(|tool| tool["name"] == "hwp_edit")
            .unwrap();
        assert_eq!(
            edit["inputSchema"]["properties"]["add_col"]["items"]["properties"]["at"]["type"],
            "integer"
        );
        // #77: positioned/counted row+column insertion fields are exposed.
        let add_row = &edit["inputSchema"]["properties"]["add_row"]["items"]["properties"];
        for field in ["at", "count", "template_row"] {
            assert_eq!(add_row[field]["type"], "integer", "add_row.{field}");
        }
        assert_eq!(
            edit["inputSchema"]["properties"]["add_col"]["items"]["properties"]["count"]["minimum"],
            1
        );
        // #78: clone_table exposes source_table/anchor/text_mode.
        let clone_table = &edit["inputSchema"]["properties"]["clone_table"];
        assert_eq!(
            clone_table["items"]["required"],
            json!(["source_table", "anchor"])
        );
        assert_eq!(
            clone_table["items"]["properties"]["text_mode"]["enum"],
            json!(["blank", "keep"])
        );
        for name in ["hwp_render", "hwp_diff"] {
            let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
            let dpi = &tool["inputSchema"]["properties"]["dpi"];
            assert_eq!(dpi["minimum"], hwp_render::MIN_DPI);
            assert_eq!(dpi["maximum"], hwp_render::MAX_DPI);
        }
    }

    #[test]
    fn compose_dry_run_accepts_inline_spec_without_writing() {
        let output = std::env::temp_dir().join(format!(
            "hwp-compose-mcp-dry-run-{}-{}.hwpx",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let result = tool_compose(
            &json!({
                "spec": {
                    "version": "1.0",
                    "sections": [{
                        "blocks": [{
                            "type": "paragraph",
                            "runs": [{"type": "text", "text": "본문"}]
                        }]
                    }]
                },
                "output": output,
                "dry_run": true,
                "allow_visual_fallback": true
            }),
            &ctx(),
        )
        .expect("compose dry-run");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["dry_run"], true);
        assert_eq!(report["native"], true);
        assert!(!output.exists());
    }

    #[test]
    fn compose_v2_rejects_deprecated_global_fallback_policy() {
        let error = tool_compose(
            &json!({
                "spec": {
                    "version": "2.0",
                    "document": {
                        "version": "1.0",
                        "sections": [{"blocks": [{
                            "type": "paragraph",
                            "runs": [{"type": "text", "text": "본문"}]
                        }]}]
                    },
                    "visuals": []
                },
                "output": "out.hwpx",
                "dry_run": true,
                "allow_visual_fallback": true
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(error.contains("\"code\": \"policy_conflict\""), "{error}");
        assert!(error.contains("$.policy"), "{error}");
    }

    #[test]
    fn compose_schema_marks_global_fallback_policy_deprecated() {
        let compose = tool_defs()
            .into_iter()
            .find(|tool| tool["name"] == "hwp_compose")
            .unwrap();
        let property = &compose["inputSchema"]["properties"]["allow_visual_fallback"];
        assert_eq!(property["type"], "boolean");
        assert_eq!(property["deprecated"], true);
    }

    #[test]
    fn compose_rejects_unknown_argument() {
        let error = tool_compose(
            &json!({
                "spec": {"version": "1.0", "sections": []},
                "output": "out.hwpx",
                "unknown": true
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(error.contains("unknown"));
    }

    #[test]
    fn compose_binds_spec_image_assets_to_sandbox_roots() {
        let directory = std::env::temp_dir().join(format!(
            "hwp-mcp-compose-roots-{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("assets")).unwrap();
        std::fs::write(
            directory.join("assets/image.gif"),
            b"GIF89a\x02\x00\x01\x00",
        )
        .unwrap();
        let sandbox = ctx_with_roots(vec![std::fs::canonicalize(&directory).unwrap()]);
        let output = directory.join("out.hwpx");

        // A spec referencing assets below the sandbox root composes as usual (verifies roots plumbing).
        let result = tool_compose(
            &json!({
                "spec": {
                    "version": "1.0",
                    "sections": [{
                        "blocks": [{
                            "type": "image",
                            "path": "assets/image.gif",
                            "width_mm": 20,
                            "height_mm": 10,
                            "placement": "inline"
                        }]
                    }]
                },
                "base_dir": directory,
                "output": output,
                "dry_run": true
            }),
            &sandbox,
        )
        .expect("image under sandbox root composes");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["dry_run"], true);
        assert_eq!(report["images"], 1);

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn template_dry_run_has_cli_report_contract_and_rejects_unknown_argument() {
        let output = std::env::temp_dir().join(format!(
            "hwp-template-mcp-dry-run-{}-{}.hwpx",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let _ = std::fs::remove_file(&output);
        let result = tool_template(
            &json!({
                "template": {
                    "version": "1.0",
                    "variables": {"title": {"type": "string", "required": true}},
                    "source": {"mode": "compose", "document": {
                        "version": "1.0",
                        "sections": [{"blocks": [{
                            "type": "paragraph",
                            "runs": [{"type": "text", "text": {
                                "node": "value", "pointer": "/values/title", "as": "text"
                            }}]
                        }]}]
                    }}
                },
                "data": {"version": "1.0", "values": {"title": "MCP"}},
                "output": output,
                "dry_run": true
            }),
            &ctx(),
        )
        .expect("template dry-run");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["schema_version"], "1.0");
        assert_eq!(report["data_schema_version"], "1.0");
        assert_eq!(report["mode"], "compose");
        assert_eq!(report["template_validation"], "passed");
        assert_eq!(report["data_validation"], "passed");
        assert_eq!(report["semantic_validation"], "not_run");
        assert_eq!(report["package_validation"], "not_run");
        assert!(!output.exists());

        let error = tool_template(&json!({
            "template": {"version": "1.0", "variables": {}, "source": {"mode": "compose", "document": {}}},
            "data": {"version": "1.0", "values": {}},
            "output": "out.hwpx",
            "unknown": true
        }), &ctx())
        .unwrap_err();
        assert!(error.contains("unknown"));
    }

    #[test]
    fn mcp_render_and_diff_reject_non_finite_or_out_of_range_dpi_before_loading() {
        for dpi in [0.0, -1.0, 601.0, 1.0e300] {
            let args = json!({"path": "missing.hwpx", "input": "missing.hwpx", "ref": "missing.png", "dpi": dpi});
            let render = tool_render(&args, &ctx()).unwrap_err();
            let diff = tool_diff(&args, &ctx()).unwrap_err();
            assert!(render.contains("DPI는 유한한"), "{render}");
            assert!(diff.contains("DPI는 유한한"), "{diff}");
        }
        for args in [
            json!({"path": "missing.hwpx", "input": "missing.hwpx", "ref": "missing.png", "dpi": "600"}),
            json!({"path": "missing.hwpx", "input": "missing.hwpx", "ref": "missing.png", "page": -1}),
            json!({"path": "missing.hwpx", "input": "missing.hwpx", "ref": "missing.png", "page": "1"}),
        ] {
            assert!(tool_render(&args, &ctx()).is_err());
            assert!(tool_diff(&args, &ctx()).is_err());
        }
    }

    #[test]
    fn protocol_line_reader_rejects_oversized_request_without_unbounded_allocation() {
        let mut reader = std::io::Cursor::new(vec![b'a'; 17]);
        let mut line = String::new();
        let error = read_line_bounded(&mut reader, &mut line, 16).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(line.len() <= 17);
    }

    #[test]
    fn call_hwp_validate() {
        if skip_if_no_fixtures() {
            return;
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{{"name":"hwp_validate","arguments":{{"path":"{}"}}}}}}"#,
            fixture("hwpx/minimal.hwpx")
        );
        let v = call(&line);
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"valid\": true"), "유효해야: {text}");
    }

    #[test]
    fn call_hwp_read_json() {
        if skip_if_no_fixtures() {
            return;
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"hwp_read","arguments":{{"path":"{}","format":"plain"}}}}}}"#,
            fixture("hwp5/hello_world.hwp")
        );
        let v = call(&line);
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Hello"), "본문 추출: {text:?}");
    }

    #[test]
    fn call_hwp_render_이미지() {
        if skip_if_no_fixtures() {
            return;
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"hwp_render","arguments":{{"path":"{}","page":1,"dpi":96}}}}}}"#,
            fixture("hwp5/hello_world.hwp")
        );
        let v = call(&line);
        assert_eq!(v["result"]["isError"], false);
        let content = v["result"]["content"].as_array().unwrap();
        let img = content
            .iter()
            .find(|c| c["type"] == "image")
            .expect("이미지 콘텐츠");
        assert_eq!(img["mimeType"], "image/png");
        assert!(
            img["data"].as_str().unwrap().len() > 100,
            "base64 PNG 비어있음"
        );
    }

    #[test]
    fn call_hwp_list_bookmarks() {
        if skip_if_no_fixtures() {
            return;
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{{"name":"hwp_list_bookmarks","arguments":{{"path":"{}"}}}}}}"#,
            fixture("hwp5/bookmark.hwp")
        );
        let v = call(&line);
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("책갈피테스트"), "책갈피 이름: {text}");
    }

    #[test]
    fn call_잘못된_인자_오류() {
        let v = call(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"hwp_read","arguments":{}}}"#,
        );
        assert_eq!(v["result"]["isError"], true);
    }

    fn temp_file(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("hwp-cli-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join(name)
    }

    fn create_hwpx(path: &Path, markdown: &str) {
        crate::commands::new::execute(
            path,
            crate::commands::new::NewInput::Markdown {
                text: markdown,
                base_dir: None,
                roots: &[],
            },
            &[],
            None,
            false,
        )
        .unwrap();
    }

    #[test]
    fn mcp_mutations_share_cli_atomic_and_noop_contracts() {
        let source = temp_file("mutation-source.hwpx");
        let edit_destination = temp_file("mutation-edit.hwpx");
        let fill_destination = temp_file("mutation-fill.hwpx");
        let convert_destination = temp_file("mutation-convert.unsupported");
        create_hwpx(&source, "{{수신}} 본문");

        std::fs::write(&edit_destination, b"EDIT ORIGINAL").unwrap();
        let edit = tool_edit(
            &json!({
                "input": source,
                "output": edit_destination,
                "replace": [{"from": "없는본문", "to": "값"}]
            }),
            &ctx(),
        );
        assert!(edit.is_err(), "0건 편집은 MCP도 실패");
        assert_eq!(std::fs::read(&edit_destination).unwrap(), b"EDIT ORIGINAL");

        std::fs::write(&fill_destination, b"FILL ORIGINAL").unwrap();
        let fill = tool_fill(
            &json!({
                "input": source,
                "output": fill_destination,
                "values": {"없는키": "값"}
            }),
            &ctx(),
        );
        assert!(fill.is_err(), "0건 fill은 MCP도 실패");
        assert_eq!(std::fs::read(&fill_destination).unwrap(), b"FILL ORIGINAL");

        std::fs::write(&convert_destination, b"CONVERT ORIGINAL").unwrap();
        let convert = tool_convert(
            &json!({
                "input": source,
                "output": convert_destination
            }),
            &ctx(),
        );
        assert!(convert.is_err(), "미지원 확장자 변환은 실패");
        assert_eq!(
            std::fs::read(&convert_destination).unwrap(),
            b"CONVERT ORIGINAL"
        );

        for path in [
            &source,
            &edit_destination,
            &fill_destination,
            &convert_destination,
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_fill_parts_splices_part_blocks() {
        let template = temp_file("fill-parts-template.hwpx");
        create_hwpx(&template, "# 보고서\n\n{{본문}}");
        let part = temp_file("fill-parts-part.md");
        std::fs::write(
            &part,
            "부분 본문입니다.\n\n<table>\n<tr><td colspan=\"2\">가로병합</td></tr>\n\
             <tr><td>a</td><td>b</td></tr>\n</table>\n",
        )
        .unwrap();
        let out = temp_file("fill-parts-out.hwpx");
        let result = tool_fill(
            &json!({
                "input": template,
                "output": out,
                "values": {},
                "parts": {"본문": part.display().to_string()}
            }),
            &ctx(),
        );
        let content = result.expect("parts fill 성공");
        let text = content[0]["text"].as_str().unwrap_or_default();
        assert!(text.contains("\"parts\""), "리포트 모드: {text}");
        let doc = crate::commands::cat::load_document(&out).unwrap();
        let plain = doc.plain_text();
        assert!(plain.contains("부분 본문입니다."), "{plain}");
        assert!(plain.contains("가로병합"), "{plain}");
        for path in [&template, &part, &out] {
            let _ = std::fs::remove_file(path);
        }
    }

    /// #56: image references inside `hwp_new` markdown input and `hwp_fill` part files are
    /// bound to the `--root` sandbox — an outside-root reference fails the tool call.
    #[test]
    fn mcp_new_and_fill_bind_markdown_images_to_sandbox_roots() {
        let uniq = format!(
            "{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        );
        let root = std::env::temp_dir().join(format!("hwp-mcp-mdimg-root-{uniq}"));
        let outside = std::env::temp_dir().join(format!("hwp-mcp-mdimg-outside-{uniq}"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // Tiny PNG (magic + IHDR with 8x8) — the file only needs to exist for the sandbox check.
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(8u32.to_be_bytes());
        png.extend(8u32.to_be_bytes());
        png.extend([0u8; 8]);
        let outside_png = outside.join("out.png");
        std::fs::write(&outside_png, &png).unwrap();
        // Markdown link destinations treat `\` as an escape — forward slashes (Windows CI).
        let outside_ref = outside_png.display().to_string().replace('\\', "/");
        let sandbox = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);

        // hwp_new: markdown referencing an absolute image outside the roots fails closed.
        let new_out = root.join("new.hwpx");
        let err = tool_new(
            &json!({
                "markdown": format!("본문\n\n![x]({outside_ref})\n"),
                "output": new_out,
            }),
            &sandbox,
        )
        .unwrap_err();
        assert!(err.contains("샌드박스"), "{err}");
        assert!(!new_out.exists(), "실패 시 출력을 게시하지 않음");

        // hwp_fill: a part file (itself inside the roots) referencing an outside image fails.
        let template = root.join("template.hwpx");
        create_hwpx(&template, "{{본문}}");
        let part = root.join("part.md");
        std::fs::write(&part, format!("부분\n\n![x]({outside_ref})\n")).unwrap();
        let fill_out = root.join("fill.hwpx");
        let err = tool_fill(
            &json!({
                "input": template,
                "output": fill_out,
                "values": {},
                "parts": {"본문": part.display().to_string()}
            }),
            &sandbox,
        )
        .unwrap_err();
        assert!(err.contains("샌드박스"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn mcp_new_fails_closed_on_drop_and_preserves_destination() {
        let destination = temp_file("new-drop.hwpx");
        std::fs::write(&destination, b"ORIGINAL").unwrap();

        let mut doc = hwp_convert::from_markdown("본문");
        let paragraph = &mut doc.sections[0].paragraphs[0];
        let control_index = paragraph.controls.len() as u32;
        paragraph
            .controls
            .push(hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"zzzz",
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
            }));
        let insert_at = paragraph.chars.len().saturating_sub(1);
        paragraph.chars.insert(
            insert_at,
            hwp_model::HwpChar::ExtCtrl {
                code: hwp_model::ctrl_char::OBJECT,
                ctrl_id: *b"zzzz",
                payload: vec![0; 12],
                ctrl_index: Some(control_index),
            },
        );
        let document_json = hwp_convert::to_json(&doc, true, false).unwrap();
        let result = tool_new(
            &json!({
                "output": destination,
                "json": document_json,
            }),
            &ctx(),
        );
        assert!(
            result.as_ref().is_err_and(|error| {
                error.contains("보존 불가")
                    && error.contains("opaque_control_unrepresentable")
                    && !error.contains("zzzz")
            }),
            "DROP은 hard failure여야: {result:?}"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"ORIGINAL");
        let _ = std::fs::remove_file(destination);
    }

    #[test]
    fn mcp_partial_edit_returns_machine_readable_warnings() {
        let source = temp_file("partial-source.hwpx");
        let destination = temp_file("partial-destination.hwpx");
        create_hwpx(&source, "있는본문");
        let content = tool_edit(
            &json!({
                "input": source,
                "output": destination,
                "replace": [
                    {"from": "있는본문", "to": "바뀐본문"},
                    {"from": "없는본문", "to": "값"}
                ],
                "allow_partial": true
            }),
            &ctx(),
        )
        .expect("allow_partial MCP edit");
        let report: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["applied"], 1);
        assert!(
            report["warnings"]
                .as_array()
                .is_some_and(|warnings| !warnings.is_empty())
        );
        for path in [&source, &destination] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_add_row_col_positioned_counted() {
        // #77: MCP add_row/add_col accept at/count/template_row and run through the
        // same conversion primitives as the CLI.
        let source = temp_file("addrowcol-source.hwpx");
        let destination = temp_file("addrowcol-out.hwpx");
        create_hwpx(&source, "| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        tool_edit(
            &json!({
                "input": source,
                "output": destination,
                "add_row": [{"table": 0, "at": 1, "count": 2, "template_row": 0}],
                "add_col": [{"table": 0, "at": 1, "count": 2}]
            }),
            &ctx(),
        )
        .expect("positioned counted MCP edit");
        // Rows 1-2 and cols 1-2 were inserted into the 2x2 table (now 4x4); fill a
        // cell of an inserted row to prove the grid is addressable.
        let fill_destination = temp_file("addrowcol-fill.hwpx");
        tool_edit(
            &json!({
                "input": destination,
                "output": fill_destination,
                "set_cell": [{"table": 0, "row": 2, "col": 3, "text": "MCP신규"}]
            }),
            &ctx(),
        )
        .expect("set_cell into the inserted band");
        let content = tool_read(
            &json!({"path": fill_destination, "format": "plain"}),
            &ctx(),
        )
        .unwrap();
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("MCP신규"), "inserted cell filled: {text}");
        assert!(text.contains("가"), "original content kept: {text}");
        // count = 0 is rejected.
        let rejected = tool_edit(
            &json!({
                "input": source,
                "output": temp_file("addrowcol-reject.hwpx"),
                "add_row": [{"table": 0, "count": 0}]
            }),
            &ctx(),
        );
        assert!(rejected.is_err(), "count 0 is refused");
        let _ = std::fs::remove_file(temp_file("addrowcol-reject.hwpx"));
        for path in [&source, &destination, &fill_destination] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_clone_table_blank_default_and_keep_mode() {
        // #78: MCP clone_table runs through the same conversion primitive as the CLI.
        let source = temp_file("clonetbl-source.hwpx");
        let blank_out = temp_file("clonetbl-blank.hwpx");
        let keep_out = temp_file("clonetbl-keep.hwpx");
        create_hwpx(&source, "앵커문단\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // text_mode omitted → blank: structure cloned, cell text stripped.
        tool_edit(
            &json!({
                "input": source,
                "output": blank_out,
                "clone_table": [{"source_table": 0, "anchor": "앵커문단"}]
            }),
            &ctx(),
        )
        .expect("blank clone");
        let content = tool_read(&json!({"path": blank_out, "format": "plain"}), &ctx()).unwrap();
        let text = content[0]["text"].as_str().unwrap();
        assert_eq!(
            text.matches('가').count(),
            1,
            "blank clone strips cell text (source only): {text}"
        );
        // keep mode clones cell text as well.
        tool_edit(
            &json!({
                "input": source,
                "output": keep_out,
                "clone_table": [{"source_table": 0, "anchor": "앵커문단", "text_mode": "keep"}]
            }),
            &ctx(),
        )
        .expect("keep clone");
        let content = tool_read(&json!({"path": keep_out, "format": "plain"}), &ctx()).unwrap();
        let text = content[0]["text"].as_str().unwrap();
        assert_eq!(
            text.matches('가').count(),
            2,
            "cloned content present: {text}"
        );
        // An invalid text_mode is rejected with a bounded error.
        let rejected = tool_edit(
            &json!({
                "input": source,
                "output": temp_file("clonetbl-reject.hwpx"),
                "clone_table": [{"source_table": 0, "anchor": "앵커문단", "text_mode": "bogus"}]
            }),
            &ctx(),
        );
        assert!(rejected.is_err(), "invalid text_mode is refused");
        let _ = std::fs::remove_file(temp_file("clonetbl-reject.hwpx"));
        for path in [&source, &blank_out, &keep_out] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_read_is_utf8_bounded_and_pageable() {
        let source = temp_file("bounded-read.hwpx");
        create_hwpx(&source, "가나다라마바사");
        let first = tool_read(
            &json!({
                "path": source,
                "format": "plain",
                "max_bytes": 7
            }),
            &ctx(),
        )
        .unwrap();
        assert!(first[0]["text"].as_str().unwrap().len() <= 7);
        let metadata: Value = serde_json::from_str(first[1]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["truncated"], true);
        let next = metadata["next_offset"].as_u64().unwrap();
        let second = tool_read(
            &json!({
                "path": source,
                "format": "plain",
                "offset": next,
                "max_bytes": 7
            }),
            &ctx(),
        )
        .unwrap();
        assert!(!second[0]["text"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_structured_replace_preserves_cli_delimiters_as_data() {
        let source = temp_file("delimiter-source.hwpx");
        let destination = temp_file("delimiter-destination.hwpx");
        create_hwpx(&source, "A=>B");

        tool_edit(
            &json!({
                "input": source,
                "output": destination,
                "replace": [{"from": "A=>B", "to": "X=Y=>Z"}]
            }),
            &ctx(),
        )
        .expect("구조화 치환");

        let edited = load_document(&destination).unwrap();
        assert!(edited.plain_text().contains("X=Y=>Z"));
        assert!(!edited.plain_text().contains("A=>B"));
        for path in [&source, &destination] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_structured_metadata_and_paragraphs_keep_delimiters() {
        let source = temp_file("typed-structural-source.hwpx");
        let destination = temp_file("typed-structural-destination.hwpx");
        create_hwpx(&source, "Anchor=>Here");

        tool_edit(
            &json!({
                "input": source,
                "output": destination,
                "set_meta": [{"key": "title", "value": "A=B=>C"}],
                "insert_para": [{
                    "anchor": "Anchor=>Here",
                    "text": "New=>Text=1"
                }]
            }),
            &ctx(),
        )
        .expect("구조화 metadata/문단 편집");

        let edited = load_document(&destination).unwrap();
        assert_eq!(edited.metadata.title.as_deref(), Some("A=B=>C"));
        assert!(edited.plain_text().contains("New=>Text=1"));
        for path in [&source, &destination] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_render_rasterizes_only_selected_page_and_reports_total_pages() {
        let source = temp_file("selected-render.hwpx");
        let mut doc = hwp_convert::from_markdown("첫 쪽\n\n둘째 쪽\n\n셋째 쪽\n");
        doc.sections[0].paragraphs[1].header.break_type |= 0x04;
        doc.sections[0].paragraphs[2].header.break_type |= 0x04;
        hwpx::write_document(&doc, &source).unwrap();

        let content = tool_render(&json!({"path": source, "page": 2, "dpi": 36}), &ctx()).unwrap();
        assert!(
            content[0]["text"]
                .as_str()
                .is_some_and(|summary| summary.contains("페이지 2/3 렌더"))
        );
        assert_eq!(content[1]["type"], "image");
        let _ = std::fs::remove_file(source);
    }

    /// Creates (base, root, outside) directories for sandbox tests.
    /// Windows-CI compatible: uses only the real temp dir, and root is canonicalized before entering ctx.
    fn sandbox_dirs(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("hwp-cli-mcp-sandbox-{tag}-{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        (base, root, outside)
    }

    #[test]
    fn sandbox_루트없으면_검사없이_통과() {
        let ctx = ctx_with_roots(Vec::new());
        // Nonexistent paths and '..' pass through with the previous behavior.
        let read = checked_read_path(&ctx, "no/such/file.hwpx").unwrap();
        assert_eq!(read, PathBuf::from("no/such/file.hwpx"));
        let write = checked_write_path(&ctx, "../out.hwpx").unwrap();
        assert_eq!(write, PathBuf::from("../out.hwpx"));
    }

    #[test]
    fn sandbox_루트밖_읽기_거부() {
        let (base, root, outside) = sandbox_dirs("read");
        let document = outside.join("doc.hwpx");
        create_hwpx(&document, "본문");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_read(&json!({"path": document}), &ctx).unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_루트밖_쓰기_거부() {
        let (base, root, outside) = sandbox_dirs("write");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "{{이름}}");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let output = outside.join("out.hwpx");
        let error = tool_fill(
            &json!({
                "input": source,
                "output": output,
                "values": {"이름": "값"}
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        assert!(!output.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_상위경로_쓰기_거부() {
        let (base, root, _outside) = sandbox_dirs("dotdot");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "{{이름}}");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_fill(
            &json!({
                "input": source,
                "output": root.join("sub").join("..").join("escape.hwpx"),
                "values": {"이름": "값"}
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(error.contains(".."), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn sandbox_심볼링크_덮어쓰기_우회_거부() {
        let (base, root, outside) = sandbox_dirs("symlink");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "{{이름}}");
        let victim = outside.join("victim.hwpx");
        std::fs::write(&victim, b"VICTIM").unwrap();
        let link = root.join("link.hwpx");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_fill(
            &json!({
                "input": source,
                "output": link,
                "values": {"이름": "값"}
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"VICTIM");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_루트안_읽기쓰기_허용() {
        let (base, root, _outside) = sandbox_dirs("inside");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "{{이름}}");
        let output = root.join("out.hwpx");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        tool_fill(
            &json!({
                "input": source,
                "output": output,
                "values": {"이름": "값"}
            }),
            &ctx,
        )
        .expect("루트 안 fill");
        let doc = load_document(&output).unwrap();
        assert!(doc.plain_text().contains("값"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_호출당_font_dir도_검사() {
        let (base, root, outside) = sandbox_dirs("fontdir");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "본문");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_render(&json!({"path": source, "font_dir": outside}), &ctx).unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_edit_중첩_이미지경로_거부() {
        let (base, root, outside) = sandbox_dirs("edit-image");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "앵커");
        let image = outside.join("image.png");
        std::fs::write(&image, b"PNG").unwrap();
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_edit(
            &json!({
                "input": source,
                "output": root.join("out.hwpx"),
                "insert_image": [{"anchor": "앵커", "path": image}]
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Counts controls matched across body, table cells, and text boxes recursively (for typed-edit round-trip checks).
    fn count_controls(
        doc: &hwp_model::Document,
        matches: fn(&hwp_model::Control) -> bool,
    ) -> usize {
        fn in_para(para: &hwp_model::Paragraph, matches: fn(&hwp_model::Control) -> bool) -> usize {
            let mut n = 0;
            for control in &para.controls {
                if matches(control) {
                    n += 1;
                }
                match control {
                    hwp_model::Control::Table(table) => {
                        for cell in &table.cells {
                            for p in &cell.paragraphs {
                                n += in_para(p, matches);
                            }
                        }
                    }
                    hwp_model::Control::Generic(g) => {
                        for list in &g.paragraph_lists {
                            for p in &list.paragraphs {
                                n += in_para(p, matches);
                            }
                        }
                    }
                    _ => {}
                }
            }
            n
        }
        doc.sections
            .iter()
            .flat_map(|section| &section.paragraphs)
            .map(|para| in_para(para, matches))
            .sum()
    }

    #[test]
    fn tools_list_exposes_typed_edit_parity_args() {
        let tools = tool_defs();
        let edit = tools
            .iter()
            .find(|tool| tool["name"] == "hwp_edit")
            .unwrap();
        let properties = &edit["inputSchema"]["properties"];
        for key in [
            "add_table",
            "set_para",
            "set_page",
            "delete_image",
            "delete_table",
            "delete_field",
            "delete_bookmark",
        ] {
            assert!(
                properties.get(key).is_some(),
                "hwp_edit 스키마에 {key} 누락"
            );
        }
        // set_page is a single object; the rest are arrays.
        assert_eq!(properties["set_page"]["type"], "object");
        assert_eq!(properties["add_table"]["type"], "array");
    }

    #[test]
    fn mcp_typed_add_table_then_delete_table_round_trip() {
        let source = temp_file("typed-add-table-source.hwpx");
        let mid = temp_file("typed-add-table-mid.hwpx");
        let out = temp_file("typed-add-table-out.hwpx");
        create_hwpx(&source, "머리말\n\n끝말");

        tool_edit(
            &json!({
                "input": source,
                "output": mid,
                "add_table": [{"anchor": "머리말", "rows": [["가", "나"], ["1", "2"]]}]
            }),
            &ctx(),
        )
        .expect("add_table 편집");
        let doc = load_document(&mid).unwrap();
        assert_eq!(
            count_controls(&doc, |c| matches!(c, hwp_model::Control::Table(_))),
            1,
            "표가 하나 있어야 한다"
        );
        let plain = doc.plain_text();
        assert!(
            plain.contains('가') && plain.contains('1'),
            "셀 텍스트: {plain}"
        );

        tool_edit(
            &json!({
                "input": mid,
                "output": out,
                "delete_table": [{"index": 0}]
            }),
            &ctx(),
        )
        .expect("delete_table 편집");
        let doc = load_document(&out).unwrap();
        assert_eq!(
            count_controls(&doc, |c| matches!(c, hwp_model::Control::Table(_))),
            0,
            "표가 삭제되어야 한다"
        );
        let plain = doc.plain_text();
        assert!(
            !plain.contains('가') && plain.contains("끝말"),
            "결과: {plain}"
        );
        for path in [&source, &mid, &out] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_set_para_and_set_page_round_trip() {
        let source = temp_file("typed-set-para-source.hwpx");
        let out = temp_file("typed-set-para-out.hwpx");
        create_hwpx(&source, "본문 문단입니다.");

        tool_edit(
            &json!({
                "input": source,
                "output": out,
                "set_para": [{"pattern": "본문", "line_spacing_pct": 130, "indent_mm": 5}],
                "set_page": {"width_mm": 200, "orientation": "landscape"}
            }),
            &ctx(),
        )
        .expect("set_para/set_page 편집");

        let doc = load_document(&out).unwrap();
        let para = doc.sections[0]
            .paragraphs
            .iter()
            .find(|p| p.plain_text().contains("본문"))
            .expect("본문 문단");
        let shape = &doc.header.para_shapes[para.para_shape.0 as usize];
        assert_eq!(shape.line_spacing_type, 0, "비율 줄간격");
        assert_eq!(shape.line_spacing, 130);
        // The IR uses double-HWPUNIT units (hwp5 PARA_SHAPE).
        assert_eq!(shape.indent, 2 * crate::commands::edit::mm_to_hwpunit(5.0));

        let page = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                hwp_model::Control::SectionDef(secd) => secd.page.as_ref(),
                _ => None,
            })
            .expect("구역 정의의 용지 정보");
        assert_eq!(page.width.0, crate::commands::edit::mm_to_hwpunit(200.0));
        assert_eq!(page.attr & 1, 1, "landscape 비트");
        for path in [&source, &out] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_delete_image_round_trip() {
        let source = temp_file("typed-delete-image-source.hwpx");
        let mid = temp_file("typed-delete-image-mid.hwpx");
        let out = temp_file("typed-delete-image-out.hwpx");
        let png = temp_file("typed-delete-image.png");
        create_hwpx(&source, "사진: 여기");
        // Minimal PNG header (IHDR 96x96) — insert_image only parses the size.
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend([0, 0, 0, 13]);
        bytes.extend(b"IHDR");
        bytes.extend(96u32.to_be_bytes());
        bytes.extend(96u32.to_be_bytes());
        bytes.extend([0u8; 8]);
        std::fs::write(&png, &bytes).unwrap();

        tool_edit(
            &json!({
                "input": source,
                "output": mid,
                "insert_image": [{"anchor": "사진:", "path": png}]
            }),
            &ctx(),
        )
        .expect("insert_image 편집");
        let doc = load_document(&mid).unwrap();
        assert_eq!(
            count_controls(&doc, |c| matches!(c, hwp_model::Control::Picture(_))),
            1,
            "그림이 하나 있어야 한다"
        );

        tool_edit(
            &json!({
                "input": mid,
                "output": out,
                "delete_image": [{"anchor": "사진:"}]
            }),
            &ctx(),
        )
        .expect("delete_image 편집");
        let doc = load_document(&out).unwrap();
        assert_eq!(
            count_controls(&doc, |c| matches!(c, hwp_model::Control::Picture(_))),
            0,
            "그림이 삭제되어야 한다"
        );
        for path in [&source, &mid, &out, &png] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_delete_field_and_bookmark_round_trip() {
        let source = temp_file("typed-delete-field-source.hwpx");
        let mid = temp_file("typed-delete-field-mid.hwpx");
        let out = temp_file("typed-delete-field-out.hwpx");
        create_hwpx(&source, "참조: 본문");

        tool_edit(
            &json!({
                "input": source,
                "output": mid,
                "create_field": [{"anchor": "참조:", "name": "수신", "value": "홍길동"}],
                "create_bookmark": [{"anchor": "참조:", "name": "참조점"}]
            }),
            &ctx(),
        )
        .expect("create_field/create_bookmark 편집");
        let doc = load_document(&mid).unwrap();
        assert!(
            hwp_convert::list_fields(&doc)
                .iter()
                .any(|f| f.name.as_deref() == Some("수신")),
            "필드가 생성되어야 한다"
        );
        assert!(
            hwp_convert::list_bookmarks(&doc)
                .iter()
                .any(|b| b.name == "참조점"),
            "책갈피가 생성되어야 한다"
        );

        let content = tool_edit(
            &json!({
                "input": mid,
                "output": out,
                "delete_field": [{"name": "수신"}],
                "delete_bookmark": [{"name": "참조점"}]
            }),
            &ctx(),
        )
        .expect("delete_field/delete_bookmark 편집");
        let report: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["applied"], 2);
        let doc = load_document(&out).unwrap();
        assert!(
            !hwp_convert::list_fields(&doc)
                .iter()
                .any(|f| f.name.as_deref() == Some("수신")),
            "필드가 삭제되어야 한다"
        );
        assert!(
            !hwp_convert::list_bookmarks(&doc)
                .iter()
                .any(|b| b.name == "참조점"),
            "책갈피가 삭제되어야 한다"
        );
        for path in [&source, &mid, &out] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_edit_validation_errors() {
        // Boundary-validation errors fire before any file is read (input/output are dummy paths).
        let dummy = json!({"input": "in.hwpx", "output": "out.hwpx"});

        let mut args = dummy.clone();
        args["delete_table"] = json!([{"index": 0, "anchor": "표"}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("delete_table"), "{error}");

        let mut args = dummy.clone();
        args["delete_table"] = json!([{}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("delete_table"), "{error}");

        let mut args = dummy.clone();
        args["set_para"] =
            json!([{"pattern": "본문", "line_spacing_pct": 130, "line_spacing_pt": 14}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("line_spacing"), "{error}");

        let mut args = dummy.clone();
        args["set_page"] = json!({"orientation": "sideways"});
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("용지 방향"), "{error}");

        let mut args = dummy.clone();
        args["set_page"] = json!([{"width_mm": 200}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("객체"), "{error}");

        let mut args = dummy.clone();
        args["add_table"] = json!([{"anchor": "머리말", "rows": [["a"], [1]]}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("add_table"), "{error}");

        let mut args = dummy.clone();
        args["add_table"] = json!([{"anchor": "머리말", "rows": "x"}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("add_table"), "{error}");

        // Empty rows have the right shape but no content — the library rejects them and that error propagates.
        let source = temp_file("typed-add-table-empty-source.hwpx");
        let out = temp_file("typed-add-table-empty-out.hwpx");
        create_hwpx(&source, "머리말");
        let error = tool_edit(
            &json!({
                "input": source,
                "output": out,
                "add_table": [{"anchor": "머리말", "rows": []}]
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(error.contains("표 행 데이터"), "{error}");
        assert!(!out.exists(), "실패한 편집은 출력을 게시하지 않는다");
        let _ = std::fs::remove_file(&source);
    }

    // ---- read/convert/render parity + hwp_grep (WI-4) ----

    #[test]
    fn mcp_read_html_and_csv_formats() {
        let source = temp_file("read-html-csv.hwpx");
        create_hwpx(
            &source,
            "본문\n\n<table>\n<tr><td>가</td><td>나</td></tr>\n</table>\n",
        );
        let html = tool_read(&json!({"path": source, "format": "html"}), &ctx()).unwrap();
        let html_text = html[0]["text"].as_str().unwrap();
        assert!(html_text.contains('<'), "html 출력: {html_text}");
        let csv = tool_read(&json!({"path": source, "format": "csv"}), &ctx()).unwrap();
        let csv_text = csv[0]["text"].as_str().unwrap();
        assert!(
            csv_text.contains('가') && csv_text.contains('나'),
            "csv 출력: {csv_text}"
        );
        let _ = std::fs::remove_file(source);
    }

    /// Builds an IR containing header/hidden-note controls (saved as .json, load_document
    /// deserializes it verbatim, so it round-trips without writer loss).
    fn create_ir_json_with_hidden_and_header(path: &Path) {
        let mut doc = hwp_convert::from_markdown("본문\n\n숨은메모\n\n머리말텍스트");
        let hidden_para = doc.sections[0].paragraphs.remove(1);
        let header_para = doc.sections[0].paragraphs.remove(1);
        let body = &mut doc.sections[0].paragraphs[0];
        // from_markdown paragraphs already carry controls (section definitions, etc.),
        // so indexes are captured at push time.
        let hidden_index = body.controls.len() as u32;
        body.controls
            .push(hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"hcnt",
                data: Vec::new(),
                paragraph_lists: vec![hwp_model::ParagraphList {
                    header_data: Vec::new(),
                    paragraphs: vec![hidden_para],
                }],
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
            }));
        let header_index = body.controls.len() as u32;
        body.controls
            .push(hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"head",
                data: Vec::new(),
                paragraph_lists: vec![hwp_model::ParagraphList {
                    header_data: Vec::new(),
                    paragraphs: vec![header_para],
                }],
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
            }));
        body.chars.insert(
            0,
            hwp_model::HwpChar::ExtCtrl {
                code: hwp_model::ctrl_char::HEADER_FOOTER,
                ctrl_id: *b"head",
                payload: vec![0; 12],
                ctrl_index: Some(header_index),
            },
        );
        body.chars.insert(
            0,
            hwp_model::HwpChar::ExtCtrl {
                code: hwp_model::ctrl_char::HIDDEN_COMMENT,
                ctrl_id: *b"hcnt",
                payload: vec![0; 12],
                ctrl_index: Some(hidden_index),
            },
        );
        let document_json = hwp_convert::to_json(&doc, true, false).unwrap();
        std::fs::write(path, document_json).unwrap();
    }

    #[test]
    fn mcp_read_with_hidden_and_header_footer_flags() {
        let source = temp_file("read-with-flags.json");
        create_ir_json_with_hidden_and_header(&source);

        let plain = tool_read(&json!({"path": source, "format": "plain"}), &ctx()).unwrap();
        let text = plain[0]["text"].as_str().unwrap();
        assert!(!text.contains("숨은메모"), "기본은 숨은 설명 제외: {text}");
        assert!(!text.contains("머리말텍스트"), "기본은 머리말 제외: {text}");

        let hidden = tool_read(
            &json!({"path": source, "format": "plain", "with_hidden": true}),
            &ctx(),
        )
        .unwrap();
        let text = hidden[0]["text"].as_str().unwrap();
        assert!(text.contains("숨은메모"), "with_hidden: {text}");
        assert!(!text.contains("머리말텍스트"), "with_hidden만: {text}");

        let header = tool_read(
            &json!({"path": source, "format": "markdown", "with_header_footer": true}),
            &ctx(),
        )
        .unwrap();
        let text = header[0]["text"].as_str().unwrap();
        assert!(text.contains("머리말텍스트"), "with_header_footer: {text}");
        assert!(!text.contains("숨은메모"), "with_header_footer만: {text}");
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_read_with_segments_envelope_and_absolute_offsets() {
        let source = temp_file("read-segments.hwpx");
        create_hwpx(&source, "가나다\n\n마바사");

        let full = tool_read(
            &json!({"path": source, "format": "markdown", "with_segments": true}),
            &ctx(),
        )
        .unwrap();
        let envelope: Value =
            serde_json::from_str(full[0]["text"].as_str().unwrap()).expect("JSON 봉투");
        let markdown = envelope["markdown"].as_str().unwrap();
        assert!(
            markdown.contains("가나다") && markdown.contains("마바사"),
            "{markdown}"
        );
        let full_segments = envelope["segments"].as_array().unwrap();
        assert!(
            full_segments.len() >= 2,
            "문단별 세그먼트: {full_segments:?}"
        );

        // A 2-character window from the middle of the second paragraph ("바") — segment offsets must stay absolute against the full markdown.
        let offset = markdown.find("바").unwrap();
        let window = tool_read(
            &json!({
                "path": source,
                "format": "markdown",
                "with_segments": true,
                "offset": offset,
                "max_bytes": 6
            }),
            &ctx(),
        )
        .unwrap();
        let windowed: Value = serde_json::from_str(window[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(windowed["markdown"].as_str().unwrap(), "바사");
        let window_segments = windowed["segments"].as_array().unwrap();
        assert!(!window_segments.is_empty(), "창과 교차하는 세그먼트");
        assert!(
            window_segments.len() < full_segments.len(),
            "창 밖 세그먼트는 걸러야: {window_segments:?}"
        );
        // Proof of absolute offsets: window-relative remapping would put start near 0,
        // but absolute values point at the character position in the full markdown.
        let char_start = markdown[..offset].chars().count();
        let char_end = char_start + 2;
        for segment in window_segments {
            let start = segment["start"].as_u64().unwrap() as usize;
            let end = segment["end"].as_u64().unwrap() as usize;
            assert!(
                start < char_end && end > char_start,
                "창 [{char_start}, {char_end})과 교차해야: {segment}"
            );
            assert!(
                full_segments.contains(segment),
                "오프셋이 전체 기준 절대값으로 동일해야: {segment}"
            );
        }
        // Pagination metadata is kept as the second content item.
        let metadata: Value = serde_json::from_str(window[1]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["truncated"], true);
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_read_with_segments_requires_markdown() {
        let source = temp_file("read-segments-reject.hwpx");
        create_hwpx(&source, "본문");
        for format in ["plain", "json", "html", "csv"] {
            let error = tool_read(
                &json!({"path": source, "format": format, "with_segments": true}),
                &ctx(),
            )
            .unwrap_err();
            assert!(error.contains("markdown"), "{format}: {error}");
        }
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_convert_to_overrides_extension_and_accepts_markdown_options() {
        let source = temp_file("convert-to-source.hwpx");
        let output = temp_file("convert-to-output.txt");
        create_hwpx(&source, "본문");
        // to=json wins over the .txt extension (same precedence as the CLI --to).
        tool_convert(
            &json!({
                "input": source,
                "output": output,
                "to": "json",
                "media_dir": "convert-to-media",
                "with_header_footer": true,
                "with_hidden": true
            }),
            &ctx(),
        )
        .expect("to 명시 변환");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).expect("JSON 출력");
        assert!(written["sections"].is_array(), "IR JSON: {written}");
        for path in [&source, &output] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_convert_request_merges_font_dirs_and_maps_args() {
        // Seam verified without rendering: a pdf-target conversion receives the startup/per-call font directories.
        let mut sandbox_ctx = ctx();
        sandbox_ctx.font_dirs = vec![PathBuf::from("/launch-fonts")];
        let request = convert_request(
            &json!({
                "input": "in.hwpx",
                "output": "out.pdf",
                "to": "pdf",
                "font_dir": "/call-fonts",
                "media_dir": "media",
                "with_header_footer": true,
                "with_hidden": true
            }),
            &sandbox_ctx,
        )
        .expect("convert_request");
        assert!(matches!(request.to, Some(hwp_cli::cli::ConvertFormat::Pdf)));
        assert_eq!(
            request.font_dirs,
            vec![PathBuf::from("/launch-fonts"), PathBuf::from("/call-fonts")],
            "PDF 변환은 병합된 폰트 디렉터리를 받아야 한다"
        );
        assert_eq!(request.media_dir, Some(PathBuf::from("media")));
        assert!(request.with_header_footer && request.with_hidden);

        let error = convert_request(
            &json!({"input": "in.hwpx", "output": "out.x", "to": "bogus"}),
            &ctx(),
        )
        .unwrap_err();
        assert!(error.contains("to"), "{error}");
    }

    #[test]
    fn mcp_convert_media_dir_is_sandbox_checked() {
        let (base, root, outside) = sandbox_dirs("convert-media");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "본문");
        let sandbox = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = convert_request(
            &json!({
                "input": source,
                "output": root.join("out.md"),
                "media_dir": outside.join("media")
            }),
            &sandbox,
        )
        .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Synthetic 3-page document (same break_type trick as the existing mcp_render_rasterizes test).
    fn create_three_page_hwpx(path: &Path) {
        let mut doc = hwp_convert::from_markdown("첫 쪽\n\n둘째 쪽\n\n셋째 쪽\n");
        doc.sections[0].paragraphs[1].header.break_type |= 0x04;
        doc.sections[0].paragraphs[2].header.break_type |= 0x04;
        hwpx::write_document(&doc, path).unwrap();
    }

    #[test]
    fn mcp_render_rejects_conflicting_or_missing_page_args() {
        let source = temp_file("render-args.hwpx");
        // The document must have 3 pages for the "1-2" selection to be genuinely multi-page (parse_pages clamps to the page count).
        create_three_page_hwpx(&source);
        // page and pages are mutually exclusive.
        let error =
            tool_render(&json!({"path": source, "page": 1, "pages": "1-2"}), &ctx()).unwrap_err();
        assert!(error.contains("pages"), "{error}");
        // svg/pdf require output_path.
        for format in ["svg", "pdf"] {
            let error =
                tool_render(&json!({"path": source, "format": format}), &ctx()).unwrap_err();
            assert!(error.contains("output_path"), "{format}: {error}");
        }
        // Multi-page png requires output_path too.
        let error =
            tool_render(&json!({"path": source, "pages": "1-2", "dpi": 36}), &ctx()).unwrap_err();
        assert!(error.contains("output_path"), "{error}");
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_render_multi_page_png_writes_numbered_files_and_metadata() {
        let source = temp_file("render-multi-source.hwpx");
        let output = temp_file("render-multi.png");
        create_three_page_hwpx(&source);
        let content = tool_render(
            &json!({"path": source, "pages": "2-3", "dpi": 36, "output_path": output}),
            &ctx(),
        )
        .expect("다중 페이지 png");
        // Unlike the base64 path, this returns only JSON metadata without image content.
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        let metadata: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["pages"], json!([2, 3]));
        assert_eq!(metadata["dpi"].as_f64(), Some(36.0));
        let page2 = output.with_file_name("render-multi-2.png");
        let page3 = output.with_file_name("render-multi-3.png");
        let files: Vec<PathBuf> = metadata["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| PathBuf::from(f.as_str().unwrap()))
            .collect();
        assert_eq!(files, vec![page2.clone(), page3.clone()]);
        for path in [&page2, &page3] {
            let bytes = std::fs::read(path).expect("페이지 파일 존재");
            assert!(
                bytes.starts_with(b"\x89PNG"),
                "PNG 매직: {}",
                path.display()
            );
        }
        assert!(!output.exists(), "다중 페이지는 번호 파일만 쓴다");
        for path in [&source, &page2, &page3] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_render_svg_and_pdf_write_files_with_output_path() {
        let source = temp_file("render-vector-source.hwpx");
        create_three_page_hwpx(&source);

        // A single-page svg is written to output_path as-is.
        let svg = temp_file("render-single.svg");
        let content = tool_render(
            &json!({"path": source, "format": "svg", "dpi": 36, "output_path": svg}),
            &ctx(),
        )
        .expect("svg 단일 페이지");
        let metadata: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["pages"], json!([1]));
        assert_eq!(metadata["files"], json!([svg.to_str().unwrap()]));
        let bytes = std::fs::read(&svg).unwrap();
        assert!(
            bytes.windows(4).any(|w| w == b"<svg"),
            "SVG 마크업: {}",
            svg.display()
        );

        // pdf writes the selected pages as a single multi-page file.
        let pdf = temp_file("render-all.pdf");
        let content = tool_render(
            &json!({"path": source, "format": "pdf", "pages": "all", "dpi": 36, "output_path": pdf}),
            &ctx(),
        )
        .expect("pdf 전체 페이지");
        let metadata: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["pages"], json!([1, 2, 3]));
        assert_eq!(metadata["files"], json!([pdf.to_str().unwrap()]));
        let bytes = std::fs::read(&pdf).unwrap();
        assert!(bytes.starts_with(b"%PDF"), "PDF 매직: {}", pdf.display());
        for path in [&source, &svg, &pdf] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_grep_match_zero_match_and_ignore_case() {
        let source = temp_file("grep-source.hwpx");
        create_hwpx(&source, "사과 바나나\n\n둘째 사과\n\nHello World");

        let content =
            tool_grep(&json!({"path": source, "pattern": "사과"}), &ctx()).expect("grep 매칭");
        let result: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["count"], 2);
        assert_eq!(result["truncated"], false);
        assert_eq!(result["matches"].as_array().unwrap().len(), 2);

        // Zero matches are a normal result, not an error (isError=false).
        let zero = call_tool(
            "hwp_grep",
            &json!({"path": source, "pattern": "없는문구"}),
            &ctx(),
        );
        assert_eq!(zero["isError"], false);
        let result: Value =
            serde_json::from_str(zero["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["count"], 0);
        assert_eq!(result["matches"].as_array().unwrap().len(), 0);

        // ignore_case ignores letter case.
        let content = tool_grep(
            &json!({"path": source, "pattern": "hello", "ignore_case": true}),
            &ctx(),
        )
        .unwrap();
        let result: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["count"], 1, "ignore_case 매칭: {result}");
        let content = tool_grep(&json!({"path": source, "pattern": "hello"}), &ctx()).unwrap();
        let result: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["count"], 0, "대소문자 구분: {result}");
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_grep_truncation_cap_logic() {
        let matches: Vec<String> = (0..201).map(|i| format!("m{i}")).collect();
        let capped = grep_result(matches);
        assert_eq!(capped["count"], 201, "count는 절단 전 전체 개수");
        assert_eq!(capped["truncated"], true);
        assert_eq!(
            capped["matches"].as_array().unwrap().len(),
            MAX_GREP_MATCHES
        );
        assert_eq!(MAX_GREP_MATCHES, 200);

        // Counts at or below the cap pass through unchanged.
        let uncapped = grep_result_capped(vec!["a".into(), "b".into()], 2);
        assert_eq!(uncapped["count"], 2);
        assert_eq!(uncapped["truncated"], false);
        let capped = grep_result_capped(vec!["a".into(), "b".into(), "c".into()], 2);
        assert_eq!(capped["count"], 3);
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["matches"].as_array().unwrap().len(), 2);
    }
}
