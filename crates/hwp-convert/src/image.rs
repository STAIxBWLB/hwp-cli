//! 이미지 삽입 — 앵커 텍스트 뒤에 새 그림(Picture)을 꽂는다.
//!
//! writer가 빈-extras Picture에 hwp5 도형 레코드(SHAPE_COMPONENT + 그림)를 합성하므로
//! (hwpx→hwp와 동일한 검증된 경로), 여기서는 최소 Picture + BinStream + 앵커 ExtCtrl만
//! 만든다. bin_ref=ItemRef(name)로 hwpx 출력·hwp5 합성·렌더가 모두 바이트를 해석한다.

use std::path::{Path, PathBuf};

use hwp_model::{BinRef, BinStream, Control, Document, HwpChar, HwpUnit, Paragraph, Picture};

use crate::edit::{adjust_runs, find_match, utf16_len};
use crate::field::{relink_ctrl_index, rev_payload};

/// gso 개체(그림/표 등) 확장 컨트롤 문자 코드.
const GSO_CODE: u16 = 11;
/// mm → HWPUNIT(1/7200 inch = 1/100 pt): 7200/25.4.
const MM_TO_HWPUNIT: f32 = 283.464_57;
/// PageDef 없을 때 본문 폭 기본값(HWPUNIT, ≈400pt).
const DEFAULT_CONTENT_WIDTH: i32 = 40_000;

/// 삽입 이미지 표시 크기.
pub enum ImageSize {
    /// 원본 픽셀 크기(96 DPI 기준), 본문 폭 초과 시 비례 축소.
    Natural,
    /// 밀리미터 지정(너비, 높이).
    Mm(f32, f32),
}

/// PNG/GIF/BMP/JPEG 헤더에서 픽셀 (너비, 높이)를 읽는다(무의존 헤더 파싱).
pub fn image_pixel_size(data: &[u8]) -> Option<(u32, u32)> {
    // PNG: IHDR 폭/높이 at 16..24 (big-endian)
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") {
        let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
        return Some((w, h));
    }
    // GIF: Logical Screen Descriptor at 6..10 (little-endian)
    if data.len() >= 10 && data.starts_with(b"GIF") {
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }
    // BMP: BITMAPINFOHEADER 폭/높이 at 18..26 (little-endian)
    if data.len() >= 26 && data.starts_with(b"BM") {
        let w = i32::from_le_bytes(data[18..22].try_into().ok()?);
        let h = i32::from_le_bytes(data[22..26].try_into().ok()?);
        return Some((w.unsigned_abs(), h.unsigned_abs()));
    }
    // JPEG: SOF 마커(0xFFC0~0xFFCF, C4/C8/CC 제외)에서 높이·너비
    if data.len() >= 4 && data[0] == 0xFF && data[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC
            {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            let seg = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            if seg < 2 {
                break;
            }
            i += 2 + seg;
        }
    }
    None
}

/// 매직 바이트로 이미지 (확장자, MIME)를 판별한다(md/html 이미지 복원용).
/// 알 수 없으면 `("bin", "application/octet-stream")` — 스펙상 미지 포맷은 `.bin`.
pub fn image_kind(data: &[u8]) -> (&'static str, &'static str) {
    match data {
        [0x89, b'P', b'N', b'G', ..] => ("png", "image/png"),
        [0xFF, 0xD8, ..] => ("jpg", "image/jpeg"),
        [b'G', b'I', b'F', ..] => ("gif", "image/gif"),
        [b'B', b'M', ..] => ("bmp", "image/bmp"),
        _ => ("bin", "application/octet-stream"),
    }
}

/// 문서 첫 구역의 본문 폭(HWPUNIT). PageDef 없으면 A4 근사 기본값.
fn content_width(doc: &Document) -> i32 {
    doc.sections
        .first()
        .and_then(|s| s.section_def())
        .and_then(|sd| sd.page)
        .map(|p| (p.width.0 - p.margin_left.0 - p.margin_right.0).max(1))
        .unwrap_or(DEFAULT_CONTENT_WIDTH)
}

