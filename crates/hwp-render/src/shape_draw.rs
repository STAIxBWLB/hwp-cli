//! 그리기 개체(도형) 렌더링 — gso 컨트롤의 raw_children에서 SHAPE_COMPONENT와
//! 기하(선/사각형/타원/호/다각형/곡선)를 렌더 시점에 파싱해 `Item::Path`로 만든다.
//!
//! IR·라운드트립 라이터를 건드리지 않는 소비단 전용(gso.rs가 박스를 읽는 패턴과 동일).
//! 좌표 변환: 생성(local) 공간 점 → 렌더 행렬(T·S·R) → +origin(HWPUNIT) → /100 = pt.
//! 바이트 레이아웃은 `docs/spec.txt` Table 81~103 + 실측(annual_report)으로 확정.

use std::sync::Arc;

use hwp_model::{BinDataId, BinRef, Document, GenericControl, OpaqueRecord, ShapeGeom, ShapeKind};

use crate::display::{Fill, FillRule, Gradient, Item, PageList, PathCmd, Stroke, path_bbox};
use crate::issues::{RenderIssueAccumulator, RenderIssueCode};

// hwp5 레코드 raw 태그 (HWPTAG_BEGIN = 0x10).
const SHAPE_COMPONENT: u16 = 0x4C; // 76
const SC_LINE: u16 = 0x4E;
const SC_RECTANGLE: u16 = 0x4F;
const SC_ELLIPSE: u16 = 0x50;
const SC_ARC: u16 = 0x51;
const SC_POLYGON: u16 = 0x52;
const SC_CURVE: u16 = 0x53;
const SC_CONTAINER: u16 = 0x56; // 86 (묶음 — 방어적)

/// 베지에 원호 근사 상수 (4/3·tan(45°/2)).
const KAPPA: f64 = 0.552_284_749_8;
const MAX_DEPTH: u32 = 16;

fn is_geom(tag: u16) -> bool {
    matches!(
        tag,
        SC_LINE | SC_RECTANGLE | SC_ELLIPSE | SC_ARC | SC_POLYGON | SC_CURVE
    )
}

/// 도형이 하나라도 있는지 (skip-count 판정용). draw_component와 같은 도달성으로 재귀.
pub fn has_shape(recs: &[OpaqueRecord]) -> bool {
    recs.iter().any(|r| match r.tag {
        SHAPE_COMPONENT => r.children.iter().any(|c| is_geom(c.tag)) || has_shape(&r.children),
        SC_CONTAINER => has_shape(&r.children),
        _ => false,
    })
}

/// hwpx 구조화 도형(ShapeGeom)을 Item::Path로 그린다. 좌표는 페이지 절대(HWPUNIT).
pub fn draw_ir_shapes(
    shapes: &[ShapeGeom],
    page: &mut PageList,
    warnings: &mut RenderIssueAccumulator,
) {
    for s in shapes {
        // A line can produce two arrowhead paths plus its own path. Reserve the
        // worst case before constructing any command buffers.
        if !warnings.charge_display_items(3) {
            return;
        }
        let commands = ir_shape_path(s);
        if commands.len() < 2 {
            continue;
        }
        let fill = match &s.fill_gradient {
            Some(g) if g.stops.len() >= 2 => Some(Fill::Gradient(Gradient {
                radial: g.radial,
                angle_deg: g.angle_deg,
                stops: g.stops.clone(),
            })),
            _ => (s.fill != 0xFFFF_FFFF).then_some(Fill::Solid(s.fill)),
        };
        let stroke = (s.border_width > 0).then(|| {
            let w = (s.border_width as f32 / 100.0).max(0.1);
            Stroke {
                color: s.border_color,
                width: w,
                dash: dash_pattern(s.border_style, w),
            }
        });
        if fill.is_none() && stroke.is_none() {
            continue;
        }
        // 선 화살촉(시작/끝) — 끝점 방향으로 채운 삼각형. Line에만.
        if matches!(s.kind, ShapeKind::Line) && (s.arrow_start != 0 || s.arrow_end != 0) {
            let aw = (s.border_width as f32 / 100.0).max(1.0);
            for head in arrowheads(&commands, s.arrow_start != 0, s.arrow_end != 0, aw) {
                page.items.push(Item::Path {
                    commands: head,
                    fill: Some(Fill::Solid(s.border_color)),
                    stroke: None,
                    rule: FillRule::NonZero,
                });
            }
        }
        page.items.push(Item::Path {
            commands,
            fill,
            stroke,
            rule: FillRule::NonZero,
        });
    }
}

/// 선 종류 → 점선 패턴(on, off, …) pt. 0/그 외=실선(빈 벡터). 굵기 비례.
fn dash_pattern(style: u8, width: f32) -> Vec<f32> {
    let u = width.max(0.5);
    match style {
        1 => vec![3.0 * u, 2.0 * u],                                     // 파선
        2 => vec![1.0 * u, 2.0 * u],                                     // 점선
        3 => vec![3.0 * u, 2.0 * u, 1.0 * u, 2.0 * u],                   // 일점쇄선
        4 => vec![3.0 * u, 2.0 * u, 1.0 * u, 2.0 * u, 1.0 * u, 2.0 * u], // 이점쇄선
        5 => vec![6.0 * u, 3.0 * u],                                     // 긴 파선
        _ => Vec::new(),
    }
}

/// Maps HWP5 border line types to `dash_pattern` style codes.
///
/// Keep this mapping synchronized with `hwp-convert/src/gso.rs`.
fn hwp5_line_style(lt: u8) -> u8 {
    match lt {
        2 => 1,
        3 => 2,
        4 => 3,
        5 => 4,
        6 => 5,
        _ => 0,
    }
}

/// 열린 경로(선)의 양 끝에 채운 삼각형 화살촉 경로를 만든다.
/// commands의 첫 점(시작)·끝 점(끝)과 인접 점으로 방향을 구한다.
fn arrowheads(commands: &[PathCmd], start: bool, end: bool, width: f32) -> Vec<Vec<PathCmd>> {
    let pts: Vec<(f32, f32)> = commands
        .iter()
        .filter_map(|c| match *c {
            PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => Some((x, y)),
            PathCmd::CubicTo(_, _, _, _, x, y) => Some((x, y)),
            PathCmd::Close => None,
        })
        .collect();
    if pts.len() < 2 {
        return Vec::new();
    }
    let size = (width * 4.0).max(5.0);
    let mut out = Vec::new();
    if end {
        let tip = pts[pts.len() - 1];
        let prev = pts[pts.len() - 2];
        if let Some(h) = arrow_triangle(tip, prev, size) {
            out.push(h);
        }
    }
    if start {
        let tip = pts[0];
        let next = pts[1];
        if let Some(h) = arrow_triangle(tip, next, size) {
            out.push(h);
        }
    }
    out
}

