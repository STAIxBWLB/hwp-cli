//! WMF(Windows Metafile) 벡터 이미지의 bounded 해석기.
//!
//! HWP/HWPX 그림 바이너리로 들어온 WMF를 레이아웃 시점에 해석해 기존 디스플레이
//! 리스트 항목(`Item::Path`/`Item::Image`/`Item::Glyphs`)으로 내린다. 백엔드별
//! 디코드 경로는 건드리지 않는다. 부분집합 밖 레코드는 계열당 한 번 typed
//! omission(`wmf_unsupported_record_omitted`)을 남기고 계속 진행하며, 손상된
//! 스트림은 자홍색 placeholder + `wmf_parse_invalid_placeholder`로 끝낸다
//! (shape_draw.rs의 소비단 전용 패턴과 동일한 규율).
//!
//! 실측 부분집합(사내 코퍼스 7개 파일): placeable 헤더 없음, 18B 표준 헤더
//! (type=1, headerSize=9 words, version=0x0300), MM_ANISOTROPIC + SetWindowOrg/Ext.
//! 레코드: `size:u32`(16비트 워드 수, 6B 헤더 포함) + `func:u16` + params.
//! WMF는 API 인자 순서를 뒤집어 기록하므로 점/크기 인자는 (Y, X) 순서다
//! (SetWindowOrg/Ext, MoveTo — MS-WMF 2.3.5 및 dstX 실측으로 확정).

use std::sync::Arc;

use crate::display::{Fill, FillRule, Item, PageList, PathCmd, Stroke};
use crate::fonts::FontStore;
use crate::issues::{RenderIssueAccumulator, RenderIssueCode};

// ── 레코드 함수 코드 (실측 부분집합) ──
const EOF: u16 = 0x0000;
const SAVE_DC: u16 = 0x001E;
const RESTORE_DC: u16 = 0x0127;
const SET_BK_MODE: u16 = 0x0102;
const SET_MAP_MODE: u16 = 0x0103;
const SET_ROP2: u16 = 0x0104;
const SET_POLY_FILL_MODE: u16 = 0x0106;
const SET_STRETCH_BLT_MODE: u16 = 0x0107;
const SET_BK_COLOR: u16 = 0x0201;
const SET_TEXT_COLOR: u16 = 0x0209;
const SET_WINDOW_ORG: u16 = 0x020B;
const SET_WINDOW_EXT: u16 = 0x020C;
const MOVE_TO: u16 = 0x0214;
const SELECT_CLIP_REGION: u16 = 0x012C;
const SELECT_OBJECT: u16 = 0x012D;
const SET_TEXT_ALIGN: u16 = 0x012E;
const DIB_CREATE_PATTERN_BRUSH: u16 = 0x0142;
const DELETE_OBJECT: u16 = 0x01F0;
const CREATE_FONT_INDIRECT: u16 = 0x02FB;
const CREATE_BRUSH_INDIRECT: u16 = 0x02FC;
const CREATE_PEN_INDIRECT: u16 = 0x02FA;
const POLYGON: u16 = 0x0324;
const POLYLINE: u16 = 0x0325;
const EXCLUDE_CLIP_RECT: u16 = 0x0415;
const INTERSECT_CLIP_RECT: u16 = 0x0416;
const POLY_POLYGON: u16 = 0x0538;
const ESCAPE: u16 = 0x0626;
const DIB_BIT_BLT: u16 = 0x0940;
const EXT_TEXT_OUT: u16 = 0x0A32;
const DIB_STRETCH_BLT: u16 = 0x0B41;

// raster op (관측값만 지원).
const ROP_SRCAND: u32 = 0x0088_00C6;
const ROP_SRCPAINT: u32 = 0x00EE_0086;
const ROP_SRCCOPY: u32 = 0x00CC_0020;

// ── 해석 상한 (초과 시 wmf_budget_exceeded + placeholder) ──
const MAX_RECORDS: u32 = 200_000;
const MAX_OBJECTS: usize = 4_096;
const MAX_DC_STACK: usize = 64;
const MAX_DIB_PIXELS: u64 = 16_777_216;

/// placeholder 자홍 (COLORREF 0x00BBGGRR → R=255,B=255).
const PLACEHOLDER_MAGENTA: u32 = 0x00FF_00FF;

/// draw_wmf의 결과. Placeholder면 자홍색 사각형과 typed issue는 이미 방출됐으므로
/// 호출자는 `Item::Image` 폴 백을 추가하지 않아야 한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmfOutcome {
    Drawn,
    Placeholder,
    /// 아무것도 방출하지 않았으니 호출자가 기존 래스터 경로(`Item::Image`)로
    /// 폴 백해야 한다 (현재는 crop이 윈도우와 전혀 겹치지 않는 퇴화 경우뿐).
    Fallback,
}

/// WMF 바이트 판정. PNG/JPEG/BMP/GIF 매직과 충돌하지 않도록 표준 헤더의
/// type/headerSize/version/선언 크기를 모두 검증한다 (보수적).
pub fn is_wmf(data: &[u8]) -> bool {
    const PLACEABLE_MAGIC: [u8; 4] = 0x9AC6_CDD7u32.to_le_bytes();
    let hdr = if data.starts_with(&PLACEABLE_MAGIC) {
        // placeable 헤더 22B 뒤에 표준 헤더가 온다.
        if data.len() < 22 + 18 {
            return false;
        }
        &data[22..]
    } else {
        data
    };
    if hdr.len() < 18 {
        return false;
    }
    let ty = u16::from_le_bytes([hdr[0], hdr[1]]);
    let header_words = u16::from_le_bytes([hdr[2], hdr[3]]);
    let version = u16::from_le_bytes([hdr[4], hdr[5]]);
    let size_words = u32::from_le_bytes([hdr[6], hdr[7], hdr[8], hdr[9]]) as usize;
    matches!(ty, 1 | 2)
        && header_words == 9
        && version == 0x0300
        && size_words >= 9
        && size_words * 2 <= hdr.len()
}

/// WMF 스트림을 해석해 목표 상자 (x, y, w, h) pt 안에 display item을 방출한다.
/// `crop`은 소스 이미지 HWPUNIT(96dpi: px×75)의 (left, top, right, bottom) —
/// 윈도우 논리 좌표로 환산(px=HWPUNIT/75)해 윈도우와 교차한 가시 영역이 목표
/// 상자를 채우도록 매핑한다 (래스터 crop 경로와 같은 의미). `None`이면 윈도우
/// 전체를 매핑한다. crop이 윈도우와 겹치지 않으면 [`WmfOutcome::Fallback`].
#[allow(clippy::too_many_arguments)]
pub fn draw_wmf(
    data: &[u8],
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    crop: Option<[f32; 4]>,
    page: &mut PageList,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) -> WmfOutcome {
    let mut interp = Interpreter {
        x: f64::from(x),
        y: f64::from(y),
        w: f64::from(w),
        h: f64::from(h),
        crop,
        page,
        store,
        warnings,
        dc: Dc::default(),
        stack: Vec::new(),
        table: Vec::new(),
    };
    match interp.run(data) {
        Ok(()) => WmfOutcome::Drawn,
        Err(Fail::NoView) => WmfOutcome::Fallback,
        Err(Fail::Invalid) => {
            interp.placeholder(RenderIssueCode::WmfParseInvalidPlaceholder, b"stream");
            WmfOutcome::Placeholder
        }
        Err(Fail::Budget) => {
            interp.placeholder(RenderIssueCode::WmfBudgetExceeded, b"budget");
            WmfOutcome::Placeholder
        }
    }
}

/// 해석 실패 분류. Invalid=손상/절단 스트림, Budget=상한 초과,
/// NoView=crop이 윈도우와 불겹침(아무것도 그릴 수 없음 → 호출자 폴 백).
enum Fail {
    Invalid,
    Budget,
    NoView,
}

type PResult<T> = Result<T, Fail>;

/// GDI 펜. style 5(PS_NULL)은 선택 시점에 None으로 접는다.
#[derive(Clone, Copy)]
struct Pen {
    color: u32,
    /// 논리 단위 폭 (0=1px 가닥 — 방출 시 1논리단위로 승격).
    width: i32,
}

/// GDI 브러시 (LOGBRUSH16 style 0/1/2 + 패턴 브러시).
#[derive(Clone, Copy)]
enum Brush {
    Solid(u32),
    Null,
    /// fg=브러시 색, WMF hatch 0..5 → 표29 style 1..6은 방출 시 +1.
    Hatch {
        color: u32,
        hatch: u16,
    },
    /// DIBCreatePatternBrush (8x8 1bpp 디더 실측) — 생성 시점에 디더 밀도를
    /// 단색으로 접은 근사 채움색을 보관한다 (density-blend; 상세는
    /// `pattern_density_blend` 주석). 실제 타일 패턴 프리미티브가 디스플레이
    /// 리스트에 생기면 이 variant의 payload만 교체하면 호출 측 변경 없이
    /// 되돌릴 수 있다.
    Pattern(u32),
}

/// LOGFONT16의 렌더 관련 필드만.
#[derive(Clone)]
struct WmfFont {
    /// lfHeight (음수=em 높이). 논리 단위.
    height: i32,
    bold: bool,
    italic: bool,
    /// CP949 → UTF-8 패밀리 이름.
    face: String,
}