/// 확장자(소문자) 추출·검증. 지원: png/jpg/jpeg/bmp/gif.
fn ext_of(path: &Path) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .ok_or_else(|| format!("이미지 확장자를 알 수 없습니다: {}", path.display()))?;
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "bmp" | "gif" => Ok(ext),
        other => Err(format!(
            "지원하지 않는 이미지 형식: {other:?} (png/jpg/jpeg/bmp/gif)"
        )),
    }
}

/// 표시 크기(HWPUNIT)를 계산한다. 자연 크기는 본문 폭(max_w) 초과 시 비례 축소.
pub(crate) fn display_size(data: &[u8], size: &ImageSize, max_w: i32) -> (i32, i32) {
    match size {
        ImageSize::Mm(w, h) => (
            (*w * MM_TO_HWPUNIT).round() as i32,
            (*h * MM_TO_HWPUNIT).round() as i32,
        ),
        ImageSize::Natural => {
            let (pw, ph) = image_pixel_size(data).unwrap_or((300, 200));
            let w = pw as i64 * 7200 / 96;
            let h = ph as i64 * 7200 / 96;
            if w > i64::from(max_w) && w > 0 {
                let scale = f64::from(max_w) / w as f64;
                (max_w, (h as f64 * scale).round() as i32)
            } else {
                (w as i32, h as i32)
            }
        }
    }
}

/// Why an image reference could not be loaded (#56). `Soft` keeps the ordinary
/// degrade-to-alt-text (markdown) / contract-error (HTML) behavior; `Hard` is a sandbox
/// violation and must abort the import — it is never degraded to a warning.
pub(crate) enum ImageOpenError {
    /// Ordinary failure (resolution, missing file, permissions, dangling symlink, ...) —
    /// the caller's usual soft warning, so missing-file semantics stay unchanged.
    Soft(String),
    /// Sandbox violation — a deliberately generic message (no resolved path, no reference
    /// string): a sandbox error must not leak the filesystem layout.
    Hard(String),
}

/// Generic sandbox-violation message for image references (#56).
fn sandbox_error() -> String {
    "이미지 경로가 샌드박스 루트(--root) 밖에 있어 거부합니다".to_string()
}

/// Opens an image file referenced from markdown/HTML, binding it to the sandbox roots (#56).
/// The caller must read from the returned handle: with roots non-empty, containment is judged
/// from the opened handle (never the request pathname), so a symlink swapped between check and
/// read cannot smuggle outside bytes in — a symlink to an outside target opens the target, and
/// the handle-derived path is the target's path, which fails closed. Roots are expected
/// canonical already (the MCP server canonicalizes them at startup). Empty roots skip the
/// check entirely (CLI behavior — zero change). Ordinary open failures are mapped through
/// `soft` so each caller keeps its existing warning message.
pub(crate) fn open_image_under_roots(
    resolved: &Path,
    roots: &[PathBuf],
    soft: impl FnOnce(std::io::Error) -> String,
) -> Result<std::fs::File, ImageOpenError> {
    let file = std::fs::File::open(resolved).map_err(|e| ImageOpenError::Soft(soft(e)))?;
    if roots.is_empty() {
        return Ok(file);
    }
    // Judge the roots against the opened handle, not the request pathname (same policy as
    // hwp-cli's asset_snapshot): resolving the path from the descriptor keeps the check bound
    // to the file that was actually opened even when a path component was renamed or a symlink
    // was swapped between the open and this point.
    #[cfg(any(unix, windows))]
    check_opened_under_roots(&file, roots)?;
    // Pathname-based fallback for targets without a handle-to-path mechanism — the open there
    // is pathname-based already, so the roots check is too.
    #[cfg(not(any(unix, windows)))]
    check_resolved_under_roots(resolved, roots)?;
    Ok(file)
}