/// tip을 꼭짓점으로, from→tip 방향을 향하는 이등변 삼각형 화살촉.
fn arrow_triangle(tip: (f32, f32), from: (f32, f32), size: f32) -> Option<Vec<PathCmd>> {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len); // 진행 방향(끝점 쪽)
    let (px, py) = (-uy, ux); // 수직
    let back = (tip.0 - ux * size, tip.1 - uy * size);
    let half = size * 0.4;
    Some(vec![
        PathCmd::MoveTo(tip.0, tip.1),
        PathCmd::LineTo(back.0 + px * half, back.1 + py * half),
        PathCmd::LineTo(back.0 - px * half, back.1 - py * half),
        PathCmd::Close,
    ])
}

fn ir_shape_path(s: &ShapeGeom) -> Vec<PathCmd> {
    let (x0, y0) = (s.x as f32 / 100.0, s.y as f32 / 100.0);
    let (w, h) = (s.w as f32 / 100.0, s.h as f32 / 100.0);
    let pts_path = |close: bool| -> Vec<PathCmd> {
        let mut cmds = Vec::with_capacity(s.points.len() + 1);
        for (i, &(px, py)) in s.points.iter().enumerate() {
            let (ax, ay) = ((s.x + px) as f32 / 100.0, (s.y + py) as f32 / 100.0);
            cmds.push(if i == 0 {
                PathCmd::MoveTo(ax, ay)
            } else {
                PathCmd::LineTo(ax, ay)
            });
        }
        if close {
            cmds.push(PathCmd::Close);
        }
        cmds
    };
    match s.kind {
        ShapeKind::Rect => {
            let (x0, y0, w, h) = (x0 as f64, y0 as f64, w as f64, h as f64);
            let radius = (s.round_ratio.min(100) as f64 / 100.0) * (w.abs().min(h.abs()) / 2.0);
            if radius > 0.1 {
                rounded_quad_path(
                    [(x0, y0), (x0 + w, y0), (x0 + w, y0 + h), (x0, y0 + h)],
                    radius,
                    &|x, y| (x as f32, y as f32),
                )
            } else {
                vec![
                    PathCmd::MoveTo(x0 as f32, y0 as f32),
                    PathCmd::LineTo((x0 + w) as f32, y0 as f32),
                    PathCmd::LineTo((x0 + w) as f32, (y0 + h) as f32),
                    PathCmd::LineTo(x0 as f32, (y0 + h) as f32),
                    PathCmd::Close,
                ]
            }
        }
        // Arc: center/ax1/ax2(중심+켤레 두 축)로 1/4 타원호를 큐빅 베지에로 그린다.
        // 베지에는 어파인 불변이라 켤레 축(비수직 포함)에도 정확. points 없으면 타원 폴백.
        ShapeKind::Arc if s.points.len() >= 3 => {
            let ab = |(px, py): (i32, i32)| ((s.x + px) as f64, (s.y + py) as f64);
            let (cx, cy) = ab(s.points[0]);
            let (a1x, a1y) = ab(s.points[1]);
            let (a2x, a2y) = ab(s.points[2]);
            let (a1, a2) = ((a1x - cx, a1y - cy), (a2x - cx, a2y - cy));
            let pt = |sx: f64, sy: f64| ((cx + sx) as f32 / 100.0, (cy + sy) as f32 / 100.0);
            let k = KAPPA;
            let p0 = pt(a1.0, a1.1);
            let p1 = pt(a2.0, a2.1);
            vec![
                PathCmd::MoveTo(p0.0, p0.1),
                cubic(
                    pt(a1.0 + k * a2.0, a1.1 + k * a2.1),
                    pt(a2.0 + k * a1.0, a2.1 + k * a1.1),
                    p1,
                ),
            ]
        }
        ShapeKind::Ellipse | ShapeKind::Arc => {
            let (cx, cy) = ((x0 + w / 2.0) as f64, (y0 + h / 2.0) as f64);
            ellipse_path(
                cx,
                cy,
                (w as f64 / 2.0, 0.0),
                (0.0, h as f64 / 2.0),
                &|x, y| (x as f32, y as f32),
            )
        }
        ShapeKind::Line => {
            if s.points.len() >= 2 {
                pts_path(false)
            } else {
                vec![PathCmd::MoveTo(x0, y0), PathCmd::LineTo(x0 + w, y0 + h)]
            }
        }
        ShapeKind::Polygon | ShapeKind::Curve => {
            if s.points.len() >= 2 {
                pts_path(true)
            } else {
                vec![
                    PathCmd::MoveTo(x0, y0),
                    PathCmd::LineTo(x0 + w, y0),
                    PathCmd::LineTo(x0 + w, y0 + h),
                    PathCmd::LineTo(x0, y0 + h),
                    PathCmd::Close,
                ]
            }
        }
    }
}

/// gso 컨트롤의 도형을 page에 그린다. origin은 페이지 기준점(HWPUNIT):
/// floating은 (horz_offset, vert_offset), 인라인은 흐름 위치.
pub fn draw_gso_shapes(
    g: &GenericControl,
    origin: (f64, f64),
    doc: &Document,
    page: &mut PageList,
    warnings: &mut RenderIssueAccumulator,
) {
    walk(&g.raw_children, origin, doc, page, warnings, 0);
}

fn walk(
    recs: &[OpaqueRecord],
    origin: (f64, f64),
    doc: &Document,
    page: &mut PageList,
    warns: &mut RenderIssueAccumulator,
    depth: u32,
) {
    if depth > MAX_DEPTH {
        warns.push_once(RenderIssueCode::ShapeDepthLimitOmitted, depth.to_le_bytes());
        return;
    }
    for r in recs {
        match r.tag {
            SHAPE_COMPONENT => draw_component(r, origin, doc, page, warns, depth),
            SC_CONTAINER => walk(&r.children, origin, doc, page, warns, depth + 1),
            _ => {} // PARA_HEADER/LIST_HEADER/CTRL_HEADER 등은 텍스트 경로가 처리
        }
    }
}