#[derive(Clone)]
enum GdiObject {
    Pen(Option<Pen>),
    Brush(Brush),
    Font(WmfFont),
}

/// DC 상태 (객체 테이블은 메타파일 공유라 Save/Restore 대상이 아니다).
#[derive(Clone)]
struct Dc {
    win_org: (i32, i32),
    win_ext: (i32, i32),
    bk_mode: u16,
    bk_color: u32,
    text_color: u32,
    text_align: u16,
    fill_rule: FillRule,
    /// MoveTo / TA_UPDATECP가 갱신하는 현재 위치 (논리 단위).
    cur_pos: (i32, i32),
    pen: Option<Pen>,
    brush: Brush,
    font: Option<WmfFont>,
}

impl Default for Dc {
    fn default() -> Self {
        Self {
            win_org: (0, 0),
            win_ext: (0, 0),
            bk_mode: 1, // OPAQUE
            bk_color: 0x00FF_FFFF,
            text_color: 0,
            text_align: 0,                // TA_LEFT|TA_TOP
            fill_rule: FillRule::EvenOdd, // GDI 기본 ALTERNATE
            cur_pos: (0, 0),
            pen: Some(Pen { color: 0, width: 0 }),
            brush: Brush::Solid(0x00FF_FFFF),
            font: None,
        }
    }
}

struct Interpreter<'a> {
    /// 목표 상자 (pt).
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    /// 소스 이미지 HWPUNIT crop (left, top, right, bottom). None=윈도우 전체.
    crop: Option<[f32; 4]>,
    page: &'a mut PageList,
    store: &'a mut FontStore,
    warnings: &'a mut RenderIssueAccumulator,
    dc: Dc,
    stack: Vec<Dc>,
    table: Vec<Option<GdiObject>>,
}