/// Containment from the opened handle: the handle-derived path (canonicalized to wash out
/// residual symlink components) must sit under at least one root. With roots set there is no
/// fail-open path — any resolution failure is a hard error (#56).
#[cfg(any(unix, windows))]
fn check_opened_under_roots(file: &std::fs::File, roots: &[PathBuf]) -> Result<(), ImageOpenError> {
    let canonical = std::fs::canonicalize(
        opened_handle_path(file).map_err(|_| ImageOpenError::Hard(sandbox_error()))?,
    )
    .map_err(|_| ImageOpenError::Hard(sandbox_error()))?;
    #[cfg(unix)]
    let contained = roots.iter().any(|root| canonical.starts_with(root));
    #[cfg(windows)]
    let contained = roots
        .iter()
        .any(|root| windows_path_is_within(&canonical, root));
    if contained {
        Ok(())
    } else {
        Err(ImageOpenError::Hard(sandbox_error()))
    }
}

#[cfg(target_os = "linux")]
fn opened_handle_path(file: &std::fs::File) -> Result<PathBuf, ImageOpenError> {
    use std::os::fd::AsRawFd as _;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|_| ImageOpenError::Hard(sandbox_error()))
}

#[cfg(target_os = "macos")]
fn opened_handle_path(file: &std::fs::File) -> Result<PathBuf, ImageOpenError> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let mut buffer = vec![0 as libc::c_char; libc::MAXPATHLEN as usize];
    // Safety: the caller holds `file` open, so the descriptor is valid, and the
    // buffer is MAXPATHLEN writable bytes as F_GETPATH requires; on success the
    // kernel writes a NUL-terminated path into it.
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) };
    if result == -1 {
        return Err(ImageOpenError::Hard(sandbox_error()));
    }
    let path = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn opened_handle_path(_file: &std::fs::File) -> Result<PathBuf, ImageOpenError> {
    // No portable handle-to-path mechanism on this target: fail closed (this
    // is only reached with a non-empty roots list).
    Err(ImageOpenError::Hard(sandbox_error()))
}

#[cfg(windows)]
fn opened_handle_path(file: &std::fs::File) -> Result<PathBuf, ImageOpenError> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_DOS,
        )
    };
    if length == 0 || length as usize >= buffer.len() {
        return Err(ImageOpenError::Hard(sandbox_error()));
    }
    Ok(PathBuf::from(OsString::from_wide(
        &buffer[..length as usize],
    )))
}

/// Case-insensitive, prefix-normalized containment for Windows handle paths (`\\?\`-style).
#[cfg(windows)]
fn windows_path_is_within(candidate: &Path, base: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        let value = path.as_os_str().to_string_lossy();
        let value = value.strip_prefix(r"\\?\UNC\").map_or_else(
            || value.strip_prefix(r"\\?\").unwrap_or(&value).to_string(),
            |suffix| format!(r"\\{suffix}"),
        );
        value.trim_end_matches(['\\', '/']).to_lowercase()
    }

    let candidate = normalized(candidate);
    let base = normalized(base);
    candidate == base
        || candidate
            .strip_prefix(&base)
            .is_some_and(|suffix| suffix.starts_with(['\\', '/']))
}

/// Pathname-based fallback for targets without a handle-to-path mechanism. The file is
/// already open at this point, so a canonicalize failure fails closed (#56).
#[cfg(not(any(unix, windows)))]
fn check_resolved_under_roots(resolved: &Path, roots: &[PathBuf]) -> Result<(), ImageOpenError> {
    let canonical =
        std::fs::canonicalize(resolved).map_err(|_| ImageOpenError::Hard(sandbox_error()))?;
    if roots.iter().any(|root| canonical.starts_with(root)) {
        Ok(())
    } else {
        Err(ImageOpenError::Hard(sandbox_error()))
    }
}