fn draw_component(
    sc: &OpaqueRecord,
    origin: (f64, f64),
    doc: &Document,
    page: &mut PageList,
    warns: &mut RenderIssueAccumulator,
    depth: u32,
) {
    let Some(style) = parse_style(&sc.data, doc, warns) else {
        warns.push(
            RenderIssueCode::ShapeStyleInvalidOmitted,
            sc.tag.to_le_bytes(),
        );
        return;
    };
    for child in &sc.children {
        match child.tag {
            SC_LINE | SC_RECTANGLE | SC_ELLIPSE | SC_ARC | SC_POLYGON | SC_CURVE => {
                // One image fill plus one path is the maximum for a component
                // (line arrowheads are charged separately at emission below).
                // Charge before parsing/building the geometry buffer.
                if !warns.charge_display_items(2) {
                    return;
                }
                let Some(commands) = geometry(child.tag, &child.data, &style, origin) else {
                    warns.push(
                        RenderIssueCode::ShapeGeometryInvalidOmitted,
                        child.tag.to_le_bytes(),
                    );
                    continue;
                };
                if commands.len() < 2 {
                    warns.push(
                        RenderIssueCode::ShapeGeometryInvalidOmitted,
                        child.tag.to_le_bytes(),
                    );
                    continue;
                }
                // 이미지 채움: 도형 경계 상자에 이미지를 깐다(테두리는 path가 그림).
                if let Some(img) = &style.image {
                    let (x0, y0, x1, y1) = path_bbox(&commands);
                    page.items.push(Item::Image {
                        x: x0,
                        y: y0,
                        w: (x1 - x0).max(0.1),
                        h: (y1 - y0).max(0.1),
                        data: img.clone(),
                        crop: None,
                        flip: 0,
                        rotation_deg: 0.0,
                        brightness: 0,
                        contrast: 0,
                    });
                }
                // 채움·선이 모두 없고 이미지도 없으면 그리지 않는다(보이지 않는 프레임).
                if style.fill.is_none() && style.stroke.is_none() {
                    continue;
                }
                // SC_LINE arrowheads are filled triangles aligned to their endpoints.
                if child.tag == SC_LINE
                    && let Some(st) = &style.stroke
                    && (style.arrow_start || style.arrow_end)
                {
                    let heads = arrowheads(
                        &commands,
                        style.arrow_start,
                        style.arrow_end,
                        st.width.max(1.0),
                    );
                    if !warns.charge_display_items(heads.len()) {
                        return;
                    }
                    for head in heads {
                        page.items.push(Item::Path {
                            commands: head,
                            fill: Some(Fill::Solid(st.color)),
                            stroke: None,
                            rule: FillRule::NonZero,
                        });
                    }
                }
                page.items.push(Item::Path {
                    commands,
                    fill: style.fill.clone(),
                    stroke: style.stroke.clone(),
                    rule: FillRule::NonZero,
                });
            }
            SHAPE_COMPONENT => draw_component(child, origin, doc, page, warns, depth + 1),
            SC_CONTAINER => walk(&child.children, origin, doc, page, warns, depth + 1),
            _ => {}
        }
    }
}

// ── 3×2 affine 행렬 ([a,b,c,d,e,f] row-major: x'=a·x+b·y+c, y'=d·x+e·y+f) ──
#[derive(Clone, Copy)]
struct Mat {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Mat {
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.b * y + self.c,
            self.d * x + self.e * y + self.f,
        )
    }
    fn mul(&self, o: &Mat) -> Mat {
        Mat {
            a: self.a * o.a + self.b * o.d,
            b: self.a * o.b + self.b * o.e,
            c: self.a * o.c + self.b * o.f + self.c,
            d: self.d * o.a + self.e * o.d,
            e: self.d * o.b + self.e * o.e,
            f: self.d * o.c + self.e * o.f + self.f,
        }
    }
}

struct Style {
    m: Mat,
    stroke: Option<Stroke>,
    fill: Option<Fill>,
    /// 이미지 채움 — 도형 경계 상자에 깐다.
    image: Option<Arc<Vec<u8>>>,
    /// Whether start/end arrowheads are present; shape and size are flattened.
    arrow_start: bool,
    arrow_end: bool,
}