// ── 바이트 리더 (전부 경계 검사) ──
fn rd_u16(d: &[u8], o: usize) -> PResult<u16> {
    d.get(o..o + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(Fail::Invalid)
}
fn rd_i16(d: &[u8], o: usize) -> PResult<i16> {
    Ok(rd_u16(d, o)? as i16)
}
fn rd_i32(d: &[u8], o: usize) -> PResult<i32> {
    d.get(o..o + 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Fail::Invalid)
}
fn rd_u32(d: &[u8], o: usize) -> PResult<u32> {
    Ok(rd_i32(d, o)? as u32)
}

impl Interpreter<'_> {
    fn run(&mut self, data: &[u8]) -> PResult<()> {
        if ![self.x, self.y, self.w, self.h]
            .iter()
            .all(|v| v.is_finite())
            || self.w <= 0.0
            || self.h <= 0.0
        {
            return Err(Fail::Invalid);
        }
        const PLACEABLE_MAGIC: [u8; 4] = 0x9AC6_CDD7u32.to_le_bytes();
        let hdr = if data.starts_with(&PLACEABLE_MAGIC) {
            data.get(22..).ok_or(Fail::Invalid)?
        } else {
            data
        };
        if hdr.len() < 18 {
            return Err(Fail::Invalid);
        }
        // is_wmf가 게이트지만 직접 호출 경로를 위해 한 번 더 검증한다.
        if !is_wmf(data) {
            return Err(Fail::Invalid);
        }
        let mut off = 9 * 2; // headerSize(9 words) — 실측은 항상 표준 18B
        for _ in 0..MAX_RECORDS {
            if off + 6 > hdr.len() {
                // EOF 없이 끝나면 절단으로 본다.
                return Err(Fail::Invalid);
            }
            let size_words = rd_u32(hdr, off)? as usize;
            let func = rd_u16(hdr, off + 4)?;
            let Some(end) = off.checked_add(size_words.saturating_mul(2)) else {
                return Err(Fail::Invalid);
            };
            if size_words < 3 || end > hdr.len() {
                return Err(Fail::Invalid);
            }
            if func == EOF {
                return Ok(());
            }
            let params = &hdr[off + 6..end];
            self.record(func, params)?;
            off = end;
        }
        Err(Fail::Budget) // 레코드 상한
    }

    fn record(&mut self, func: u16, p: &[u8]) -> PResult<()> {
        match func {
            // ── 상태/윈도잉 ──
            SET_MAP_MODE | SET_STRETCH_BLT_MODE => {} // 해석상 무의미 (MM_ANISOTROPIC 실측)
            SET_ROP2 => {
                // R2_COPYPEN(13)·R2_MASKPEN(9) 실측. v1은 항상 COPYPEN처럼 그린다
                // (MASKPEN의 dst&pen 합성은 미지원 — 오차는 색 합성에 국한).
            }
            SET_BK_MODE => self.dc.bk_mode = rd_u16(p, 0)?,
            SET_BK_COLOR => self.dc.bk_color = rd_u32(p, 0)? & 0x00FF_FFFF,
            SET_TEXT_COLOR => self.dc.text_color = rd_u32(p, 0)? & 0x00FF_FFFF,
            SET_TEXT_ALIGN => self.dc.text_align = rd_u16(p, 0)?,
            SET_POLY_FILL_MODE => {
                self.dc.fill_rule = match rd_u16(p, 0)? {
                    2 => FillRule::NonZero, // WINDING
                    _ => FillRule::EvenOdd, // ALTERNATE(1) 외 값도 방어적으로 even-odd
                };
            }
            SET_WINDOW_ORG => {
                // WMF는 (Y, X) 순서로 기록한다.
                self.dc.win_org = (i32::from(rd_i16(p, 2)?), i32::from(rd_i16(p, 0)?));
            }
            SET_WINDOW_EXT => {
                self.dc.win_ext = (i32::from(rd_i16(p, 2)?), i32::from(rd_i16(p, 0)?));
            }
            SAVE_DC => {
                if self.stack.len() >= MAX_DC_STACK {
                    return Err(Fail::Budget);
                }
                self.stack.push(self.dc.clone());
            }
            RESTORE_DC => {
                let n = rd_i16(p, 0).unwrap_or(-1);
                if n < 0 {
                    // 음수=상대 (실측 -1, 드물게 -2). 스택이 비면 GDI 오류 → 무시.
                    let pops = (-i32::from(n)).min(self.stack.len() as i32);
                    for _ in 0..pops {
                        if let Some(dc) = self.stack.pop() {
                            self.dc = dc;
                        }
                    }
                } else if n > 0 && self.stack.len() >= n as usize {
                    // 양수=절대(1-기반 저장 번호) — 그 이후 저장분을 모두 버린다.
                    while self.stack.len() >= n as usize {
                        if let Some(dc) = self.stack.pop() {
                            self.dc = dc;
                        }
                    }
                }
            }
            // ── 객체 테이블 (GDI: 첫 빈 슬롯에 생성) ──
            CREATE_PEN_INDIRECT => {
                // LOGPEN16: style u16, width POINTL(x i16만 유효 — y는 무시,
                // GDI는 x 컴포넌트만 펜 폭으로 쓴다), COLORREF u32.
                let style = rd_u16(p, 0)?;
                let width = i32::from(rd_i16(p, 2)?);
                let color = rd_u32(p, 6)? & 0x00FF_FFFF;
                // PS_NULL(5)만 접는다. 파선 계열(1..4)은 미관측 — 실선으로 그린다.
                let pen = (style != 5).then_some(Pen { color, width });
                self.create_object(GdiObject::Pen(pen))?;
            }
            CREATE_BRUSH_INDIRECT => {
                // LOGBRUSH16: style u16, COLORREF u32, hatch u16.
                let style = rd_u16(p, 0)?;
                let color = rd_u32(p, 2)? & 0x00FF_FFFF;
                let hatch = rd_u16(p, 6)?;
                let brush = match style {
                    0 => Brush::Solid(color),
                    1 => Brush::Null,
                    2 => Brush::Hatch { color, hatch },
                    _ => return Err(Fail::Invalid),
                };
                self.create_object(GdiObject::Brush(brush))?;
            }
            CREATE_FONT_INDIRECT => {
                // LOGFONT16(50B): lfHeight i16 … lfFaceName은 offset 18, 32B CP949.
                if p.len() < 50 {
                    return Err(Fail::Invalid);
                }
                let height = i32::from(rd_i16(p, 0)?);
                let weight = rd_i16(p, 8)?;
                let italic = p[10] != 0;
                let face_raw = &p[18..50];
                let end = face_raw
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(face_raw.len());
                let (face, _, _) = encoding_rs::EUC_KR.decode(&face_raw[..end]);
                self.create_object(GdiObject::Font(WmfFont {
                    height,
                    bold: weight >= 700,
                    italic,
                    face: face.into_owned(),
                }))?;
            }
            DIB_CREATE_PATTERN_BRUSH => {
                // style u16 + colorUsage u16 + packed DIB (실측 8x8 1bpp 디더).
                if p.len() < 4 {
                    return Err(Fail::Invalid);
                }
                let brush = match pattern_density_blend(&p[4..], &self.dc) {
                    Some(color) => Brush::Pattern(color),
                    None => {
                        // 8x8 mono가 아닌 형태는 미관측 — bounded skip (생성 시점에
                        // 한 번 남긴다). 슬롯 인덱스 유지를 위해 null 브러시로 등록.
                        self.unsupported(DIB_CREATE_PATTERN_BRUSH);
                        Brush::Null
                    }
                };
                self.create_object(GdiObject::Brush(brush))?;
            }
            SELECT_OBJECT => {
                let idx = rd_u16(p, 0)? as usize;
                match self.table.get(idx).and_then(|o| o.as_ref()) {
                    Some(GdiObject::Pen(pen)) => self.dc.pen = *pen,
                    Some(GdiObject::Brush(brush)) => self.dc.brush = *brush,
                    Some(GdiObject::Font(font)) => self.dc.font = Some(font.clone()),
                    None => {} // 잘못된 인덱스는 GDI도 조용히 실패
                }
            }
            DELETE_OBJECT => {
                let idx = rd_u16(p, 0)? as usize;
                if let Some(slot) = self.table.get_mut(idx) {
                    *slot = None;
                }
            }
            // ── 클리핑: 디스플레이 리스트에 클립 프리미티브가 없어 v1은 무시한다
            // (issue 없음 — 경계 밖 잉크가 살짝 넘칠 수 있음. 측정 후 재검토). ──
            EXCLUDE_CLIP_RECT | INTERSECT_CLIP_RECT | SELECT_CLIP_REGION => {}
            // ── Escape: 실측은 전부 MFCOMMENT(0x000F) — 조용히 무시. ──
            // ── Escape: MFCOMMENT(0x000F)만 조용히 무시 (실측 전부 이것).
            // 그 외 escape 함수는 detail=함수 id만 담아 bounded-skip. ──
            ESCAPE => {
                let esc = rd_u16(p, 0)?;
                if esc != 0x000F {
                    self.unsupported(esc);
                }
            }
            // ── 그리기 ──
            MOVE_TO => {
                // (Y, X) 순서. TA_UPDATECP 텍스트의 기준점.
                self.dc.cur_pos = (i32::from(rd_i16(p, 2)?), i32::from(rd_i16(p, 0)?));
            }
            POLYGON | POLYLINE => {
                let count = rd_i16(p, 0)?;
                if count < 0 {
                    return Err(Fail::Invalid);
                }
                let pts = self.read_points(p, 2, count as usize)?;
                if func == POLYGON {
                    self.emit_polygon(&[pts.as_slice()], true)?;
                } else {
                    self.emit_polyline(&pts)?;
                }
            }
            POLY_POLYGON => {
                let count = rd_i16(p, 0)?;
                if count < 0 {
                    return Err(Fail::Invalid);
                }
                let count = count as usize;
                let mut off = 2;
                let mut polys = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    let n = rd_i16(p, off)?;
                    if n < 0 {
                        return Err(Fail::Invalid);
                    }
                    off += 2;
                    polys.push(n as usize);
                }
                let mut groups = Vec::with_capacity(polys.len());
                for n in polys {
                    let pts = self.read_points(p, off, n)?;
                    off += n * 4;
                    groups.push(pts);
                }
                let refs: Vec<&[(i32, i32)]> = groups.iter().map(Vec::as_slice).collect();
                self.emit_polygon(&refs, true)?;
            }
            DIB_STRETCH_BLT => self.dib_stretch_blit(p)?,
            DIB_BIT_BLT => {
                // rop u32 + (srcY, srcX, H, W, dstY, dstX) i16×6 — 실측 3996건 전부
                // 폭 또는 높이가 0이라 GDI 의미상 아무것도 칠하지 않는다. 정확히 skip
                // (typed issue도 불필요 — 0-면적 blit 생략은 의미 동치).
                let height = rd_i16(p, 8)?;
                let width = rd_i16(p, 10)?;
                if height != 0 && width != 0 {
                    // 비0-면적 DIBBitBlt는 미관측 — 부분집합 밖.
                    self.unsupported(func);
                }
            }
            EXT_TEXT_OUT => self.ext_text_out(p)?,
            _ => self.unsupported(func),
        }
        Ok(())
    }

    /// 부분집합 밖 레코드 계열 — detail은 함수 코드만 (내용 무첨가).
    fn unsupported(&mut self, func: u16) {
        self.warnings.push(
            RenderIssueCode::WmfUnsupportedRecordOmitted,
            format!("0x{func:04X}"),
        );
    }

    fn create_object(&mut self, obj: GdiObject) -> PResult<()> {
        if let Some(slot) = self
            .table
            .iter_mut()
            .take(MAX_OBJECTS)
            .find(|s| s.is_none())
        {
            *slot = Some(obj);
            return Ok(());
        }
        if self.table.len() < MAX_OBJECTS {
            self.table.push(Some(obj));
            return Ok(());
        }
        Err(Fail::Budget)
    }

    /// 현재 DC의 가시 논리 영역 (ox, oy, ex, ey): 윈도우(org/ext — 파일 기록은
    /// (Y, X) 순서라 X-ext가 두 번째)와 crop의 교차. ext가 음수면 절댓값을 쓴다
    /// (뒤집힌 윈도우는 미관측 — 방어적 처리). crop은 HWPUNIT → px(=/75) 환산 후
    /// 교차하며, 교차가 비면 Fail::NoView다. 부분 crop일 때 영역 밖 잉크는 클립
    /// 프리미티브 부재로 상자 밖까지 그려질 수 있다 (클립 레코드 무시와 같은
    /// 클래스의 v1 한계 — 코퍼스 crop은 전부 full-extent라 실영향 없음).
    fn view(&self) -> PResult<(f64, f64, f64, f64)> {
        let (ex, ey) = self.dc.win_ext;
        if ex == 0 || ey == 0 {
            return Err(Fail::Invalid);
        }
        let (wx, wy) = (f64::from(self.dc.win_org.0), f64::from(self.dc.win_org.1));
        let (we, whe) = (f64::from(ex.abs()), f64::from(ey.abs()));
        let Some([l, t, r, b]) = self.crop else {
            return Ok((wx, wy, we, whe));
        };
        if ![l, t, r, b].iter().all(|v| v.is_finite()) {
            return Err(Fail::NoView);
        }
        let (cl, ct) = (f64::from(l) / 75.0, f64::from(t) / 75.0);
        let (cr, cb) = (f64::from(r) / 75.0, f64::from(b) / 75.0);
        let (x0, y0) = (cl.max(wx), ct.max(wy));
        let (x1, y1) = (cr.min(wx + we), cb.min(wy + whe));
        if x1 - x0 <= 0.0 || y1 - y0 <= 0.0 {
            return Err(Fail::NoView);
        }
        Ok((x0, y0, x1 - x0, y1 - y0))
    }

    /// (lx, ly) 논리 좌표 → 페이지 pt (가시 영역이 목표 상자를 채우도록).
    fn map(&self, lx: i32, ly: i32) -> PResult<(f32, f32)> {
        let (ox, oy, ex, ey) = self.view()?;
        let px = self.x + (f64::from(lx) - ox) * self.w / ex;
        let py = self.y + (f64::from(ly) - oy) * self.h / ey;
        let (px, py) = (px as f32, py as f32);
        if !px.is_finite() || !py.is_finite() {
            return Err(Fail::Invalid);
        }
        Ok((px, py))
    }

    /// x축 논리 단위 → pt 스케일 (펜 폭·텍스트 진행량에 사용).
    fn sx(&self) -> PResult<f32> {
        let (_, _, ex, _) = self.view()?;
        let s = (self.w / ex) as f32;
        if !s.is_finite() {
            return Err(Fail::Invalid);
        }
        Ok(s)
    }

    fn sy(&self) -> PResult<f32> {
        let (_, _, _, ey) = self.view()?;
        let s = (self.h / ey) as f32;
        if !s.is_finite() {
            return Err(Fail::Invalid);
        }
        Ok(s)
    }

    fn read_points(&self, p: &[u8], off: usize, count: usize) -> PResult<Vec<(i32, i32)>> {
        let mut pts = Vec::with_capacity(count);
        for i in 0..count {
            // POINTS는 (X, Y) 정상 순서.
            let px = i32::from(rd_i16(p, off + i * 4)?);
            let py = i32::from(rd_i16(p, off + i * 4 + 2)?);
            pts.push((px, py));
        }
        Ok(pts)
    }

    fn current_fill(&self) -> Option<Fill> {
        match self.dc.brush {
            Brush::Solid(c) => Some(Fill::Solid(c)),
            Brush::Null => None,
            Brush::Hatch { color, hatch } => Some(Fill::Hatch {
                fg: color,
                // OPAQUE면 배경색으로 먼저 칠하고, TRANSPARENT이면 투명.
                bg: if self.dc.bk_mode == 1 {
                    self.dc.bk_color
                } else {
                    0xFFFF_FFFF
                },
                style: u32::from(hatch).min(5) + 1,
            }),
            Brush::Pattern(color) => Some(Fill::Solid(color)), // density-blend 근사
        }
    }

    fn current_stroke(&self) -> PResult<Option<Stroke>> {
        let Some(pen) = self.dc.pen else {
            return Ok(None);
        };
        let width = (pen.width.max(1) as f32 * self.sx()?).max(0.1);
        Ok(Some(Stroke::solid(pen.color, width)))
    }

    /// 다각형(닫힘) — 현재 브러시로 채우고 펜으로 스트로크.
    fn emit_polygon(&mut self, polys: &[&[(i32, i32)]], close: bool) -> PResult<()> {
        let mut commands = Vec::new();
        for poly in polys {
            for (i, &(lx, ly)) in poly.iter().enumerate() {
                let (px, py) = self.map(lx, ly)?;
                commands.push(if i == 0 {
                    PathCmd::MoveTo(px, py)
                } else {
                    PathCmd::LineTo(px, py)
                });
            }
            if close && !poly.is_empty() {
                commands.push(PathCmd::Close);
            }
        }
        if commands.len() < 2 {
            return Ok(());
        }
        let (fill, stroke) = (self.current_fill(), self.current_stroke()?);
        if fill.is_none() && stroke.is_none() {
            return Ok(()); // 보이지 않는 프레임
        }
        if !self.warnings.charge_display_items(1) {
            return Err(Fail::Budget);
        }
        self.page.items.push(Item::Path {
            commands,
            fill,
            stroke,
            rule: self.dc.fill_rule,
        });
        Ok(())
    }

    /// 열린折선 — 펜으로만.
    fn emit_polyline(&mut self, pts: &[(i32, i32)]) -> PResult<()> {
        let Some(stroke) = self.current_stroke()? else {
            return Ok(());
        };
        if pts.len() < 2 {
            return Ok(());
        }
        let mut commands = Vec::with_capacity(pts.len());
        for (i, &(lx, ly)) in pts.iter().enumerate() {
            let (px, py) = self.map(lx, ly)?;
            commands.push(if i == 0 {
                PathCmd::MoveTo(px, py)
            } else {
                PathCmd::LineTo(px, py)
            });
        }
        if !self.warnings.charge_display_items(1) {
            return Err(Fail::Budget);
        }
        self.page.items.push(Item::Path {
            commands,
            fill: None,
            stroke: Some(stroke),
            rule: self.dc.fill_rule,
        });
        Ok(())
    }

    /// ExtTextOut — CP949 문자열을 현재 폰트로 셰이핑해 Glyphs로 방출한다.
    fn ext_text_out(&mut self, p: &[u8]) -> PResult<()> {
        let ty = i32::from(rd_i16(p, 0)?);
        let tx = i32::from(rd_i16(p, 2)?);
        let count = rd_i16(p, 4)?;
        let options = rd_u16(p, 6)?;
        if count < 0 {
            return Err(Fail::Invalid);
        }
        let count = count as usize;
        let has_rect = options & 0x0006 != 0; // ETO_OPAQUE(2)|ETO_CLIPPED(4)
        let str_off = if has_rect { 16 } else { 8 };
        let bytes = p.get(str_off..str_off + count).ok_or(Fail::Invalid)?;
        // 문자열은 짝수 길이로 패딩, 뒤에 dx i16 배열이 올 수 있다. dx(글자별
        // 위치)는 v1에서 무시 — 셰이핑 어드밴스로 대체한다(미세 자간 오차 가능).
        let (text, _, _) = encoding_rs::EUC_KR.decode(bytes);
        if text.is_empty() {
            return Ok(());
        }
        // ETO_OPAQUE: 명시 사각형을 배경색으로 먼저 칠한다.
        if options & 0x0002 != 0 {
            let (l, t, r, b) = (
                i32::from(rd_i16(p, 8)?),
                i32::from(rd_i16(p, 10)?),
                i32::from(rd_i16(p, 12)?),
                i32::from(rd_i16(p, 14)?),
            );
            let (x0, y0) = self.map(l, t)?;
            let (x1, y1) = self.map(r, b)?;
            let (rx, rw) = (x0.min(x1), (x1 - x0).abs());
            let (ry, rh) = (y0.min(y1), (y1 - y0).abs());
            if rw > 0.0 && rh > 0.0 {
                if !self.warnings.charge_display_items(1) {
                    return Err(Fail::Budget);
                }
                self.page.items.push(Item::Rect {
                    x: rx,
                    y: ry,
                    w: rw,
                    h: rh,
                    fill: self.dc.bk_color,
                });
            }
        }
        let font = self.dc.font.clone().unwrap_or(WmfFont {
            // 폰트 미선택 — GDI 스톡 폰트 대신 ~10pt 상당 폴 백 이름 없음 경로.
            height: 0,
            bold: false,
            italic: false,
            face: String::new(),
        });
        let size_pt = if font.height != 0 {
            font.height.unsigned_abs() as f32 * self.sy()?
        } else {
            10.0
        };
        let Some(selection) = self.store.resolve_family_selection(&font.face, font.bold) else {
            return Ok(()); // FontMissing은 resolver가 이미 기록했다
        };
        let Some(run) = crate::shape::shape_text_with_font(
            &selection.font,
            size_pt,
            self.dc.text_color,
            font.italic,
            &text,
            selection.faux_bold,
        ) else {
            return Ok(());
        };
        // TA_UPDATECP(1): x,y 인자 대신 현재 위치를 쓰고 진행량만큼 갱신한다.
        let (tx, ty) = if self.dc.text_align & 1 != 0 {
            self.dc.cur_pos
        } else {
            (tx, ty)
        };
        let (mut gx, mut gy) = self.map(tx, ty)?;
        match self.dc.text_align & 6 {
            6 => gx -= run.width_pt / 2.0, // TA_CENTER
            2 => gx -= run.width_pt,       // TA_RIGHT
            _ => {}                        // TA_LEFT
        }
        match self.dc.text_align & 24 {
            24 => {}                  // TA_BASELINE — y가 곧 베이스라인
            8 => gy -= size_pt * 0.2, // TA_BOTTOM
            _ => gy += size_pt * 0.8, // TA_TOP — 위쪽 기준을 베이스라인으로
        }
        if self.dc.text_align & 1 != 0 {
            let advance_lu = (run.width_pt / self.sx()?) as i32;
            self.dc.cur_pos.0 += advance_lu;
        }
        if !gx.is_finite() || !gy.is_finite() {
            return Err(Fail::Invalid);
        }
        if !self.warnings.charge_display_items(1) {
            return Err(Fail::Budget);
        }
        self.page.items.push(Item::Glyphs { x: gx, y: gy, run });
        Ok(())
    }

    /// DIBStretchBlt — packed DIB을 RGBA로 디코드해 PNG 바이트의 Item::Image로.
    ///
    /// 실측 ROP 3종의 의미를 blit 단독으로 근사한다 (코퍼스에서 마스크/컬러 쌍은
    /// 인접하지 않고 사이에 텍스트·다각형이 끼어 있어 쌍 결합은 z-order상 부정확):
    /// - SRCCOPY: 불투명 blit.
    /// - SRCPAINT(1bpp): dst|src — src가 흰색인 곳만 해당 색으로 덮는다
    ///   → 밝은 픽셀=불투명, 어두운 픽셀=투명 알파로 정확히 동치.
    /// - SRCAND: dst&src — 흰 픽셀은 dst 유지(투명), 나머지는 src 색으로 덮는
    ///   근사(dst가 흰색이면 정확; 아니면 채널 AND와 다를 수 있음).
    fn dib_stretch_blit(&mut self, p: &[u8]) -> PResult<()> {
        let rop = rd_u32(p, 0)?;
        // (srcY, srcX, srcH, srcW, dstH, dstW, dstY, dstX) — 실측 확정 순서.
        let src_h = rd_i16(p, 8)?;
        let src_w = rd_i16(p, 10)?;
        let dst_h = i32::from(rd_i16(p, 12)?);
        let dst_w = i32::from(rd_i16(p, 14)?);
        let dst_y = i32::from(rd_i16(p, 16)?);
        let dst_x = i32::from(rd_i16(p, 18)?);
        if src_h != 0 || src_w != 0 {
            // 소스 영역 지정 blit는 미관측 (실측은 항상 0=전체 소스).
            self.unsupported(DIB_STRETCH_BLT);
            return Ok(());
        }
        if dst_w == 0 || dst_h == 0 {
            return Ok(()); // 0-면적은 아무것도 칠하지 않는다 (정확한 skip)
        }
        if !matches!(rop, ROP_SRCCOPY | ROP_SRCPAINT | ROP_SRCAND) {
            self.unsupported(DIB_STRETCH_BLT);
            return Ok(());
        }
        let dib = p.get(20..).ok_or(Fail::Invalid)?;
        let mut img = match decode_dib(dib) {
            Ok(img) => img,
            Err(DibFail::Unsupported) => {
                // 압축(RLE)·미지원 bpp 등 미관측 조합 — 이 blit만 생략.
                self.unsupported(DIB_STRETCH_BLT);
                return Ok(());
            }
            Err(DibFail::Budget) => return Err(Fail::Budget),
            Err(DibFail::Invalid) => return Err(Fail::Invalid),
        };
        // SRCPAINT는 1bpp 마스크에서만 알파 합성 의미가 성립한다 (실측 전부 1bpp).
        if rop == ROP_SRCPAINT && img.bpp != 1 {
            self.unsupported(DIB_STRETCH_BLT);
            return Ok(());
        }
        match rop {
            ROP_SRCPAINT => {
                // 밝은(흰) 픽셀만 불투명 — dst|mask의 동치 변환.
                for px in img.rgba.chunks_exact_mut(4) {
                    let lum = u16::from(px[0]) + u16::from(px[1]) + u16::from(px[2]);
                    px[3] = if lum > 3 * 128 { 255 } else { 0 };
                }
            }
            ROP_SRCAND => {
                // 흰 픽셀은 dst 유지(투명), 나머지는 src 색.
                for px in img.rgba.chunks_exact_mut(4) {
                    let white = px[0] >= 250 && px[1] >= 250 && px[2] >= 250;
                    px[3] = if white { 0 } else { 255 };
                }
            }
            _ => {} // SRCCOPY — 불투명 그대로
        }
        // 음수 폭/높이는 GDI에서 미러링 (실측 dstW<0 존재).
        let (dst_x, dst_w) = if dst_w < 0 {
            img.flip_horizontal();
            (dst_x + dst_w, -dst_w)
        } else {
            (dst_x, dst_w)
        };
        let (dst_y, dst_h) = if dst_h < 0 {
            img.flip_vertical();
            (dst_y + dst_h, -dst_h)
        } else {
            (dst_y, dst_h)
        };
        let (ix, iy) = self.map(dst_x, dst_y)?;
        let iw = dst_w as f32 * self.sx()?;
        let ih = dst_h as f32 * self.sy()?;
        if ![iw, ih].iter().all(|v| v.is_finite()) || iw <= 0.0 || ih <= 0.0 {
            return Err(Fail::Invalid);
        }
        let Some(png) = encode_png(&img) else {
            return Err(Fail::Invalid);
        };
        if !self.warnings.charge_display_items(1) {
            return Err(Fail::Budget);
        }
        self.page.items.push(Item::Image {
            x: ix,
            y: iy,
            w: iw,
            h: ih,
            data: Arc::new(png),
            crop: None,
            flip: 0,
            rotation_deg: 0.0,
            brightness: 0,
            contrast: 0,
        });
        Ok(())
    }

    /// 손상/상한 초과 시 전체 이미지 상자를 자홍색으로 — 백엔드 placeholder와 동일 규칙.
    fn placeholder(&mut self, code: RenderIssueCode, detail: &[u8]) {
        self.warnings.push(code, detail);
        if !self.warnings.charge_display_items(1) {
            return;
        }
        self.page.items.push(Item::Rect {
            x: self.x as f32,
            y: self.y as f32,
            w: self.w as f32,
            h: self.h as f32,
            fill: PLACEHOLDER_MAGENTA,
        });
    }
}