/// 한 문단에서 앵커 텍스트 뒤에 그림 앵커를 삽입한다. 반환=삽입 여부.
fn insert_image_in_para(para: &mut Paragraph, anchor: &str, pic: &Picture) -> bool {
    let Some((cidx, wpos)) = find_match(&para.chars, anchor, 0) else {
        return false;
    };
    let ins = (cidx + anchor.chars().count()).min(para.chars.len());
    let iw = wpos + utf16_len(anchor);
    // control 삽입 위치 = ins 이전 ExtCtrl 개수(등장순서가 chars와 정합해야 함).
    let ci = para.chars[..ins]
        .iter()
        .filter(|c| matches!(c, HwpChar::ExtCtrl { .. }))
        .count()
        .min(para.controls.len());
    para.controls.insert(ci, Control::Picture(pic.clone()));
    para.chars.insert(
        ins,
        HwpChar::ExtCtrl {
            code: GSO_CODE,
            ctrl_id: *b"gso ",
            payload: rev_payload(b"gso "),
            ctrl_index: None,
        },
    );
    adjust_runs(&mut para.char_shape_runs, iw, 0, 8); // ExtCtrl wchar_width=8
    relink_ctrl_index(para);
    para.header.ctrl_mask = 0; // writer가 chars에서 재계산(gso bit11 포함)
    para.line_segs.clear();
    true
}