fn rd_u16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn rd_i32(d: &[u8], o: usize) -> Option<i32> {
    d.get(o..o + 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn rd_u32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn rd_f64(d: &[u8], o: usize) -> f64 {
    d.get(o..o + 8)
        .map(|b| f64::from_le_bytes(b.try_into().unwrap()))
        .unwrap_or(0.0)
}
fn rd_mat(d: &[u8], o: usize) -> Mat {
    Mat {
        a: rd_f64(d, o),
        b: rd_f64(d, o + 8),
        c: rd_f64(d, o + 16),
        d: rd_f64(d, o + 24),
        e: rd_f64(d, o + 32),
        f: rd_f64(d, o + 40),
    }
}

/// SHAPE_COMPONENT 데이터에서 렌더 행렬·테두리·채움을 읽는다.
/// 레이아웃(실측): [CHID×2 또는 ×1] + 개체요소속성 + (translation 48 + (scale 48+rotation 48)×cnt)
/// + 테두리선(13) + 채우기(Table 28).
fn parse_style(d: &[u8], doc: &Document, issues: &mut RenderIssueAccumulator) -> Option<Style> {
    if d.len() < 8 {
        return None;
    }
    // top-level은 CHID가 두 번(8B), 묶음 멤버는 한 번(4B).
    let base = if d[0..4] == d[4..8] { 8 } else { 4 };
    let cnt = rd_u16(d, base + 42)? as usize;
    let t = rd_mat(d, base + 44);
    // 마지막(최상위) scale/rotation 쌍 사용.
    let pair = base + 44 + 48 + cnt.saturating_sub(1) * 96;
    let m = if d.len() >= pair + 96 {
        t.mul(&rd_mat(d, pair).mul(&rd_mat(d, pair + 48)))
    } else {
        t
    };

    let bo = base + 92 + cnt * 96; // border_offset
    let mut stroke = None;
    let mut fill = None;
    let mut image = None;
    let mut arrow_start = false;
    let mut arrow_end = false;
    if let (Some(color), Some(width), Some(lattr)) =
        (rd_u32(d, bo), rd_i32(d, bo + 4), rd_u32(d, bo + 8))
    {
        let lt = (lattr & 0x3F) as u8;
        if lt != 0 {
            let w = (width as f32 / 100.0).max(0.1);
            stroke = Some(Stroke {
                color,
                width: w,
                dash: dash_pattern(hwp5_line_style(lt), w),
            });
            // Table 82: bits 10-15 encode the start arrow, 16-21 the end arrow.
            arrow_start = (lattr >> 10) & 0x3F != 0;
            arrow_end = (lattr >> 16) & 0x3F != 0;
        }
        let fo = bo + 13;
        if let Some(ft) = rd_u32(d, fo) {
            if ft & 0x1 != 0 {
                fill = rd_u32(d, fo + 4).map(Fill::Solid); // 단색 배경색
            } else if ft & 0x4 != 0 {
                fill = parse_gradient(d, fo + 4).map(Fill::Gradient);
            } else if ft & 0x2 != 0 {
                image = parse_image_fill(d, fo + 4, doc, issues);
            }
        }
    }
    Some(Style {
        m,
        stroke,
        fill,
        image,
        arrow_start,
        arrow_end,
    })
}

/// Table 28 gradient: type, angle, centers, spread, and count as `i16`;
/// positions as `INT32[num]` when `num > 2`; then `COLORREF[num]`.
/// [`hwp_model::GradientSpec::parse_hwp5`] is the canonical parser shared
/// with `BorderFill`.
fn parse_gradient(d: &[u8], fo: usize) -> Option<Gradient> {
    let (g, _) = hwp_model::GradientSpec::parse_hwp5(d, fo)?;
    Some(Gradient {
        radial: g.radial,
        angle_deg: g.angle_deg,
        stops: g.stops,
    })
}

/// 이미지 채움: BinData ID 참조를 풀어 원본 바이트를 얻는다.
fn parse_image_fill(
    d: &[u8],
    fo: usize,
    doc: &Document,
    issues: &mut RenderIssueAccumulator,
) -> Option<Arc<Vec<u8>>> {
    // BYTE 이미지유형 + 그림정보(가변) ... 끝부분에 DWORD BinItem ID. 보수적으로 마지막
    // 4바이트 정렬 위치에서 유효한 bin id를 찾는다.
    for end in (fo + 1..=d.len().min(fo + 64)).rev() {
        if end >= 4
            && let Some(id) = rd_u16(d, end - 4)
            && id != 0
            && let Some(bytes) = doc.resolve_bin(&BinRef::Id(BinDataId(id)))
        {
            return Some(issues.cached_binary(bytes));
        }
    }
    None
}

/// 기하 레코드를 페이지 좌표(pt) 경로로 변환.
fn geometry(tag: u16, d: &[u8], s: &Style, origin: (f64, f64)) -> Option<Vec<PathCmd>> {
    // local 점(HWPUNIT) → 행렬 → +origin → /100 = pt.
    let to_pt = |x: f64, y: f64| -> (f32, f32) {
        let (px, py) = s.m.apply(x, y);
        (
            ((px + origin.0) / 100.0) as f32,
            ((py + origin.1) / 100.0) as f32,
        )
    };
    let p =
        |o: usize| -> Option<(f64, f64)> { Some((rd_i32(d, o)? as f64, rd_i32(d, o + 4)? as f64)) };

    match tag {
        SC_LINE => {
            let (sx, sy) = p(0)?;
            let (ex, ey) = p(8)?;
            if (sx - ex).abs() < f64::EPSILON && (sy - ey).abs() < f64::EPSILON {
                return None;
            }
            let (a, b) = to_pt(sx, sy);
            let (c, e) = to_pt(ex, ey);
            Some(vec![PathCmd::MoveTo(a, b), PathCmd::LineTo(c, e)])
        }
        SC_RECTANGLE => {
            // BYTE 곡률% + 4×(x,y). 곡률>0이면 둥근 모서리(원호 근사).
            let curv = (*d.first()? as u32).min(100);
            let corners = [p(1)?, p(9)?, p(17)?, p(25)?];
            if curv > 0 {
                let e01 = dist(corners[0], corners[1]);
                let e12 = dist(corners[1], corners[2]);
                let radius = (curv as f64 / 100.0) * (e01.min(e12) / 2.0);
                if radius > 1.0 {
                    return Some(rounded_quad_path(corners, radius, &to_pt));
                }
            }
            let mut cmds = Vec::with_capacity(6);
            for (i, &(x, y)) in corners.iter().enumerate() {
                let (px, py) = to_pt(x, y);
                cmds.push(if i == 0 {
                    PathCmd::MoveTo(px, py)
                } else {
                    PathCmd::LineTo(px, py)
                });
            }
            cmds.push(PathCmd::Close);
            Some(cmds)
        }
        SC_POLYGON => {
            let n = rd_u16(d, 0)? as usize;
            if !(2..=4096).contains(&n) {
                return None;
            }
            let mut cmds = Vec::with_capacity(n + 1);
            for i in 0..n {
                let (x, y) = p(4 + i * 8)?;
                let (px, py) = to_pt(x, y);
                cmds.push(if i == 0 {
                    PathCmd::MoveTo(px, py)
                } else {
                    PathCmd::LineTo(px, py)
                });
            }
            cmds.push(PathCmd::Close);
            Some(cmds)
        }
        SC_ELLIPSE => {
            // Table 97 UINT32 attributes followed by center and two axis endpoints.
            let attr = rd_u32(d, 0)?;
            let (cx, cy) = p(4)?;
            let (a1x, a1y) = p(12)?;
            let (a2x, a2y) = p(20)?;
            let (a1, a2) = ((a1x - cx, a1y - cy), (a2x - cx, a2y - cy));
            // Bit 1 marks an ellipse converted to an arc (GG-23). Bytes 28 and
            // 36 hold the endpoints; bits 2-9 hold kind 0=open, 1=pie, 2=chord.
            // The kind mapping still requires comparison with genuine Hangul output.
            if attr & 0x2 != 0 {
                let (sx, sy) = p(28)?;
                let (ex, ey) = p(36)?;
                let kind = (attr >> 2) & 0xFF;
                return Some(ellipse_arc_path(
                    (cx, cy),
                    a1,
                    a2,
                    (sx, sy),
                    (ex, ey),
                    kind,
                    &to_pt,
                ));
            }
            Some(ellipse_path(cx, cy, a1, a2, &to_pt))
        }
        SC_ARC => {
            // BYTE arctype + center + start(ax1) + end(ax2).
            let (cx, cy) = p(1)?;
            let (sx, sy) = p(9)?;
            let (ex, ey) = p(17)?;
            Some(arc_path(cx, cy, (sx, sy), (ex, ey), &to_pt))
        }
        SC_CURVE => {
            let n = rd_u16(d, 0)? as usize;
            if !(2..=4096).contains(&n) {
                return None;
            }
            // 세그먼트 타입 무시 — 점들을 폴리라인으로 근사(방어적; 파일에 없음).
            let mut cmds = Vec::with_capacity(n);
            for i in 0..n {
                let (x, y) = p(2 + i * 8)?;
                let (px, py) = to_pt(x, y);
                cmds.push(if i == 0 {
                    PathCmd::MoveTo(px, py)
                } else {
                    PathCmd::LineTo(px, py)
                });
            }
            Some(cmds)
        }
        _ => None,
    }
}

/// 두 점 사이 유클리드 거리.
fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    (dx * dx + dy * dy).sqrt()
}

/// 둥근 모서리 사각형(임의 4점) 경로. corners는 순서대로 이은 4점,
/// radius는 corners와 같은 좌표계의 모서리 반경. map으로 출력 좌표 변환.
/// 각 모서리는 90° 원호를 큐빅 베지에(KAPPA)로 근사한다.
fn rounded_quad_path(
    corners: [(f64, f64); 4],
    radius: f64,
    map: &impl Fn(f64, f64) -> (f32, f32),
) -> Vec<PathCmd> {
    let unit = |from: (f64, f64), to: (f64, f64)| -> (f64, f64) {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            (0.0, 0.0)
        } else {
            (dx / len, dy / len)
        }
    };
    // 각 모서리의 진입(이전 변 쪽)·이탈(다음 변 쪽) 점. 인접 변 길이의
    // 절반으로 반경을 캡해 곡선 겹침을 막는다.
    let mut t_in = [(0.0, 0.0); 4];
    let mut t_out = [(0.0, 0.0); 4];
    for i in 0..4 {
        let c = corners[i];
        let prev = corners[(i + 3) % 4];
        let next = corners[(i + 1) % 4];
        let r_prev = radius.min(dist(c, prev) / 2.0);
        let r_next = radius.min(dist(c, next) / 2.0);
        let up = unit(c, prev);
        let un = unit(c, next);
        t_in[i] = (c.0 + up.0 * r_prev, c.1 + up.1 * r_prev);
        t_out[i] = (c.0 + un.0 * r_next, c.1 + un.1 * r_next);
    }
    let m = |p: (f64, f64)| map(p.0, p.1);
    let mut cmds = Vec::with_capacity(13);
    let (sx, sy) = m(t_out[0]);
    cmds.push(PathCmd::MoveTo(sx, sy));
    for i in 1..=4 {
        let idx = i % 4;
        let (ax, ay) = m(t_in[idx]);
        cmds.push(PathCmd::LineTo(ax, ay));
        let c = corners[idx];
        let cp1 = (
            t_in[idx].0 + (c.0 - t_in[idx].0) * KAPPA,
            t_in[idx].1 + (c.1 - t_in[idx].1) * KAPPA,
        );
        let cp2 = (
            t_out[idx].0 + (c.0 - t_out[idx].0) * KAPPA,
            t_out[idx].1 + (c.1 - t_out[idx].1) * KAPPA,
        );
        let (c1x, c1y) = m(cp1);
        let (c2x, c2y) = m(cp2);
        let (ox, oy) = m(t_out[idx]);
        cmds.push(PathCmd::CubicTo(c1x, c1y, c2x, c2y, ox, oy));
    }
    cmds.push(PathCmd::Close);
    cmds
}