/// 디코드된 DIB (top-down RGBA 행렬).
struct Dib {
    w: u32,
    h: u32,
    bpp: u16,
    rgba: Vec<u8>,
}

impl Dib {
    fn flip_horizontal(&mut self) {
        let w = self.w as usize;
        for row in self.rgba.chunks_exact_mut(w * 4) {
            for c in 0..w / 2 {
                let (a, b) = (c * 4, (w - 1 - c) * 4);
                for k in 0..4 {
                    row.swap(a + k, b + k);
                }
            }
        }
    }

    fn flip_vertical(&mut self) {
        let w = self.w as usize;
        let h = self.h as usize;
        for r in 0..h / 2 {
            for c in 0..w * 4 {
                self.rgba.swap(r * w * 4 + c, (h - 1 - r) * w * 4 + c);
            }
        }
    }
}

/// packed DIB 디코드 실패 분류.
enum DibFail {
    /// 손상/절단 — 전체 스트림을 placeholder로.
    Invalid,
    /// 픽셀 상한 초과 — budget placeholder.
    Budget,
    /// 미관측 조합(압축·bpp·COREHEADER) — 이 blit만 typed omission.
    Unsupported,
}

/// packed DIB(BITMAPINFOHEADER 40B + 팔레트 + bottom-up 픽셀)을 top-down RGBA로.
/// bpp 1/4/8(RGBQUAD 팔레트)·24(BGR)만 지원. 팔레트가 없는 1bpp(실측 — 헤더 바로
/// 뒤에 픽셀이 오는 기록기가 있다)는 0=검정/1=흰색 기본 팔레트를 적용한다.
fn decode_dib(dib: &[u8]) -> Result<Dib, DibFail> {
    if dib.len() < 40 {
        return Err(DibFail::Invalid);
    }
    if rd_u32(dib, 0).map_err(|_| DibFail::Invalid)? != 40 {
        return Err(DibFail::Unsupported); // BITMAPCOREHEADER(12B) 등 미관측
    }
    let rd = |o: usize| rd_i32(dib, o).map_err(|_| DibFail::Invalid);
    let w = rd(4)?;
    let h_raw = rd(8)?;
    if w <= 0 || h_raw == 0 {
        return Err(DibFail::Invalid);
    }
    let planes = rd_u16(dib, 12).map_err(|_| DibFail::Invalid)?;
    let bpp = rd_u16(dib, 14).map_err(|_| DibFail::Invalid)?;
    let compression = rd_u32(dib, 16).map_err(|_| DibFail::Invalid)?;
    let clr_used = rd_u32(dib, 32).map_err(|_| DibFail::Invalid)? as usize;
    if planes != 1 || !matches!(bpp, 1 | 4 | 8 | 24) || compression != 0 {
        return Err(DibFail::Unsupported);
    }
    // biClrUsed는 팔레트 할당 크기를 결정하므로 검증 전에 신뢰할 수 없다 —
    // 1<<bpp(합법 상한, 실측 최대 164/8bpp)를 넘으면 손상으로 본다. 픽셀 버퍼는
    // 위의 MAX_DIB_PIXELS 검사 이후에만 할당된다.
    if bpp <= 8 && clr_used > (1usize << bpp) {
        return Err(DibFail::Invalid);
    }
    let (w, h) = (w as u32, h_raw.unsigned_abs());
    if u64::from(w) * u64::from(h) > MAX_DIB_PIXELS {
        return Err(DibFail::Budget);
    }
    let stride = (u64::from(w) * u64::from(bpp)).div_ceil(32) * 4;
    let img_bytes = (stride * u64::from(h)) as usize;
    // 픽셀은 항상 스트림 끝에서 img_bytes만큼 — 팔레트를 잘라 쓰는 기록기
    // (1bpp 무팔레트 실측)와 정상 기록기를 같은 식으로 커버한다.
    let Some(px_off) = dib.len().checked_sub(img_bytes) else {
        return Err(DibFail::Invalid);
    };
    if px_off < 40 {
        return Err(DibFail::Invalid);
    }
    let pal_avail = (px_off - 40) / 4;
    // 팔레트: clrUsed > 0이면 그 수, 아니면 1<<bpp. 부족분은 회색조로 보충.
    let pal_needed = if bpp <= 8 {
        if clr_used > 0 {
            clr_used
        } else {
            1usize << bpp
        }
    } else {
        0
    };
    let mut palette: Vec<(u8, u8, u8)> = Vec::with_capacity(pal_needed);
    for i in 0..pal_needed {
        if i < pal_avail {
            let o = 40 + i * 4;
            // RGBQUAD = (B, G, R, reserved).
            palette.push((dib[o + 2], dib[o + 1], dib[o]));
        } else {
            // 기본 회색조 램프 (1bpp: 0=검정, 1=흰색).
            let v = (i * 255 / (pal_needed - 1).max(1)) as u8;
            palette.push((v, v, v));
        }
    }
    let (w_us, h_us, stride_us) = (w as usize, h as usize, stride as usize);
    let mut rgba = vec![0u8; w_us * h_us * 4];
    let bottom_up = h_raw > 0;
    for y in 0..h_us {
        let src_y = if bottom_up { h_us - 1 - y } else { y };
        let row = &dib[px_off + src_y * stride_us..px_off + src_y * stride_us + stride_us];
        for x in 0..w_us {
            let (r, g, b) = match bpp {
                24 => {
                    let o = x * 3;
                    (row[o + 2], row[o + 1], row[o])
                }
                8 => palette.get(row[x] as usize).copied().unwrap_or((0, 0, 0)),
                4 => {
                    let idx = if x % 2 == 0 {
                        row[x / 2] >> 4
                    } else {
                        row[x / 2] & 0x0F
                    };
                    palette.get(idx as usize).copied().unwrap_or((0, 0, 0))
                }
                _ => {
                    let idx = (row[x / 8] >> (7 - (x % 8))) & 1;
                    palette.get(idx as usize).copied().unwrap_or((0, 0, 0))
                }
            };
            let o = (y * w_us + x) * 4;
            rgba[o..o + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    Ok(Dib { w, h, bpp, rgba })
}

/// 모노 패턴 브러시의 디더 밀도 → 단색 근사 (density-blend).
///
/// 8x8 1bpp 디더는 출력 해상도에서 평탄한 틴트로 읽히므로 1-bit 비율을 밀도로
/// fg/bg를 채널별로 섞어 `Fill::Solid`로 내린다. GDI의 모노 패턴 실현 색은 DIB
/// 팔레트가 아니라 생성 시점 DC의 text color(1-bit)와 bk color(0-bit)다.
/// (채움 시점 bk mode가 TRANSPARENT면 0-bit가 비쳐야 하지만 v1은 그 뉘앙스를
/// 무시하고 혼합 단색을 그대로 쓴다.) 기대 형태(8x8 1bpp BI_RGB)가 아니면
/// None — 호출자가 typed omission으로 bounded-skip한다.
fn pattern_density_blend(dib: &[u8], dc: &Dc) -> Option<u32> {
    if dib.len() < 72 {
        return None;
    }
    if rd_u32(dib, 0).ok()? != 40 {
        return None;
    }
    let w = rd_i32(dib, 4).ok()?;
    let h = rd_i32(dib, 8).ok()?;
    let planes = rd_u16(dib, 12).ok()?;
    let bpp = rd_u16(dib, 14).ok()?;
    let compression = rd_u32(dib, 16).ok()?;
    if w != 8 || h.unsigned_abs() != 8 || planes != 1 || bpp != 1 || compression != 0 {
        return None;
    }
    // 픽셀은 스트림 끝 32B (stride 4B × 8행) — decode_dib와 같은 끝 기준 역산.
    let px = &dib[dib.len() - 32..];
    let bits: u32 = px.iter().map(|b| b.count_ones()).sum();
    let (fg, bg) = (dc.text_color, dc.bk_color);
    let mix = |shift: u32| {
        let f = (fg >> shift) & 0xFF;
        let b = (bg >> shift) & 0xFF;
        (f * bits + b * (64 - bits) + 32) / 64
    };
    // COLORREF(0x00BBGGRR) 채널은 바이트 경계 — shift 0/8/16 (R/G/B).
    Some(mix(0) | (mix(8) << 8) | (mix(16) << 16))
}

/// RGBA → PNG 인코딩 (hwp-convert의 encode_png과 같은 품질 설정).
fn encode_png(img: &Dib) -> Option<Vec<u8>> {
    use image::ImageEncoder as _;
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(&img.rgba, img.w, img.h, image::ExtendedColorType::Rgba8)
    .ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::FillRule as DispFillRule;

    /// 인메모리 WMF 빌더 (png.rs의 synthetic-PNG 패턴과 동일한 접근).
    struct Builder {
        recs: Vec<u8>,
    }

    impl Builder {
        /// SetWindowOrg(0,0) + SetWindowExt(Y=100, X=200)를 심고 시작한다.
        /// (파일 기록 순서는 Y 먼저 — ext 좌표계 (200 x 100).)
        fn new() -> Self {
            let mut b = Builder { recs: Vec::new() };
            b.rec(SET_WINDOW_ORG, &u16_params(&[0, 0]));
            b.rec(SET_WINDOW_EXT, &u16_params(&[100, 200]));
            b
        }

        /// 레코드 하나: size(워드) + func + params(짝수 패딩).
        fn rec(&mut self, func: u16, params: &[u8]) -> &mut Self {
            let size_words = (6 + params.len()).div_ceil(2);
            self.recs
                .extend_from_slice(&(size_words as u32).to_le_bytes());
            self.recs.extend_from_slice(&func.to_le_bytes());
            self.recs.extend_from_slice(params);
            if !params.len().is_multiple_of(2) {
                self.recs.push(0);
            }
            self
        }

        fn select_object(&mut self, idx: u16) -> &mut Self {
            self.rec(SELECT_OBJECT, &idx.to_le_bytes())
        }

        fn create_solid_brush(&mut self, color: u32) -> &mut Self {
            let mut p = Vec::new();
            p.extend_from_slice(&0u16.to_le_bytes()); // BS_SOLID
            p.extend_from_slice(&color.to_le_bytes());
            p.extend_from_slice(&0u16.to_le_bytes()); // hatch
            self.rec(CREATE_BRUSH_INDIRECT, &p)
        }

        fn create_pen(&mut self, style: u16, width: i32, color: u32) -> &mut Self {
            self.create_pen_xy(style, width as i16, 0, color)
        }

        /// POINTL x/y를 따로 쓰는 변형 — y 무시 검증용.
        fn create_pen_xy(&mut self, style: u16, x: i16, y: i16, color: u32) -> &mut Self {
            let mut p = Vec::new();
            p.extend_from_slice(&style.to_le_bytes());
            p.extend_from_slice(&x.to_le_bytes());
            p.extend_from_slice(&y.to_le_bytes());
            p.extend_from_slice(&color.to_le_bytes());
            self.rec(CREATE_PEN_INDIRECT, &p)
        }

        fn create_pattern_brush(&mut self, dib: &[u8]) -> &mut Self {
            let mut p = Vec::new();
            p.extend_from_slice(&3u16.to_le_bytes()); // BS_PATTERN
            p.extend_from_slice(&0u16.to_le_bytes()); // colorUsage
            p.extend_from_slice(dib);
            self.rec(DIB_CREATE_PATTERN_BRUSH, &p)
        }

        fn points(&mut self, func: u16, pts: &[(i16, i16)]) -> &mut Self {
            let mut p = Vec::new();
            p.extend_from_slice(&(pts.len() as i16).to_le_bytes());
            for &(x, y) in pts {
                p.extend_from_slice(&x.to_le_bytes());
                p.extend_from_slice(&y.to_le_bytes());
            }
            self.rec(func, &p)
        }

        fn finish(&self) -> Vec<u8> {
            let mut out = Vec::new();
            let size_words = (18 + self.recs.len() + 6) / 2;
            out.extend_from_slice(&1u16.to_le_bytes()); // type = 메모리 메타파일
            out.extend_from_slice(&9u16.to_le_bytes()); // headerSize (words)
            out.extend_from_slice(&0x0300u16.to_le_bytes());
            out.extend_from_slice(&(size_words as u32).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // numObjects
            out.extend_from_slice(&0u32.to_le_bytes()); // maxRecord
            out.extend_from_slice(&0u16.to_le_bytes()); // numParams
            out.extend_from_slice(&self.recs);
            out.extend_from_slice(&3u32.to_le_bytes()); // EOF
            out.extend_from_slice(&0u16.to_le_bytes());
            out
        }
    }

    fn u16_params(vals: &[u16]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// 8x8 1bpp DIB. rows[0]이 맨 위 행. palette=None이면 무팔레트(기본 흑백).
    fn dib_1bpp(rows: [u8; 8], palette: Option<[(u8, u8, u8); 2]>) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&40u32.to_le_bytes()); // biSize
        d.extend_from_slice(&8i32.to_le_bytes()); // biWidth
        d.extend_from_slice(&8i32.to_le_bytes()); // biHeight (bottom-up)
        d.extend_from_slice(&1u16.to_le_bytes()); // planes
        d.extend_from_slice(&1u16.to_le_bytes()); // bpp
        d.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        d.extend_from_slice(&32u32.to_le_bytes()); // sizeImage
        d.extend_from_slice(&0i32.to_le_bytes()); // xppm
        d.extend_from_slice(&0i32.to_le_bytes()); // yppm
        d.extend_from_slice(&0u32.to_le_bytes()); // clrUsed
        d.extend_from_slice(&0u32.to_le_bytes()); // clrImportant
        if let Some(pal) = palette {
            for (r, g, b) in pal {
                d.extend_from_slice(&[b, g, r, 0]); // RGBQUAD
            }
        }
        // bottom-up: 마지막 행부터 기록. stride=4B(패딩).
        for row in rows.iter().rev() {
            d.extend_from_slice(&[*row, 0, 0, 0]);
        }
        d
    }

    /// 2x2 24bpp DIB (전부 같은 색).
    fn dib_24bpp(rgb: (u8, u8, u8)) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&24u16.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        let (r, g, b) = rgb;
        for _ in 0..2 {
            d.extend_from_slice(&[b, g, r, b, g, r, 0, 0]); // stride 8B
        }
        d
    }

    /// DIBStretchBlt params: rop + (srcY,srcX,srcH,srcW,dstH,dstW,dstY,dstX) + DIB.
    fn stretch_blit(rop: u32, dst: (i16, i16, i16, i16), dib: &[u8]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&rop.to_le_bytes());
        let (dst_y, dst_x, dst_h, dst_w) = dst;
        for v in [
            dib_i16(dib, 8), // srcY = biHeight (실측 기록기 관행)
            dib_i16(dib, 4), // srcX = biWidth
            0,
            0,
            dst_h,
            dst_w,
            dst_y,
            dst_x,
        ] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.extend_from_slice(dib);
        p
    }

    fn dib_i16(dib: &[u8], off: usize) -> i16 {
        i32::from_le_bytes(dib[off..off + 4].try_into().unwrap()) as i16
    }

    struct Ctx {
        page: PageList,
        store: FontStore,
        warnings: RenderIssueAccumulator,
    }

    fn draw(data: &[u8]) -> (Ctx, WmfOutcome) {
        draw_with_crop(data, None)
    }

    fn draw_with_crop(data: &[u8], crop: Option<[f32; 4]>) -> (Ctx, WmfOutcome) {
        let mut ctx = Ctx {
            page: PageList {
                width_pt: 600.0,
                height_pt: 800.0,
                items: Vec::new(),
            },
            store: FontStore::new(),
            warnings: RenderIssueAccumulator::new(),
        };
        // 목표 상자 (10, 20, 200, 100) pt — ext (200, 100)이므로 sx=1, sy=1.
        let outcome = draw_wmf(
            data,
            10.0,
            20.0,
            200.0,
            100.0,
            crop,
            &mut ctx.page,
            &mut ctx.store,
            &mut ctx.warnings,
        );
        (ctx, outcome)
    }

    #[test]
    fn solid_polygon_fill_and_fill_rule() {
        let mut b = Builder::new();
        b.create_solid_brush(0x0000_00FF) // COLORREF red
            .select_object(0)
            .rec(SET_POLY_FILL_MODE, &2u16.to_le_bytes()) // WINDING
            .points(POLYGON, &[(0, 0), (200, 0), (0, 100)]);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        let path = ctx
            .page
            .items
            .iter()
            .find_map(|it| match it {
                Item::Path {
                    commands,
                    fill,
                    rule,
                    ..
                } => Some((commands, fill, rule)),
                _ => None,
            })
            .expect("Path가 방출되어야 함");
        assert!(matches!(path.1, Some(Fill::Solid(0x0000_00FF))));
        assert_eq!(*path.2, DispFillRule::NonZero);
        // (0,0)→(10,20), (200,0)→(210,20): ext X=200, sx=1 (Y,X 기록 순서 고정 확인).
        assert!(
            matches!(path.0[0], PathCmd::MoveTo(x, y) if (x - 10.0).abs() < 1e-3 && (y - 20.0).abs() < 1e-3)
        );
        assert!(
            matches!(path.0[1], PathCmd::LineTo(x, y) if (x - 210.0).abs() < 1e-3 && (y - 20.0).abs() < 1e-3)
        );
        assert!(matches!(path.0.last(), Some(PathCmd::Close)));

        // ALTERNATE → EvenOdd.
        let mut b = Builder::new();
        b.create_solid_brush(0)
            .select_object(0)
            .rec(SET_POLY_FILL_MODE, &1u16.to_le_bytes())
            .points(POLYGON, &[(0, 0), (10, 0), (0, 10)]);
        let (ctx, _) = draw(&b.finish());
        let rule = ctx.page.items.iter().find_map(|it| match it {
            Item::Path { rule, .. } => Some(*rule),
            _ => None,
        });
        assert_eq!(rule, Some(DispFillRule::EvenOdd));
    }

    #[test]
    fn pen_polyline_stroke() {
        let mut b = Builder::new();
        b.create_pen(0, 4, 0x0000_FF00) // solid, 4논리단위, green
            .select_object(0)
            .points(POLYLINE, &[(0, 0), (50, 50)]);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        assert_eq!(ctx.page.items.len(), 1);
        let Item::Path {
            stroke,
            fill,
            commands,
            ..
        } = &ctx.page.items[0]
        else {
            panic!("Path여야 함");
        };
        assert!(fill.is_none());
        let st = stroke.as_ref().expect("펜 스트로크");
        assert_eq!(st.color, 0x0000_FF00);
        assert!(
            (st.width - 4.0).abs() < 1e-3,
            "4lu × sx(1) = 4pt: {}",
            st.width
        );
        assert!(!matches!(commands.last(), Some(PathCmd::Close)));
    }

    #[test]
    fn ext_text_out_glyphs_and_opaque_rect() {
        let mut b = Builder::new();
        // LOGFONT16: lfHeight=-20, weight=400, face="Arial"(CP949 ASCII).
        let mut lf = vec![0u8; 50];
        lf[0..2].copy_from_slice(&(-20i16).to_le_bytes());
        lf[8..10].copy_from_slice(&400i16.to_le_bytes());
        lf[13] = 0; // ANSI charset
        lf[18..18 + 5].copy_from_slice(b"Arial");
        b.rec(CREATE_FONT_INDIRECT, &lf)
            .select_object(0)
            .rec(SET_TEXT_COLOR, &0x0000_0000u32.to_le_bytes())
            .rec(SET_BK_COLOR, &0x00FF_FFFFu32.to_le_bytes());
        // ETO_OPAQUE rect (l=0,t=0,r=50,b=20) + "AB" @ (x=5, y=10).
        let mut p = Vec::new();
        p.extend_from_slice(&10i16.to_le_bytes()); // y
        p.extend_from_slice(&5i16.to_le_bytes()); // x
        p.extend_from_slice(&2i16.to_le_bytes()); // count
        p.extend_from_slice(&2u16.to_le_bytes()); // ETO_OPAQUE
        p.extend_from_slice(&0i16.to_le_bytes()); // rect left
        p.extend_from_slice(&0i16.to_le_bytes()); // rect top
        p.extend_from_slice(&50i16.to_le_bytes()); // rect right
        p.extend_from_slice(&20i16.to_le_bytes()); // rect bottom
        p.extend_from_slice(b"AB");
        b.rec(EXT_TEXT_OUT, &p);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        // 첫 항목은 배경 Rect (ETO_OPAQUE).
        let Item::Rect { x, y, w, h, fill } = ctx.page.items[0] else {
            panic!("배경 Rect가 먼저 와야 함: {:?}", ctx.page.items.len());
        };
        assert_eq!(fill, 0x00FF_FFFF);
        assert!((x - 10.0).abs() < 1e-3 && (y - 20.0).abs() < 1e-3);
        assert!((w - 50.0).abs() < 1e-3 && (h - 20.0).abs() < 1e-3);
        // 둘째는 Glyphs — TA_TOP이므로 baseline = y + 0.8·size (size=20pt).
        let Item::Glyphs { x, y, run } = &ctx.page.items[1] else {
            panic!("Glyphs여야 함");
        };
        assert_eq!(run.text, "AB");
        assert!((x - 15.0).abs() < 1e-3, "x=10+5·1: {x}");
        assert!((y - (30.0 + 16.0)).abs() < 1e-3, "y=20+10+0.8·20: {y}");
        assert!((run.size_pt - 20.0).abs() < 1e-3);
    }

    #[test]
    fn stretch_blit_copy_and_masked_alpha() {
        let mut b = Builder::new();
        // 1bpp SRCPAINT 마스크: 윗 4행만 1.
        let mask = dib_1bpp([0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00], None);
        let color = dib_1bpp(
            [0x00; 8],
            Some([(255, 0, 0), (255, 255, 255)]), // idx0=red, idx1=white
        );
        let copy = dib_24bpp((9, 8, 7));
        b.rec(
            DIB_STRETCH_BLT,
            &stretch_blit(ROP_SRCPAINT, (0, 0, 8, 8), &mask),
        );
        b.rec(
            DIB_STRETCH_BLT,
            &stretch_blit(ROP_SRCAND, (0, 0, 8, 8), &color),
        );
        b.rec(
            DIB_STRETCH_BLT,
            &stretch_blit(ROP_SRCCOPY, (20, 20, 2, 2), &copy),
        );
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        let images: Vec<&Vec<u8>> = ctx
            .page
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Image { data, .. } => Some(&**data),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 3);
        let decode = |png: &Vec<u8>| image::load_from_memory(png).unwrap().to_rgba8();
        // 마스크: 상단 행은 불투명 흰색, 하단 행은 투명.
        let m = decode(images[0]);
        assert_eq!(m.dimensions(), (8, 8));
        assert_eq!(m.get_pixel(0, 0).0[3], 255);
        assert_eq!(m.get_pixel(0, 7).0[3], 0);
        // SRCAND: 전부 idx0(red) → 불투명 빨강 (흰색이 아니므로).
        let c = decode(images[1]);
        assert_eq!(c.get_pixel(0, 0).0, [255, 0, 0, 255]);
        // SRCCOPY: 불투명 원색.
        let s = decode(images[2]);
        assert_eq!(s.dimensions(), (2, 2));
        assert_eq!(s.get_pixel(0, 0).0, [9, 8, 7, 255]);
    }

    #[test]
    fn zero_area_bitblt_skips_exactly() {
        let mut b = Builder::new();
        let mut p = Vec::new();
        p.extend_from_slice(&0x00AA_0029u32.to_le_bytes()); // DSTINVERT
        for v in [0i16, 0, 0, 100, 5, 5] {
            // srcY, srcX, H=0, W=100, dstY, dstX
            p.extend_from_slice(&v.to_le_bytes());
        }
        b.rec(DIB_BIT_BLT, &p);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        assert!(ctx.page.items.is_empty());
        assert_eq!(ctx.warnings.finish().issue_count, 0);
    }

    #[test]
    fn unknown_record_is_typed_omission_and_continues() {
        let mut b = Builder::new();
        b.rec(0x7777, &[1, 2, 3, 4]);
        b.rec(0x7777, &[5, 6, 7, 8]);
        b.create_solid_brush(0x0011_2233)
            .select_object(0)
            .points(POLYGON, &[(0, 0), (10, 0), (0, 10)]);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        assert_eq!(ctx.page.items.len(), 1, "뒤 레코드는 계속 해석");
        let report = ctx.warnings.finish();
        let s = report
            .issues
            .iter()
            .find(|s| s.code == RenderIssueCode::WmfUnsupportedRecordOmitted)
            .expect("omission issue");
        assert_eq!(s.count, 2);
        assert_eq!(s.sample_sha256.len(), 1, "같은 함수 코드는 sample dedup");
    }

    #[test]
    fn truncated_stream_is_placeholder() {
        let mut data = Builder::new().finish();
        data.truncate(data.len() - 10); // EOF와 레코드 일부 절단
        // 마지막 완전 레코드가 선언 크기를 넘지 않도록, 크기만 부풀린 변형도 검사.
        let (ctx, outcome) = draw(&data);
        assert_eq!(outcome, WmfOutcome::Placeholder);
        assert_eq!(ctx.page.items.len(), 1);
        let Item::Rect { fill, .. } = ctx.page.items[0] else {
            panic!("placeholder Rect여야 함");
        };
        assert_eq!(fill, 0x00FF_00FF);
        let report = ctx.warnings.finish();
        assert!(
            report
                .issues
                .iter()
                .any(|s| s.code == RenderIssueCode::WmfParseInvalidPlaceholder)
        );
    }

    #[test]
    fn is_wmf_sniffing_rejects_raster_magic() {
        assert!(is_wmf(&Builder::new().finish()));
        assert!(!is_wmf(b"\x89PNG\r\n\x1a\n...."));
        assert!(!is_wmf(b"\xFF\xD8\xFF\xE0...."));
        assert!(!is_wmf(b"GIF89a........"));
        assert!(!is_wmf(b"BM.........."));
        assert!(!is_wmf(b"short"));
        // 버전/헤더 크기 불일치는 거부.
        let mut bad = Builder::new().finish();
        bad[4] = 0x00;
        bad[5] = 0x01; // version 0x0100
        assert!(!is_wmf(&bad));
    }

    /// crop 테스트용: 단 하나의 삼각형을 그리는 스트림.
    fn one_triangle() -> Vec<u8> {
        let mut b = Builder::new();
        b.create_solid_brush(0x0000_00FF)
            .select_object(0)
            .points(POLYGON, &[(150, 50), (200, 100), (100, 100)]);
        b.finish()
    }

    fn first_path_start(ctx: &Ctx) -> (f32, f32) {
        ctx.page
            .items
            .iter()
            .find_map(|it| match it {
                Item::Path { commands, .. } => match commands[0] {
                    PathCmd::MoveTo(x, y) => Some((x, y)),
                    _ => None,
                },
                _ => None,
            })
            .expect("Path가 있어야 함")
    }

    #[test]
    fn full_extent_crop_matches_no_crop() {
        let data = one_triangle();
        let (plain, po) = draw(&data);
        // 윈도우는 X=200, Y=100 — full-extent crop은 HWPUNIT(=px×75)으로 동일 범위.
        let (cropped, co) = draw_with_crop(&data, Some([0.0, 0.0, 200.0 * 75.0, 100.0 * 75.0]));
        assert_eq!(po, WmfOutcome::Drawn);
        assert_eq!(co, WmfOutcome::Drawn);
        assert_eq!(first_path_start(&plain), first_path_start(&cropped));
    }

    #[test]
    fn partial_crop_remaps_subwindow_onto_box() {
        let data = one_triangle();
        // 오른쪽 절반만: 가시 영역 = x∈[100,200], y∈[0,100] → sx=200/100=2.
        let (ctx, outcome) =
            draw_with_crop(&data, Some([100.0 * 75.0, 0.0, 200.0 * 75.0, 100.0 * 75.0]));
        assert_eq!(outcome, WmfOutcome::Drawn);
        // (150, 50) → x = 10 + (150-100)·2 = 110, y = 20 + 50·1 = 70.
        let (px, py) = first_path_start(&ctx);
        assert!((px - 110.0).abs() < 1e-3, "x: {px}");
        assert!((py - 70.0).abs() < 1e-3, "y: {py}");
    }

    #[test]
    fn disjoint_crop_falls_back_without_items_or_issues() {
        let data = one_triangle();
        // 윈도우(X 0..200) 밖의 crop — 호출자가 래스터 경로로 폴 백해야 한다.
        let (ctx, outcome) =
            draw_with_crop(&data, Some([300.0 * 75.0, 0.0, 400.0 * 75.0, 100.0 * 75.0]));
        assert_eq!(outcome, WmfOutcome::Fallback);
        assert!(ctx.page.items.is_empty(), "아무것도 방출하지 않아야 함");
        assert_eq!(ctx.warnings.finish().issue_count, 0, "조용한 폴 백");
    }

    /// 패턴 브러시 삼각형의 채움색을 꺼낸다.
    fn pattern_fill(rows: [u8; 8]) -> (Ctx, Option<u32>) {
        let mut b = Builder::new();
        b.rec(SET_TEXT_COLOR, &0x0000_0000u32.to_le_bytes()) // 1-bit = 검정
            .rec(SET_BK_COLOR, &0x00FF_FFFFu32.to_le_bytes()) // 0-bit = 흰색
            .create_pattern_brush(&dib_1bpp(rows, Some([(0, 0, 0), (255, 255, 255)])))
            .select_object(0)
            .points(POLYGON, &[(0, 0), (10, 0), (0, 10)]);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        let fill = ctx.page.items.iter().find_map(|it| match it {
            Item::Path {
                fill: Some(Fill::Solid(c)),
                ..
            } => Some(*c),
            _ => None,
        });
        (ctx, fill)
    }

    #[test]
    fn pattern_brush_density_blend_solid() {
        // 50% 체커보드 → fg(검정)/bg(흰색)의 정확한 중간색.
        let (_, fill) = pattern_fill([0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55]);
        assert_eq!(fill, Some(0x0080_8080));
        // 100% → fg 그대로.
        let (_, fill) = pattern_fill([0xFF; 8]);
        assert_eq!(fill, Some(0x0000_0000));
        // 0% → bg 그대로.
        let (_, fill) = pattern_fill([0x00; 8]);
        assert_eq!(fill, Some(0x00FF_FFFF));
    }

    #[test]
    fn pattern_brush_unexpected_form_is_typed_omission() {
        // 8x8 1bpp가 아닌 형태 (bpp=4로 변조) — 생성 시점 omission + 채움 없음.
        let mut dib = dib_1bpp([0xFF; 8], Some([(0, 0, 0), (255, 255, 255)]));
        dib[14] = 4; // biBitCount = 4
        let mut b = Builder::new();
        b.create_pattern_brush(&dib)
            .select_object(0)
            .points(POLYGON, &[(0, 0), (10, 0), (0, 10)]);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        let Item::Path { fill, stroke, .. } = &ctx.page.items[0] else {
            panic!("Path여야 함");
        };
        assert!(fill.is_none(), "미지 형태는 채움 없음");
        assert!(stroke.is_some(), "기본 펜 스트로크는 남는다");
        let report = ctx.warnings.finish();
        let s = report
            .issues
            .iter()
            .find(|s| s.code == RenderIssueCode::WmfUnsupportedRecordOmitted)
            .expect("omission issue");
        assert_eq!(s.count, 1);
    }

    #[test]
    fn pattern_blend_non_grayscale_channels() {
        // fg=red(0x000000FF), bg=blue(0x00FF0000), 50% 체커보드.
        // 채널별 (f·32 + b·32 + 32)/64: R=(255·32+32)/64=128, G=(0+32)/64=0,
        // B=(255·32+32)/64=128 → 0x00800080.
        let mut b = Builder::new();
        b.rec(SET_TEXT_COLOR, &0x0000_00FFu32.to_le_bytes())
            .rec(SET_BK_COLOR, &0x00FF_0000u32.to_le_bytes())
            .create_pattern_brush(&dib_1bpp(
                [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55],
                Some([(0, 0, 0), (255, 255, 255)]),
            ))
            .select_object(0)
            .points(POLYGON, &[(0, 0), (10, 0), (0, 10)]);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        let fill = ctx.page.items.iter().find_map(|it| match it {
            Item::Path {
                fill: Some(Fill::Solid(c)),
                ..
            } => Some(*c),
            _ => None,
        });
        assert_eq!(fill, Some(0x0080_0080));
    }

    #[test]
    fn pen_width_uses_pointl_x_only() {
        // width POINTL = (x=3, y=0x7FFF) — y가 새면 폭이 깨진다.
        let mut b = Builder::new();
        b.create_pen_xy(0, 3, 0x7FFF, 0)
            .select_object(0)
            .points(POLYLINE, &[(0, 0), (50, 50)]);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        let Item::Path { stroke, .. } = &ctx.page.items[0] else {
            panic!("Path여야 함");
        };
        let w = stroke.as_ref().expect("펜 스트로크").width;
        assert!((w - 3.0).abs() < 1e-3, "x=3lu × sx(1) = 3pt: {w}");
    }

    #[test]
    fn escape_only_mfcomment_is_silent() {
        let mut b = Builder::new();
        // MFCOMMENT(0x000F) ×2 — 무시. SETABORTPROC(0x0200), 클립 보드 escape
        // (0x0201) — 각각 omission. 0x0200을 한 번 더 → 같은 id는 sample dedup.
        for esc in [0x000Fu16, 0x000F, 0x0200, 0x0201, 0x0200] {
            b.rec(ESCAPE, &esc.to_le_bytes());
        }
        b.create_solid_brush(0x0011_2233)
            .select_object(0)
            .points(POLYGON, &[(0, 0), (10, 0), (0, 10)]);
        let (ctx, outcome) = draw(&b.finish());
        assert_eq!(outcome, WmfOutcome::Drawn);
        assert_eq!(ctx.page.items.len(), 1, "escape 뒤 레코드는 계속 해석");
        let report = ctx.warnings.finish();
        let s = report
            .issues
            .iter()
            .find(|s| s.code == RenderIssueCode::WmfUnsupportedRecordOmitted)
            .expect("omission issue");
        assert_eq!(s.count, 3, "MFCOMMENT 2건은 무시, 나머지 3건");
        assert_eq!(s.sample_sha256.len(), 2, "distinct escape id당 sample 1개");
    }
}