/// 본문/표 셀/글상자 문단을 재귀로 훑어 첫 매칭에 그림을 삽입한다.
fn insert_image_rec(para: &mut Paragraph, anchor: &str, pic: &Picture) -> bool {
    if insert_image_in_para(para, anchor, pic) {
        return true;
    }
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        if insert_image_rec(p, anchor, pic) {
                            return true;
                        }
                    }
                }
            }
            Control::Generic(g) => {
                for l in &mut g.paragraph_lists {
                    for p in &mut l.paragraphs {
                        if insert_image_rec(p, anchor, pic) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// `anchor` 텍스트를 가진 첫 문단의 그 뒤에 `path` 이미지를 인라인(글자처럼)으로 삽입한다.
/// writer(hwp5)가 빈-extras Picture에 도형 레코드를 합성한다.
pub fn insert_image(
    doc: &mut Document,
    anchor: &str,
    path: &Path,
    size: ImageSize,
) -> Result<(), String> {
    let ext = ext_of(path)?;
    let data =
        std::fs::read(path).map_err(|e| format!("이미지 읽기 실패 {}: {e}", path.display()))?;
    if data.is_empty() {
        return Err(format!("빈 이미지 파일: {}", path.display()));
    }
    let (w, h) = display_size(&data, &size, content_width(doc));
    let name = format!("inserted{}.{ext}", doc.bin_streams.len() + 1);
    let pic = Picture {
        common_data: Vec::new(),
        width: HwpUnit(w.max(1)),
        height: HwpUnit(h.max(1)),
        treat_as_char: true,
        z_order: 0,
        vert_offset: 0,
        horz_offset: 0,
        description: None,
        bin_ref: BinRef::ItemRef(name.clone()),
        extras: Vec::new(),
    };
    let inserted = doc
        .sections
        .iter_mut()
        .flat_map(|s| &mut s.paragraphs)
        .any(|p| insert_image_rec(p, anchor, &pic));
    if !inserted {
        return Err(format!("앵커 {anchor:?}를 찾을 수 없습니다"));
    }
    doc.bin_streams.push(BinStream { name, data });
    Ok(())
}

/// 도장(직인) 기본 크기 20mm — 공공 실무 관례.
const DEFAULT_SEAL_MM: f32 = 20.0;
/// 앵커 오프셋 근사용 평균 글자 advance(HWPUNIT). 10pt 전각 ≈ 1000.
const AVG_CHAR_ADVANCE: i32 = 1000;
/// 도장을 얹을 줄 높이 근사(HWPUNIT).
const SEAL_LINE_HEIGHT: i32 = 1000;
/// 도장 z-순서 — 본문·일반 개체 위(앞)에 겹치도록 크게 잡는다.
const SEAL_Z_ORDER: u32 = 1000;

/// 한 문단에서 앵커 문구 위에 도장을 **부유 배치**한다. 앵커 텍스트는 유지하고,
/// gso 앵커 문자만 앵커 뒤에 삽입한다. 반환=삽입 여부.
///
/// 위치는 문단 기준(vertRelTo/horzRelTo=PARA)이며, 앵커 문구의 대략적 오프셋으로
/// 도장 중심을 맞춘다 — 정밀 배치는 실기 레이아웃에 의존하므로 근사값이다.
fn insert_seal_in_para(
    para: &mut Paragraph,
    anchor: &str,
    seal_w: i32,
    seal_h: i32,
    name: &str,
) -> bool {
    let Some((cidx, wpos)) = find_match(&para.chars, anchor, 0) else {
        return false;
    };
    // 가로 오프셋은 앵커 앞 **보이는 글자** 수로만 추정한다. wpos(wchar 위치)는 문단
    // 선두의 구역/단 정의 컨트롤 문자(각 wchar_width=8, 화면 폭 0)를 포함해 값이 크게
    // 부풀려지므로(D1 실측: 앵커가 좌측인데 horzOffset≈17666로 우측 이탈) 쓰지 않는다.
    // Text 글자만 세어 대략적 advance로 환산한다(전각 10pt ≈ AVG_CHAR_ADVANCE).
    let visible_before = para.chars[..cidx]
        .iter()
        .filter(|c| matches!(c, HwpChar::Text(_)))
        .count() as i32;
    let anchor_glyphs = anchor.chars().count() as i32;
    // 앵커 문구 중앙에 도장 중심을 맞춘 문단 기준 오프셋(대략적).
    let horz =
        visible_before * AVG_CHAR_ADVANCE + anchor_glyphs * AVG_CHAR_ADVANCE / 2 - seal_w / 2;
    // 줄 높이보다 큰 도장은 위로 밀어 줄 중앙에 오게 한다(음수=위로).
    let vert = (SEAL_LINE_HEIGHT - seal_h) / 2;
    let pic = Picture {
        common_data: Vec::new(),
        width: HwpUnit(seal_w.max(1)),
        height: HwpUnit(seal_h.max(1)),
        treat_as_char: false, // 부유(글 앞) 배치 — writer가 floating 공통속성 합성
        z_order: SEAL_Z_ORDER,
        vert_offset: vert,
        horz_offset: horz.max(0),
        description: None,
        bin_ref: BinRef::ItemRef(name.to_string()),
        extras: Vec::new(),
    };
    // 앵커 뒤에 gso 앵커 문자 삽입 — 앵커 텍스트 자체는 유지된다.
    let ins = (cidx + anchor.chars().count()).min(para.chars.len());
    let iw = wpos + utf16_len(anchor);
    let ci = para.chars[..ins]
        .iter()
        .filter(|c| matches!(c, HwpChar::ExtCtrl { .. }))
        .count()
        .min(para.controls.len());
    para.controls.insert(ci, Control::Picture(pic));
    para.chars.insert(
        ins,
        HwpChar::ExtCtrl {
            code: GSO_CODE,
            ctrl_id: *b"gso ",
            payload: rev_payload(b"gso "),
            ctrl_index: None,
        },
    );
    adjust_runs(&mut para.char_shape_runs, iw, 0, 8); // ExtCtrl wchar_width=8
    relink_ctrl_index(para);
    para.header.ctrl_mask = 0; // writer가 chars에서 재계산(gso bit11 포함)
    para.line_segs.clear();
    true
}

/// 본문/표 셀/글상자 문단을 재귀로 훑어 첫 매칭에 도장을 부유 배치한다.
fn insert_seal_rec(
    para: &mut Paragraph,
    anchor: &str,
    seal_w: i32,
    seal_h: i32,
    name: &str,
) -> bool {
    if insert_seal_in_para(para, anchor, seal_w, seal_h, name) {
        return true;
    }
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        if insert_seal_rec(p, anchor, seal_w, seal_h, name) {
                            return true;
                        }
                    }
                }
            }
            Control::Generic(g) => {
                for l in &mut g.paragraph_lists {
                    for p in &mut l.paragraphs {
                        if insert_seal_rec(p, anchor, seal_w, seal_h, name) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// 앵커 문구가 있는 첫 문단의 그 위치에 도장 이미지를 **부유(글 앞) 배치**로 겹친다.
/// `insert_image`(앵커 뒤 인라인 삽입)와 달리 앵커 텍스트를 유지하고 그림을 떠 있는
/// 개체로 만들어 앵커 문구 위에 겹치게 한다("(인)" 위 직인 날인 관례).
///
/// `size_mm`이 없으면 기본 20mm(도장 관례). 이미지 원본 비율을 유지한다(정사각 폴백).
/// hwp5 writer가 빈-extras Picture에 floating 공통속성을 합성한다(검증된 경로 재사용).
pub fn insert_seal(
    doc: &mut Document,
    anchor: &str,
    path: &Path,
    size_mm: Option<f32>,
) -> Result<(), String> {
    let ext = ext_of(path)?;
    let data =
        std::fs::read(path).map_err(|e| format!("이미지 읽기 실패 {}: {e}", path.display()))?;
    if data.is_empty() {
        return Err(format!("빈 이미지 파일: {}", path.display()));
    }
    let mm = size_mm.unwrap_or(DEFAULT_SEAL_MM);
    if !(mm.is_finite() && mm > 0.0) {
        return Err(format!("도장 크기(mm)는 양수여야 합니다: {mm}"));
    }
    let seal_w = ((mm * MM_TO_HWPUNIT).round() as i32).max(1);
    // 원본 비율 유지(치수를 못 읽으면 정사각).
    let seal_h = match image_pixel_size(&data) {
        Some((pw, ph)) if pw > 0 => ((i64::from(seal_w) * i64::from(ph)) / i64::from(pw)) as i32,
        _ => seal_w,
    }
    .max(1);
    let name = format!("seal{}.{ext}", doc.bin_streams.len() + 1);
    let inserted = doc
        .sections
        .iter_mut()
        .flat_map(|s| &mut s.paragraphs)
        .any(|p| insert_seal_rec(p, anchor, seal_w, seal_h, &name));
    if !inserted {
        return Err(format!("앵커 {anchor:?}를 찾을 수 없습니다"));
    }
    doc.bin_streams.push(BinStream { name, data });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// image_pixel_size가 최소 PNG/BMP 헤더에서 치수를 읽는다.
    #[test]
    fn 픽셀_치수_헤더_파싱() {
        // PNG: 시그니처(8) + IHDR len(4) + "IHDR"(4) + w(4 BE) + h(4 BE)
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(200u32.to_be_bytes());
        png.extend(100u32.to_be_bytes());
        assert_eq!(image_pixel_size(&png), Some((200, 100)));

        // BMP: "BM" + 16바이트 채운 뒤 폭/높이 at 18/22 (LE)
        let mut bmp = b"BM".to_vec();
        bmp.extend([0u8; 16]);
        bmp.extend(50i32.to_le_bytes());
        bmp.extend(40i32.to_le_bytes());
        assert_eq!(image_pixel_size(&bmp), Some((50, 40)));
    }

    /// insert_image가 Picture+BinStream을 만들고 앵커 링크가 맞는다.
    #[test]
    fn 이미지_삽입_구조() {
        let mut doc = crate::from_markdown::from_markdown("사진: 여기");
        let dir = std::env::temp_dir().join("hwp-img-test");
        std::fs::create_dir_all(&dir).unwrap();
        let png_path = dir.join("t.png");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]); // 나머지(파싱 안 함)
        std::fs::write(&png_path, &png).unwrap();

        insert_image(&mut doc, "사진:", &png_path, ImageSize::Natural).unwrap();

        // BinStream 1개 + Picture 1개 + resolve_bin 성공.
        assert_eq!(doc.bin_streams.len(), 1);
        let para = &doc.sections[0].paragraphs[0];
        let pic = para.controls.iter().find_map(|c| match c {
            Control::Picture(p) => Some(p),
            _ => None,
        });
        let pic = pic.expect("Picture 존재");
        assert!(pic.extras.is_empty(), "writer가 합성하도록 빈 extras");
        assert!(doc.resolve_bin(&pic.bin_ref).is_some(), "bin_ref 해석");
        // 앵커 ExtCtrl가 Picture를 가리킨다.
        let ext = para.chars.iter().find_map(|c| match c {
            HwpChar::ExtCtrl {
                code, ctrl_index, ..
            } if *code == GSO_CODE => *ctrl_index,
            _ => None,
        });
        assert!(
            matches!(para.controls[ext.unwrap() as usize], Control::Picture(_)),
            "앵커 ExtCtrl가 Picture 컨트롤을 가리켜야 한다"
        );
        // 96px → 96*7200/96 = 7200 HWPUNIT.
        assert_eq!(pic.width.0, 7200);

        // 없는 앵커는 오류.
        assert!(insert_image(&mut doc, "없는앵커", &png_path, ImageSize::Natural).is_err());
    }

    /// insert_seal이 앵커 문단에 **부유** Picture를 만들고 앵커 텍스트를 유지한다.
    #[test]
    fn 도장_부유_삽입_구조() {
        let mut doc = crate::from_markdown::from_markdown("결재란 (인) 끝");
        let dir = std::env::temp_dir().join("hwp-seal-test");
        std::fs::create_dir_all(&dir).unwrap();
        let png_path = dir.join("s.png");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]);
        std::fs::write(&png_path, &png).unwrap();

        insert_seal(&mut doc, "(인)", &png_path, None).unwrap();

        assert_eq!(doc.bin_streams.len(), 1);
        let para = &doc.sections[0].paragraphs[0];
        let pic = para
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("Picture 존재");
        // 부유 배치·writer 합성용 빈 extras·앞 z-order.
        // hwpx writer(write_picture)는 treat_as_char=false일 때
        // textWrap=IN_FRONT_OF_TEXT + flowWithText=0 + allowOverlap=1 + PARA 기준으로
        // 이 오프셋/z-order를 방출하고, hwp5 writer는 attr=0x04aa4310(PARA·글앞·제한해제)로 합성한다.
        assert!(!pic.treat_as_char, "도장은 부유(글 앞) 배치여야 한다");
        assert!(
            pic.extras.is_empty(),
            "writer가 floating 속성 합성하도록 빈 extras"
        );
        assert_eq!(pic.z_order, SEAL_Z_ORDER, "앞(위) 배치용 z-order 상수");
        // 도장이 줄 높이보다 크면 세로 오프셋이 음수(위로 올려 줄 중앙 정렬).
        assert!(
            pic.vert_offset < 0,
            "큰 도장은 줄 위로 올려 겹친다(음수 세로 오프셋)"
        );
        assert!(doc.resolve_bin(&pic.bin_ref).is_some(), "bin_ref 해석");
        // 기본 20mm·정사각(96x96)이므로 너비=높이.
        assert_eq!(pic.width.0, (20.0 * MM_TO_HWPUNIT).round() as i32);
        assert_eq!(pic.height.0, pic.width.0, "원본 비율(정사각) 유지");
        // 앵커 문구를 지나며 오프셋이 양수(문단 안쪽으로 이동).
        assert!(pic.horz_offset > 0, "앵커 뒤쪽 가로 오프셋");
        // 앵커 텍스트는 유지되어야 한다.
        assert!(doc.plain_text().contains("(인)"), "앵커 텍스트 유지");
        // 앵커 ExtCtrl가 Picture를 가리킨다.
        let ext = para.chars.iter().find_map(|c| match c {
            HwpChar::ExtCtrl {
                code, ctrl_index, ..
            } if *code == GSO_CODE => *ctrl_index,
            _ => None,
        });
        assert!(
            matches!(para.controls[ext.unwrap() as usize], Control::Picture(_)),
            "앵커 ExtCtrl가 Picture를 가리켜야 한다"
        );
        // 없는 앵커·잘못된 크기는 오류.
        assert!(insert_seal(&mut doc, "없음", &png_path, Some(15.0)).is_err());
        assert!(insert_seal(&mut doc, "(인)", &png_path, Some(0.0)).is_err());
    }
}