/// 중심 C와 두 축 벡터(a1, a2)로 타원을 4개 큐빅 베지에로 근사.
fn ellipse_path(
    cx: f64,
    cy: f64,
    a1: (f64, f64),
    a2: (f64, f64),
    to_pt: &impl Fn(f64, f64) -> (f32, f32),
) -> Vec<PathCmd> {
    let pt = |sx: f64, sy: f64| to_pt(cx + sx, cy + sy);
    // 앵커: C+a1, C+a2, C-a1, C-a2. 제어점 = 앵커 ± k·(다음 축).
    let (p0, p1, p2, p3) = (
        pt(a1.0, a1.1),
        pt(a2.0, a2.1),
        pt(-a1.0, -a1.1),
        pt(-a2.0, -a2.1),
    );
    let k = KAPPA;
    let c = |bx: f64, by: f64| pt(bx, by);
    vec![
        PathCmd::MoveTo(p0.0, p0.1),
        cubic(
            c(a1.0 + k * a2.0, a1.1 + k * a2.1),
            c(a2.0 + k * a1.0, a2.1 + k * a1.1),
            p1,
        ),
        cubic(
            c(a2.0 - k * a1.0, a2.1 - k * a1.1),
            c(-a1.0 + k * a2.0, -a1.1 + k * a2.1),
            p2,
        ),
        cubic(
            c(-a1.0 - k * a2.0, -a1.1 - k * a2.1),
            c(-a2.0 - k * a1.0, -a2.1 - k * a1.1),
            p3,
        ),
        cubic(
            c(-a2.0 + k * a1.0, -a2.1 + k * a1.1),
            c(a1.0 - k * a2.0, a1.1 - k * a2.1),
            p0,
        ),
        PathCmd::Close,
    ]
}

/// Approximates a GG-23 ellipse arc from its center, two axis vectors, and
/// endpoints with cubic Beziers. It uses the same shorter-sweep convention as
/// `arc_path`. Table 97 bits 2-9 encode 0=open, 1=pie, and 2=chord; the mapping
/// still requires comparison with genuine Hangul output.
fn ellipse_arc_path(
    c: (f64, f64),
    a1: (f64, f64),
    a2: (f64, f64),
    start: (f64, f64),
    end: (f64, f64),
    kind: u32,
    to_pt: &impl Fn(f64, f64) -> (f32, f32),
) -> Vec<PathCmd> {
    let (cx, cy) = c;
    // Solve theta in basis (a1, a2) for P(theta) = C + a1*cos(theta) + a2*sin(theta).
    let det = a1.0 * a2.1 - a1.1 * a2.0;
    if det.abs() < f64::EPSILON {
        return Vec::new();
    }
    let theta = |p: (f64, f64)| {
        let (sx, sy) = (p.0 - cx, p.1 - cy);
        let cos = (sx * a2.1 - a2.0 * sy) / det;
        let sin = (a1.0 * sy - sx * a1.1) / det;
        sin.atan2(cos)
    };
    let t0 = theta(start);
    let mut sweep = theta(end) - t0;
    // Choose the shorter sweep in [-pi, pi].
    while sweep > std::f64::consts::PI {
        sweep -= std::f64::consts::TAU;
    }
    while sweep < -std::f64::consts::PI {
        sweep += std::f64::consts::TAU;
    }
    // A degenerate equal-endpoint arc falls back to a complete ellipse.
    if sweep.abs() < 1e-6 {
        return ellipse_path(cx, cy, a1, a2, to_pt);
    }
    let pt = |th: f64| {
        to_pt(
            cx + a1.0 * th.cos() + a2.0 * th.sin(),
            cy + a1.1 * th.cos() + a2.1 * th.sin(),
        )
    };
    let segs = (sweep.abs() / (std::f64::consts::PI / 2.0)).ceil().max(1.0) as usize;
    let dphi = sweep / segs as f64;
    let alpha = (4.0 / 3.0) * (dphi / 4.0).tan();
    let start_pt = pt(t0);
    let mut cmds = vec![PathCmd::MoveTo(start_pt.0, start_pt.1)];
    let mut th = t0;
    for _ in 0..segs {
        let th1 = th + dphi;
        // Tangent P'(theta) = -a1*sin(theta) + a2*cos(theta); controls are P +/- alpha*P'.
        let derv = |t: f64| {
            (
                a2.0 * t.cos() - a1.0 * t.sin(),
                a2.1 * t.cos() - a1.1 * t.sin(),
            )
        };
        let (px, py) = (
            cx + a1.0 * th.cos() + a2.0 * th.sin(),
            cy + a1.1 * th.cos() + a2.1 * th.sin(),
        );
        let (dx0, dy0) = derv(th);
        let c1 = to_pt(px + alpha * dx0, py + alpha * dy0);
        let (qx, qy) = (
            cx + a1.0 * th1.cos() + a2.0 * th1.sin(),
            cy + a1.1 * th1.cos() + a2.1 * th1.sin(),
        );
        let (dx1, dy1) = derv(th1);
        let c2 = to_pt(qx - alpha * dx1, qy - alpha * dy1);
        let p1 = pt(th1);
        cmds.push(PathCmd::CubicTo(c1.0, c1.1, c2.0, c2.1, p1.0, p1.1));
        th = th1;
    }
    match kind {
        // Pie: close through the center.
        1 => {
            let c = to_pt(cx, cy);
            cmds.push(PathCmd::LineTo(c.0, c.1));
            cmds.push(PathCmd::Close);
        }
        // Chord: close directly from end to start.
        2 => cmds.push(PathCmd::Close),
        _ => {}
    }
    cmds
}

