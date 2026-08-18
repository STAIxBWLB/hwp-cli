//! hwp5 gso 도형 레코드(SHAPE_COMPONENT 서브트리) → 구조화 도형([`ShapeGeom`]) 변환.
//!
//! hwpx writer가 hwp→hwpx 변환에서 장식 도형·글상자 테두리를 보존할 때 쓴다.
//! 바이트 레이아웃은 `hwp-render/src/shape_draw.rs`(렌더 전용 파서 `parse_style`/`geometry`)와
//! 동일 실측 — 오프셋을 고치면 양쪽을 함께 봐야 한다(hwpx→hwp-render 역의존 불가라 사본).
//!
//! 정답지: 코퍼스 (원본.hwp ↔ 한글 export hwpx) 쌍 — 장식선의 SHAPE_COMPONENT(252B, CHID
//! "$lin"×2, scale 행렬 496.08/0.04, border width=32) + SC_LINE local (0,0)→(100,100)이
//! 한글 export의 `<hp:line>` curSz 49608×4·lineShape width=32·startPt/endPt와 정확 대응.

use hwp_model::opaque::OpaqueRecord;
use hwp_model::{GradientSpec, ShapeGeom, ShapeKind};

// 실제 레코드 태그 값(HWPTAG_BEGIN 0x10 + 60…) — shape_draw.rs·실측 JSON 덤프와 일치.
const SHAPE_COMPONENT: u16 = 0x4C;
const SC_LINE: u16 = 0x4E;
const SC_RECTANGLE: u16 = 0x4F;
const SC_ELLIPSE: u16 = 0x50;
const SC_ARC: u16 = 0x51;
const SC_POLYGON: u16 = 0x52;
const SC_CURVE: u16 = 0x53;
const SC_CONTAINER: u16 = 0x56;
const MAX_DEPTH: u32 = 8;

/// gso `raw_children`(SHAPE_COMPONENT 서브트리)에서 도형들을 추출한다.
/// 좌표는 렌더 행렬을 적용한 HWPUNIT, gso 박스 원점 기준. `points`는 bbox 원점 상대
/// (ShapeGeom 규약). SC_ARC·이미지 채움은 v1 제외(스킵).
pub fn shapes_from_raw(raw: &[OpaqueRecord]) -> Vec<ShapeGeom> {
    let mut out = Vec::new();
    walk(raw, &mut out, 0);
    out
}

fn walk(recs: &[OpaqueRecord], out: &mut Vec<ShapeGeom>, depth: u32) {
    if depth > MAX_DEPTH {
        return;
    }
    for r in recs {
        match r.tag {
            SHAPE_COMPONENT => component(r, out, depth),
            SC_CONTAINER => walk(&r.children, out, depth + 1),
            _ => {}
        }
    }
}

fn component(sc: &OpaqueRecord, out: &mut Vec<ShapeGeom>, depth: u32) {
    let Some(style) = parse_style(&sc.data) else {
        return;
    };
    for child in &sc.children {
        match child.tag {
            SC_LINE | SC_RECTANGLE | SC_ELLIPSE | SC_ARC | SC_POLYGON | SC_CURVE => {
                if let Some(shape) = geometry(child.tag, &child.data, &style) {
                    out.push(shape);
                }
            }
            SHAPE_COMPONENT => component(child, out, depth + 1),
            SC_CONTAINER => walk(&child.children, out, depth + 1),
            _ => {} // 이미지 채움 등 v1 제외
        }
    }
}

// ── 3×2 어파인 행렬 ([a,b,c,d,e,f]: x'=a·x+b·y+c, y'=d·x+e·y+f) ──
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
        .map(|b| f64::from_le_bytes(b.try_into().expect("8바이트 슬라이스")))
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

struct Style {
    m: Mat,
    border_color: u32,
    border_width: i32,
    border_style: u8,
    fill: u32,
    fill_gradient: Option<GradientSpec>,
}