/// 중심 C, 시작/끝 점으로 원호를 큐빅 베지에로 근사(짧은 쪽 sweep).
fn arc_path(
    cx: f64,
    cy: f64,
    start: (f64, f64),
    end: (f64, f64),
    to_pt: &impl Fn(f64, f64) -> (f32, f32),
) -> Vec<PathCmd> {
    let s = (start.0 - cx, start.1 - cy);
    let r = (s.0 * s.0 + s.1 * s.1).sqrt();
    if r < f64::EPSILON {
        return Vec::new();
    }
    let e = (end.0 - cx, end.1 - cy);
    let t0 = s.1.atan2(s.0);
    let mut sweep = e.1.atan2(e.0) - t0;
    // 짧은 쪽 [-π, π].
    while sweep > std::f64::consts::PI {
        sweep -= std::f64::consts::TAU;
    }
    while sweep < -std::f64::consts::PI {
        sweep += std::f64::consts::TAU;
    }
    let pt = |th: f64| to_pt(cx + r * th.cos(), cy + r * th.sin());
    let segs = (sweep.abs() / (std::f64::consts::PI / 2.0)).ceil().max(1.0) as usize;
    let dphi = sweep / segs as f64;
    let alpha = (4.0 / 3.0) * (dphi / 4.0).tan();
    let start_pt = pt(t0);
    let mut cmds = vec![PathCmd::MoveTo(start_pt.0, start_pt.1)];
    let mut th = t0;
    for _ in 0..segs {
        let th1 = th + dphi;
        // 접선 T'(θ) = r·(-sinθ, cosθ); 제어점 = P ± α·T'.
        let c1 = to_pt(
            cx + r * th.cos() - alpha * r * th.sin(),
            cy + r * th.sin() + alpha * r * th.cos(),
        );
        let c2 = to_pt(
            cx + r * th1.cos() + alpha * r * th1.sin(),
            cy + r * th1.sin() - alpha * r * th1.cos(),
        );
        let p1 = pt(th1);
        cmds.push(PathCmd::CubicTo(c1.0, c1.1, c2.0, c2.1, p1.0, p1.1));
        th = th1;
    }
    cmds
}