/// SHAPE_COMPONENT 데이터에서 행렬·테두리·채움을 읽는다.
/// [CHID×2(top) 또는 ×1(멤버)] + 속성 + (translation 48 + (scale 48+rotation 48)×cnt)
/// + 테두리선(13B) + 채우기.
fn parse_style(d: &[u8]) -> Option<Style> {
    if d.len() < 8 {
        return None;
    }
    let base = if d[0..4] == d[4..8] { 8 } else { 4 };
    let cnt = rd_u16(d, base + 42)? as usize;
    let t = rd_mat(d, base + 44);
    let pair = base + 44 + 48 + cnt.saturating_sub(1) * 96;
    let m = if d.len() >= pair + 96 {
        t.mul(&rd_mat(d, pair).mul(&rd_mat(d, pair + 48)))
    } else {
        t
    };

    let bo = base + 92 + cnt * 96;
    let mut border_color = 0xFFFF_FFFFu32; // 없음
    let mut border_width = 0i32;
    let mut border_style = 0u8;
    let mut fill = 0xFFFF_FFFFu32; // 없음
    let mut fill_gradient = None;
    if let (Some(color), Some(width), Some(lattr)) =
        (rd_u32(d, bo), rd_i32(d, bo + 4), rd_u32(d, bo + 8))
    {
        let lt = (lattr & 0x3F) as u8;
        if lt != 0 {
            border_color = color;
            border_width = width.max(1);
            border_style = hwp5_line_style(lt);
        }
        let fo = bo + 13;
        if let Some(ft) = rd_u32(d, fo) {
            if ft & 0x1 != 0 {
                fill = rd_u32(d, fo + 4).unwrap_or(0xFFFF_FFFF);
            } else if ft & 0x4 != 0 {
                fill_gradient = parse_gradient(d, fo + 4);
            }
            // bit1(이미지 채움)은 v1 제외 — bin 참조 필요.
        }
    }
    Some(Style {
        m,
        border_color,
        border_width,
        border_style,
        fill,
        fill_gradient,
    })
}

/// hwp5 테두리선 종류(1실선/2긴점선/3점선/4점쇄선/5이점쇄선/6장대시…) →
/// ShapeGeom border_style(0=SOLID/1=DASH/2=DOT/3=DASH_DOT/4=DASH_DOT_DOT/5=LONG_DASH).
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

/// 그러데이션(Table 28): type(i16) 각(i16) cx cy spread num(i16),
/// num>2면 INT32[num] 위치, 이어서 COLORREF[num].
fn parse_gradient(d: &[u8], fo: usize) -> Option<GradientSpec> {
    let gtype = rd_u16(d, fo)? as i16;
    let angle = rd_u16(d, fo + 2)? as i16 as f32;
    let center_x = rd_u16(d, fo + 4)? as i16 as i32;
    let center_y = rd_u16(d, fo + 6)? as i16 as i32;
    let step = rd_u16(d, fo + 8)? as i16 as i32;
    let num = rd_u16(d, fo + 10)? as usize;
    if !(2..=16).contains(&num) {
        return None;
    }
    let mut off = fo + 12;
    let positions: Vec<f32> = if num > 2 {
        let mut v = Vec::with_capacity(num);
        for i in 0..num {
            v.push(rd_i32(d, off + i * 4)? as f32);
        }
        off += num * 4;
        let max = v.iter().cloned().fold(1.0_f32, f32::max);
        v.iter().map(|p| (p / max).clamp(0.0, 1.0)).collect()
    } else {
        (0..num).map(|i| i as f32 / (num - 1) as f32).collect()
    };
    let mut stops = Vec::with_capacity(num);
    for i in 0..num {
        let c = rd_u32(d, off + i * 4)?;
        stops.push((positions.get(i).copied().unwrap_or(0.0), c));
    }
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(GradientSpec {
        radial: gtype == 1,
        angle_deg: angle,
        center_x,
        center_y,
        step,
        stops,
    })
}

/// 기하 레코드의 local 점들에 행렬을 적용해 ShapeGeom을 만든다(HWPUNIT, 박스 원점 기준).
fn geometry(tag: u16, d: &[u8], s: &Style) -> Option<ShapeGeom> {
    let p =
        |o: usize| -> Option<(f64, f64)> { Some((rd_i32(d, o)? as f64, rd_i32(d, o + 4)? as f64)) };

    let (kind, raw_pts, round_ratio): (ShapeKind, Vec<(f64, f64)>, u8) = match tag {
        SC_LINE => {
            let a = p(0)?;
            let b = p(8)?;
            if (a.0 - b.0).abs() < f64::EPSILON && (a.1 - b.1).abs() < f64::EPSILON {
                return None;
            }
            (ShapeKind::Line, vec![a, b], 0)
        }
        SC_RECTANGLE => {
            // BYTE 곡률% + 모서리 4×(x,y).
            let curv = (*d.first()? as u32).min(100) as u8;
            (ShapeKind::Rect, vec![p(1)?, p(9)?, p(17)?, p(25)?], curv)
        }
        SC_POLYGON => {
            let n = rd_u16(d, 0)? as usize;
            if !(2..=4096).contains(&n) {
                return None;
            }
            let pts = (0..n).map(|i| p(4 + i * 8)).collect::<Option<Vec<_>>>()?;
            (ShapeKind::Polygon, pts, 0)
        }
        SC_ELLIPSE => {
            // UINT32 attr + center + ax1(끝점) + ax2(끝점) → bbox 근사.
            let c = p(4)?;
            let a1 = p(12)?;
            let a2 = p(20)?;
            let rx = ((a1.0 - c.0).powi(2) + (a1.1 - c.1).powi(2)).sqrt();
            let ry = ((a2.0 - c.0).powi(2) + (a2.1 - c.1).powi(2)).sqrt();
            (
                ShapeKind::Ellipse,
                vec![(c.0 - rx, c.1 - ry), (c.0 + rx, c.1 + ry)],
                0,
            )
        }
        SC_CURVE => {
            let n = rd_u16(d, 0)? as usize;
            if !(2..=4096).contains(&n) {
                return None;
            }
            // 세그먼트 타입 무시 — 폴리라인 근사(렌더와 동일 방침).
            let pts = (0..n).map(|i| p(2 + i * 8)).collect::<Option<Vec<_>>>()?;
            (ShapeKind::Curve, pts, 0)
        }
        SC_ARC => {
            // BYTE kind + center + ax1(끝점) + ax2(끝점) (정품 25B). 3점 보존(호 곡선).
            let c = p(1)?;
            let a1 = p(9)?;
            let a2 = p(17)?;
            (ShapeKind::Arc, vec![c, a1, a2], 0)
        }
        _ => return None,
    };

    // 행렬 적용 후 bbox.
    let mut tp: Vec<(f64, f64)> = raw_pts.iter().map(|&(x, y)| s.m.apply(x, y)).collect();
    // Arc: 행렬(회전+비균등 스케일)이 center/ax1/ax2 두 축을 비수직(전단)으로 만든다.
    // 한글 OWPML arc는 **수직 두 축**만 받으므로(비수직=pinwheel), 두 축을 그 각의 이등분선
    // 기준 ±45°(=90° 사이)로 등방화해 원형 1/4호로 근사한다(회전·방향 보존, ~미세 타원율 손실).
    if matches!(kind, ShapeKind::Arc) && tp.len() == 3 {
        let c = tp[0];
        let (v1, v2) = (
            (tp[1].0 - c.0, tp[1].1 - c.1),
            (tp[2].0 - c.0, tp[2].1 - c.1),
        );
        let r = (v1.0.hypot(v1.1) + v2.0.hypot(v2.1)) / 2.0;
        let (a1, a2) = (v1.1.atan2(v1.0), v2.1.atan2(v2.0));
        let mut d = a2 - a1; // v1→v2 sweep, 짧은 쪽으로 정규화
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        let (bis, q) = (a1 + d / 2.0, d.signum() * std::f64::consts::FRAC_PI_4);
        tp[1] = (c.0 + r * (bis - q).cos(), c.1 + r * (bis - q).sin());
        tp[2] = (c.0 + r * (bis + q).cos(), c.1 + r * (bis + q).sin());
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in &tp {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    let points: Vec<(i32, i32)> = match kind {
        // Arc는 center/ax1/ax2 3점을 bbox 기준으로 보존(writer가 그대로 방출).
        ShapeKind::Line | ShapeKind::Polygon | ShapeKind::Curve | ShapeKind::Arc => tp
            .iter()
            .map(|&(x, y)| ((x - minx).round() as i32, (y - miny).round() as i32))
            .collect(),
        _ => Vec::new(),
    };
    Some(ShapeGeom {
        kind,
        x: minx.round() as i32,
        y: miny.round() as i32,
        w: (maxx - minx).round() as i32,
        h: (maxy - miny).round() as i32,
        points,
        fill: s.fill,
        fill_gradient: s.fill_gradient.clone(),
        border_color: s.border_color,
        border_width: s.border_width,
        round_ratio,
        border_style: s.border_style,
        arrow_start: 0,
        arrow_end: 0,
        anchored: false, // 배치는 gso 40B 헤더가 결정(writer가 pos로 방출)
        description: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 코퍼스 실쌍(원본.hwp ↔ 한글 export hwpx)의 장식선 SHAPE_COMPONENT 252B.
    /// CHID "$lin"×2, cnt=1, scale 행렬 496.08/0.04, border 검정 width=32.
    const LINE_SC: &str = "6e696c246e696c240000000000000000000001006400000064000000c8c1000004000000000000000000e4600000020000000100000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000e17a14ae47017f400000000000000000000000000000000000000000000000007b14ae47e17aa43f0000000000000000000000000000f03f000000000000008000000000000000000000000000000000000000000000f03f00000000000000000000000020000000410000c000010000000000000000000000ffffffff00000000000000000000000000000000000000000001e76b390000";
    /// 그 자식 SC_LINE 20B: local (0,0) → (100,100).
    const LINE_GEOM: &str = "0000000000000000640000006400000000000000";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn 실쌍_장식선_변환() {
        // 태그를 리터럴로 박아 상수 오류를 잡는다(실측 JSON 덤프의 tag=0x4c/0x4e).
        let raw = vec![OpaqueRecord {
            tag: 0x4C,
            data: hex(LINE_SC),
            children: vec![OpaqueRecord {
                tag: 0x4E,
                data: hex(LINE_GEOM),
                children: Vec::new(),
            }],
        }];
        let shapes = shapes_from_raw(&raw);
        assert_eq!(shapes.len(), 1, "{shapes:?}");
        let s = &shapes[0];
        assert_eq!(s.kind, ShapeKind::Line);
        // 한글 export 정답: curSz 49608×4, lineShape width=32 color=#000000.
        assert_eq!((s.x, s.y), (0, 0));
        assert_eq!((s.w, s.h), (49608, 4));
        assert_eq!(s.points, vec![(0, 0), (49608, 4)]);
        assert_eq!(s.border_width, 32);
        assert_eq!(s.border_color, 0);
        assert_eq!(s.border_style, 0); // SOLID
    }

    #[test]
    fn 빈_또는_잘린_데이터_안전() {
        assert!(shapes_from_raw(&[]).is_empty());
        let raw = vec![OpaqueRecord {
            tag: SHAPE_COMPONENT,
            data: vec![0u8; 4],
            children: Vec::new(),
        }];
        assert!(shapes_from_raw(&raw).is_empty());
    }

    /// Builds a minimal table-28 gradient block: type, angle, center_x,
    /// center_y, spread, num=2, then 2 COLORREFs (no position array since
    /// num <= 2).
    fn table28_block(gtype: i16, angle: i16, cx: i16, cy: i16, spread: i16) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&gtype.to_le_bytes());
        d.extend_from_slice(&angle.to_le_bytes());
        d.extend_from_slice(&cx.to_le_bytes());
        d.extend_from_slice(&cy.to_le_bytes());
        d.extend_from_slice(&spread.to_le_bytes());
        d.extend_from_slice(&2i16.to_le_bytes()); // num
        d.extend_from_slice(&0xFF0000u32.to_le_bytes());
        d.extend_from_slice(&0x0000FFu32.to_le_bytes());
        d
    }

    #[test]
    fn gradient_center_and_step_from_table28() {
        let d = table28_block(0, 45, 30, 70, 120);
        let gr = parse_gradient(&d, 0).unwrap();
        assert_eq!(gr.center_x, 30);
        assert_eq!(gr.center_y, 70);
        assert_eq!(gr.step, 120);
    }
}