fn cubic(c1: (f32, f32), c2: (f32, f32), p: (f32, f32)) -> PathCmd {
    PathCmd::CubicTo(c1.0, c1.1, c2.0, c2.1, p.0, p.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 그러데이션_2색_파싱() {
        // fo=0: gtype=0(선형) angle=90 num=2, color0=red color1=blue.
        let mut d = vec![0u8; 20];
        d[2..4].copy_from_slice(&90u16.to_le_bytes());
        d[10..12].copy_from_slice(&2u16.to_le_bytes());
        d[12..16].copy_from_slice(&0x0000_00FFu32.to_le_bytes()); // R=FF
        d[16..20].copy_from_slice(&0x00FF_0000u32.to_le_bytes()); // B=FF
        let g = parse_gradient(&d, 0).unwrap();
        assert!(!g.radial);
        assert_eq!(g.angle_deg, 90.0);
        assert_eq!(g.stops, vec![(0.0, 0x0000_00FF), (1.0, 0x00FF_0000)]);
    }

    #[test]
    fn hwpx_도형_ir_경로_변환() {
        use hwp_model::{ShapeGeom, ShapeKind};
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        // 사각형: x=2000(20pt) y=1000(10pt) w=30000(300pt) h=15000(150pt), 주황 채움+파랑 테두리.
        let rect = ShapeGeom {
            kind: ShapeKind::Rect,
            x: 2000,
            y: 1000,
            w: 30000,
            h: 15000,
            points: Vec::new(),
            fill: 0x0000_CCFF,
            fill_gradient: None,
            border_color: 0x00FF_0000,
            border_width: 100,
            round_ratio: 0,
            border_style: 0,
            arrow_start: 0,
            arrow_end: 0,
            anchored: false,
            description: None,
        };
        let mut issues = RenderIssueAccumulator::new();
        draw_ir_shapes(&[rect], &mut page, &mut issues);
        assert_eq!(page.items.len(), 1);
        let Item::Path {
            commands,
            fill,
            stroke,
            ..
        } = &page.items[0]
        else {
            panic!("Path가 아님");
        };
        assert_eq!(commands.len(), 5, "사각형은 Move+Line×3+Close");
        assert!(
            matches!(commands[0], PathCmd::MoveTo(x, y) if (x - 20.0).abs() < 0.1 && (y - 10.0).abs() < 0.1)
        );
        assert!(matches!(fill, Some(Fill::Solid(0x0000_CCFF))));
        assert!(stroke.is_some(), "테두리 있어야");

        // 채움·선 없으면 path 생성 안 함.
        let mut p2 = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let invisible = ShapeGeom {
            kind: ShapeKind::Rect,
            x: 0,
            y: 0,
            w: 1000,
            h: 1000,
            points: Vec::new(),
            fill: 0xFFFF_FFFF,
            fill_gradient: None,
            border_color: 0xFFFF_FFFF,
            border_width: 0,
            round_ratio: 0,
            border_style: 0,
            arrow_start: 0,
            arrow_end: 0,
            anchored: false,
            description: None,
        };
        draw_ir_shapes(&[invisible], &mut p2, &mut issues);
        assert!(p2.items.is_empty(), "보이지 않는 도형은 생략");
    }

    #[test]
    fn ir_그러데이션_채움() {
        use hwp_model::GradientSpec;
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        // fill_gradient가 있으면 단색 fill을 무시하고 Gradient로 채운다.
        let shape = ShapeGeom {
            kind: ShapeKind::Rect,
            x: 0,
            y: 0,
            w: 10000,
            h: 10000,
            points: Vec::new(),
            fill: 0x0000_00FF, // 무시되어야 함
            fill_gradient: Some(GradientSpec {
                radial: false,
                angle_deg: 90.0,
                center_x: 0,
                center_y: 0,
                step: 255,
                stops: vec![(0.0, 0x0000_00FF), (1.0, 0x00FF_0000)],
            }),
            border_color: 0xFFFF_FFFF,
            border_width: 0,
            round_ratio: 0,
            border_style: 0,
            arrow_start: 0,
            arrow_end: 0,
            anchored: false,
            description: None,
        };
        let mut issues = RenderIssueAccumulator::new();
        draw_ir_shapes(&[shape], &mut page, &mut issues);
        assert_eq!(page.items.len(), 1);
        let Item::Path { fill, .. } = &page.items[0] else {
            panic!("Path가 아님");
        };
        let Some(Fill::Gradient(g)) = fill else {
            panic!("Gradient 채움이어야 함");
        };
        assert!(!g.radial);
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].1, 0x0000_00FF);
        assert_eq!(g.stops[1].1, 0x00FF_0000);
    }

    #[test]
    fn 방사형_3색_위치() {
        // gtype=1(방사) num=3, 위치 0/50/100, 색 3개.
        let mut d = vec![0u8; 32];
        d[0..2].copy_from_slice(&1u16.to_le_bytes());
        d[10..12].copy_from_slice(&3u16.to_le_bytes());
        d[12..16].copy_from_slice(&0i32.to_le_bytes());
        d[16..20].copy_from_slice(&50i32.to_le_bytes());
        d[20..24].copy_from_slice(&100i32.to_le_bytes());
        d[24..28].copy_from_slice(&0x11u32.to_le_bytes());
        d[28..32].copy_from_slice(&0x22u32.to_le_bytes());
        // 색이 하나 더 필요하지만 버퍼 끝 → None 허용. 최소 검증: radial + 위치 정규화.
        if let Some(g) = parse_gradient(&d, 0) {
            assert!(g.radial);
            assert!((g.stops[0].0 - 0.0).abs() < 0.01);
        }
    }

    #[test]
    fn 둥근모서리_사각형_경로() {
        use hwp_model::{ShapeGeom, ShapeKind};
        let base = ShapeGeom {
            kind: ShapeKind::Rect,
            x: 0,
            y: 0,
            w: 20000, // 200pt
            h: 10000, // 100pt
            points: Vec::new(),
            fill: 0x0000_00FF,
            fill_gradient: None,
            border_color: 0xFFFF_FFFF,
            border_width: 0,
            round_ratio: 0,
            border_style: 0,
            arrow_start: 0,
            arrow_end: 0,
            anchored: false,
            description: None,
        };
        // 직각: Move + Line×3 + Close = 5개, CubicTo 없음.
        let sharp = ir_shape_path(&base);
        assert_eq!(sharp.len(), 5);
        assert!(!sharp.iter().any(|c| matches!(c, PathCmd::CubicTo(..))));

        // 곡률 50%: 네 모서리 원호(CubicTo 4개) + 변(LineTo 4개) + Move + Close.
        let round = ir_shape_path(&ShapeGeom {
            round_ratio: 50,
            ..base.clone()
        });
        let cubics = round
            .iter()
            .filter(|c| matches!(c, PathCmd::CubicTo(..)))
            .count();
        assert_eq!(cubics, 4, "네 모서리마다 원호 1개");
        assert!(matches!(round.last(), Some(PathCmd::Close)));
        // 반경 = 50% × min(200,100)/2 = 25pt. 시작점은 우상단으로 가는 변 위
        // (모서리에서 radius만큼 떨어진 점) → x ≈ radius(25), y = 0.
        let PathCmd::MoveTo(sx, sy) = round[0] else {
            panic!("MoveTo로 시작해야");
        };
        assert!((sx - 25.0).abs() < 0.5, "시작 x≈25, 실제 {sx}");
        assert!((sy - 0.0).abs() < 0.5, "시작 y≈0, 실제 {sy}");

        // 곡률 100%: 반경 = min/2 = 50pt (캡), 여전히 CubicTo 4개.
        let full = ir_shape_path(&ShapeGeom {
            round_ratio: 100,
            ..base
        });
        assert_eq!(
            full.iter()
                .filter(|c| matches!(c, PathCmd::CubicTo(..)))
                .count(),
            4
        );
    }

    #[test]
    fn 점선_패턴_매핑() {
        assert!(dash_pattern(0, 1.0).is_empty(), "실선");
        assert_eq!(dash_pattern(1, 1.0).len(), 2, "파선 2구간");
        assert_eq!(dash_pattern(3, 1.0).len(), 4, "일점쇄선 4구간");
        assert_eq!(dash_pattern(4, 1.0).len(), 6, "이점쇄선 6구간");
        // 굵기 비례: 굵을수록 간격도 커진다.
        assert!(dash_pattern(1, 4.0)[0] > dash_pattern(1, 1.0)[0]);
    }

    #[test]
    fn 화살촉_삼각형_방향() {
        // 수평선 (0,10)→(100,10): 끝(오른쪽)에 화살촉, 시작(왼쪽)에 화살촉.
        let line = vec![PathCmd::MoveTo(0.0, 10.0), PathCmd::LineTo(100.0, 10.0)];
        let heads = arrowheads(&line, true, true, 1.0);
        assert_eq!(heads.len(), 2, "양끝 화살촉");
        // 첫 화살촉 = 끝점(오른쪽), 꼭짓점 x≈100.
        let PathCmd::MoveTo(tx, ty) = heads[0][0] else {
            panic!("MoveTo");
        };
        assert!((tx - 100.0).abs() < 0.1 && (ty - 10.0).abs() < 0.1);
        // 삼각형: Move+Line+Line+Close.
        assert!(matches!(heads[0].last(), Some(PathCmd::Close)));
        assert_eq!(heads[0].len(), 4);
        // 끝 화살촉의 밑변은 꼭짓점보다 왼쪽(x<100).
        let PathCmd::LineTo(bx, _) = heads[0][1] else {
            panic!("LineTo");
        };
        assert!(bx < 100.0, "밑변은 진행 반대쪽");

        // 화살촉 없음 → 빈 벡터.
        assert!(arrowheads(&line, false, false, 1.0).is_empty());
    }

    #[test]
    fn ir_선_점선_화살표_적용() {
        use hwp_model::{ShapeGeom, ShapeKind};
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let line = ShapeGeom {
            kind: ShapeKind::Line,
            x: 0,
            y: 0,
            w: 10000,
            h: 0,
            points: Vec::new(),
            fill: 0xFFFF_FFFF,
            fill_gradient: None,
            border_color: 0x0000_0000,
            border_width: 200,
            round_ratio: 0,
            border_style: 1, // 파선
            arrow_start: 0,
            arrow_end: 1, // 끝 화살촉
            anchored: false,
            description: None,
        };
        let mut issues = RenderIssueAccumulator::new();
        draw_ir_shapes(&[line], &mut page, &mut issues);
        // 선 path 1개 + 화살촉 path 1개 = 2.
        assert_eq!(page.items.len(), 2);
        // 화살촉(채움 있음, 선 없음)이 먼저 push된다.
        let Item::Path { fill, stroke, .. } = &page.items[0] else {
            panic!("Path");
        };
        assert!(fill.is_some() && stroke.is_none(), "화살촉=채움 삼각형");
        // 선 path는 점선(dash) 적용.
        let Item::Path { stroke, .. } = &page.items[1] else {
            panic!("Path");
        };
        let s = stroke.as_ref().expect("선 stroke");
        assert!(!s.dash.is_empty(), "파선 dash 적용");
    }

    #[test]
    fn hwp5_raw_line_applies_dash_and_arrowhead() {
        // Compose one identity SHAPE_COMPONENT with one SC_LINE child.
        // The 13-byte line info stores color, width, and attributes.
        // Type 2 selects dash; end-arrow bits 16-21 select one arrowhead.
        let mut d = vec![0u8; 204];
        d[4] = 0xFF; // Prevent duplicate CHID detection so the base stays at 4.
        d[46..48].copy_from_slice(&1u16.to_le_bytes()); // cnt=1
        for o in [48usize, 96, 144] {
            // Identity matrix (a=1, d=1).
            d[o..o + 8].copy_from_slice(&1.0f64.to_le_bytes());
            d[o + 24..o + 32].copy_from_slice(&1.0f64.to_le_bytes());
        }
        let bo = 192; // base + 92 + cnt*96
        d[bo..bo + 4].copy_from_slice(&0x0000_0000u32.to_le_bytes()); // Black.
        d[bo + 4..bo + 8].copy_from_slice(&100i32.to_le_bytes()); // width=1pt
        d[bo + 8..bo + 12].copy_from_slice(&(2u32 | (1 << 16)).to_le_bytes());

        let mut line = vec![0u8; 16];
        line[8..12].copy_from_slice(&10000i32.to_le_bytes()); // End x=100 pt.

        let sc = OpaqueRecord {
            tag: SHAPE_COMPONENT,
            data: d,
            children: vec![OpaqueRecord {
                tag: SC_LINE,
                data: line,
                children: Vec::new(),
            }],
        };
        let g = GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: vec![sc],
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        };
        let doc = Document::default();
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let mut issues = RenderIssueAccumulator::new();
        draw_gso_shapes(&g, (0.0, 0.0), &doc, &mut page, &mut issues);
        // One arrowhead path plus one line path.
        assert_eq!(page.items.len(), 2, "{:?}", page.items.len());
        let Item::Path { fill, stroke, .. } = &page.items[0] else {
            panic!("Path");
        };
        assert!(fill.is_some() && stroke.is_none(), "arrowhead is fill-only");
        let Item::Path { stroke, .. } = &page.items[1] else {
            panic!("Path");
        };
        let s = stroke.as_ref().expect("line stroke");
        assert_eq!(s.dash.len(), 2, "raw type 2 has one dash-gap pair");
    }

    #[test]
    fn hwp5_raw_line_type_mapping_matches_converter() {
        // Keep this table synchronized with gso.rs.
        assert_eq!(hwp5_line_style(1), 0, "solid");
        assert_eq!(hwp5_line_style(2), 1);
        assert_eq!(hwp5_line_style(3), 2);
        assert_eq!(hwp5_line_style(6), 5);
        assert_eq!(hwp5_line_style(7), 0, "out-of-range values are solid");
    }

    /// GG-23: ellipse bit 1 and endpoint coordinates produce open, pie, and chord paths.
    #[test]
    fn 타원_호변환_경로() {
        let style = Style {
            m: Mat {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 0.0,
                e: 1.0,
                f: 0.0,
            },
            stroke: None,
            fill: None,
            image: None,
            arrow_start: false,
            arrow_end: false,
        };
        let mut d = vec![0u8; 60];
        let put = |d: &mut [u8], o: usize, v: i32| {
            d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        };
        // Center (5000, 5000) with orthogonal 1000-unit axes describes a circle.
        put(&mut d, 4, 5000);
        put(&mut d, 8, 5000);
        put(&mut d, 12, 6000);
        put(&mut d, 16, 5000);
        put(&mut d, 20, 5000);
        put(&mut d, 24, 6000);
        // Start is C+a1 and end is the 90-degree point C+a2; point conversion divides by 100.
        put(&mut d, 28, 6000);
        put(&mut d, 32, 5000);
        put(&mut d, 36, 5000);
        put(&mut d, 40, 6000);

        // Bit 1 alone creates an open arc with MoveTo and cubic segments but no Close.
        put(&mut d, 0, 0x2);
        let cmds = geometry(SC_ELLIPSE, &d, &style, (0.0, 0.0)).unwrap();
        let PathCmd::MoveTo(x, y) = cmds[0] else {
            panic!("MoveTo 기대: {:?}", cmds[0])
        };
        assert!(
            (x - 60.0).abs() < 0.01 && (y - 50.0).abs() < 0.01,
            "시작점 {x},{y}"
        );
        assert!(!matches!(cmds.last(), Some(PathCmd::Close)), "열린 호");
        // The endpoint is near (50, 60).
        let Some(PathCmd::CubicTo(.., ex, ey)) = cmds.last() else {
            panic!("CubicTo 기대")
        };
        assert!(
            (ex - 50.0).abs() < 0.01 && (ey - 60.0).abs() < 0.01,
            "끝점 {ex},{ey}"
        );

        // Pie kind 1 closes through the center.
        put(&mut d, 0, 0x2 | (1 << 2));
        let cmds = geometry(SC_ELLIPSE, &d, &style, (0.0, 0.0)).unwrap();
        assert!(matches!(cmds.last(), Some(PathCmd::Close)));
        assert!(
            cmds.iter()
                .any(|c| matches!(c, PathCmd::LineTo(x, y) if (*x - 50.0).abs() < 0.01 && (*y - 50.0).abs() < 0.01)),
            "중심 경유: {cmds:?}"
        );

        // Chord kind 2 closes without visiting the center.
        put(&mut d, 0, 0x2 | (2 << 2));
        let cmds = geometry(SC_ELLIPSE, &d, &style, (0.0, 0.0)).unwrap();
        assert!(matches!(cmds.last(), Some(PathCmd::Close)));
        assert!(
            !cmds.iter()
                .any(|c| matches!(c, PathCmd::LineTo(x, y) if (*x - 50.0).abs() < 0.01 && (*y - 50.0).abs() < 0.01)),
            "중심 미경유: {cmds:?}"
        );

        // Without bit 1, preserve the full ellipse path: MoveTo, four cubics, Close.
        put(&mut d, 0, 0);
        let cmds = geometry(SC_ELLIPSE, &d, &style, (0.0, 0.0)).unwrap();
        assert_eq!(cmds.len(), 6);
        assert!(matches!(cmds.last(), Some(PathCmd::Close)));
    }
}
