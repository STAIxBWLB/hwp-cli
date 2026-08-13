//! DisplayList — 레이아웃과 백엔드 사이의 안정 계약.
//!
//! HWP 도메인 지식이 제거된 순수 그리기 명령. 좌표는 pt(f32),
//! 페이지 원점 좌상단, y축 아래 방향.

use std::sync::Arc;

use crate::shape::ShapedRun;

pub struct DisplayList {
    pub pages: Vec<PageList>,
}

pub struct PageList {
    pub width_pt: f32,
    pub height_pt: f32,
    pub items: Vec<Item>,
}

pub enum Item {
    /// 베이스라인 원점 (x, y)에 배치된 글리프 런
    Glyphs { x: f32, y: f32, run: ShapedRun },
    /// 채움 사각형 (셀 배경 등) — COLORREF
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: u32,
    },
    /// 이미지 — 인코딩된 원본 바이트 (PNG/JPEG/BMP/GIF)
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        data: Arc<Vec<u8>>,
        /// 자르기 (좌,상,우,하) — 원본 이미지 HWPUNIT(96dpi: px×75) 좌표.
        /// None=전체 표시. 자른 영역이 (x,y,w,h)에 늘려져 들어간다.
        crop: Option<[f32; 4]>,
        /// 뒤집기 (0=없음, 1=가로, 2=세로, 3=양쪽). 영역 중심 기준.
        flip: u8,
        /// 회전(도, 시계 방향). 영역 중심 기준.
        rotation_deg: f32,
        /// 밝기/명암 보정 (-100..100, 0=원본). 둘 다 0이면 백엔드의
        /// 무손실 빠른 경로(PDF DCTDecode, SVG 원본 임베드)를 유지한다.
        brightness: i8,
        contrast: i8,
    },
    /// Arbitrary page-space path for shapes, borders, and line decorations.
    Path {
        commands: Vec<PathCmd>,
        /// 채움 (단색/그러데이션). None=채움 없음. (이미지 채움은 별도 Item::Image로 emit.)
        fill: Option<Fill>,
        /// 선 스타일(색·굵기·점선). None=선 없음.
        stroke: Option<Stroke>,
    },
}

/// 선 스타일 — 색, 굵기(pt), 점선 패턴.
#[derive(Debug, Clone)]
pub struct Stroke {
    /// 선색 COLORREF(0x00BBGGRR).
    pub color: u32,
    /// 굵기 pt.
    pub width: f32,
    /// 점선 패턴(on, off, …) pt. 빈 벡터=실선.
    pub dash: Vec<f32>,
}

impl Stroke {
    /// 실선.
    pub fn solid(color: u32, width: f32) -> Self {
        Self {
            color,
            width,
            dash: Vec::new(),
        }
    }
}

/// 경로 명령 (좌표 pt, 페이지 공간).
#[derive(Debug, Clone, Copy)]
pub enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// 경로 채움.
#[derive(Debug, Clone)]
pub enum Fill {
    /// 단색 COLORREF(0x00BBGGRR).
    Solid(u32),
    Gradient(Gradient),
    /// 무늬(해치) 채움 — fg=무늬색, bg=배경색(0xFFFFFFFF=투명), style=표 29 (1-6).
    Hatch {
        fg: u32,
        bg: u32,
        style: u32,
    },
}

/// 해치 무늬 선 간격(pt). 한글 정확한 디더 패턴은 비공개 — 시각적 무게를 맞춘 근사.
pub const HATCH_SPACING: f32 = 3.0;
/// 해치 무늬 선 굵기(pt).
pub const HATCH_LINE_WIDTH: f32 = 0.5;

/// 해치 무늬 선분열 — ((x,y),(x,y)) 쌍.
type Segments = Vec<((f32, f32), (f32, f32))>;

/// 해치 무늬 선분 열거 — bbox(page pt) 안으로 클립된 ((x,y),(x,y)) 쌍.
/// style: 표 29 (1=가로, 2=세로, 3=\, 4=/, 5=십자, 6=대각십자).
pub fn hatch_segments(style: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> Segments {
    let mut out = Vec::new();
    if x1 <= x0 || y1 <= y0 {
        return out;
    }
    let s = HATCH_SPACING;
    let horiz = |out: &mut Segments| {
        let mut y = y0;
        while y <= y1 {
            out.push(((x0, y), (x1, y)));
            y += s;
        }
    };
    let vert = |out: &mut Segments| {
        let mut x = x0;
        while x <= x1 {
            out.push(((x, y0), (x, y1)));
            x += s;
        }
    };
    // 대각선은 수직 간격이 s가 되도록 축 방향 step = s·√2.
    let diag = s * std::f32::consts::SQRT_2;
    let backslash = |out: &mut Segments| {
        // y = x + c
        let mut c = (y0 - x1) - (y0 - x1).rem_euclid(diag);
        while c <= y1 - x0 {
            let lo = x0.max(y0 - c);
            let hi = x1.min(y1 - c);
            if lo < hi {
                out.push(((lo, lo + c), (hi, hi + c)));
            }
            c += diag;
        }
    };
    let slash = |out: &mut Segments| {
        // y = -x + c
        let mut c = (y0 + x0) - (y0 + x0).rem_euclid(diag);
        while c <= y1 + x1 {
            let lo = x0.max(c - y1);
            let hi = x1.min(c - y0);
            if lo < hi {
                out.push(((lo, c - lo), (hi, c - hi)));
            }
            c += diag;
        }
    };
    match style {
        1 => horiz(&mut out),
        2 => vert(&mut out),
        3 => backslash(&mut out),
        4 => slash(&mut out),
        5 => {
            horiz(&mut out);
            vert(&mut out);
        }
        6 => {
            backslash(&mut out);
            slash(&mut out);
        }
        _ => {}
    }
    out
}

/// 그러데이션 채움. 좌표는 도형 경계 상자 기준으로 백엔드가 배치한다.
#[derive(Debug, Clone)]
pub struct Gradient {
    /// true=방사형(radial), false=선형(linear).
    pub radial: bool,
    /// 선형 방향(도). 0=가로(왼→오), 90=세로.
    pub angle_deg: f32,
    /// (위치 0..1, COLORREF). 위치 오름차순.
    pub stops: Vec<(f32, u32)>,
}

impl Gradient {
    /// 위치 t(0..1)의 보간색 (r,g,b).
    pub fn color_at(&self, t: f32) -> (u8, u8, u8) {
        if self.stops.is_empty() {
            return (0, 0, 0);
        }
        let t = t.clamp(0.0, 1.0);
        if t <= self.stops[0].0 {
            return colorref_rgb(self.stops[0].1);
        }
        if t >= self.stops[self.stops.len() - 1].0 {
            return colorref_rgb(self.stops[self.stops.len() - 1].1);
        }
        for w in self.stops.windows(2) {
            let (p0, c0) = w[0];
            let (p1, c1) = w[1];
            if t >= p0 && t <= p1 {
                let f = if (p1 - p0).abs() < f32::EPSILON {
                    0.0
                } else {
                    (t - p0) / (p1 - p0)
                };
                let (r0, g0, b0) = colorref_rgb(c0);
                let (r1, g1, b1) = colorref_rgb(c1);
                return (lerp_u8(r0, r1, f), lerp_u8(g0, g1, f), lerp_u8(b0, b1, f));
            }
        }
        colorref_rgb(self.stops[self.stops.len() - 1].1)
    }
}

fn lerp_u8(a: u8, b: u8, f: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * f)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// COLORREF(0x00BBGGRR) → (r, g, b). 0xFFFFFFFF은 흰색 취급(그러데이션 stop용).
fn colorref_rgb(c: u32) -> (u8, u8, u8) {
    (
        (c & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        ((c >> 16) & 0xFF) as u8,
    )
}

// ── 이미지 변환 공통 수학 (GG-15) ─────────────────────────────────────
//
// 아핀 행렬은 PDF/SVG matrix() 순서 [a b c d e f]: x' = a·x + c·y + e,
// y' = b·x + d·y + f (y-아래 좌표계). tiny-skia는 from_row(a, c, b, d, e, f)로
// 변환해 쓴다.

/// 아핀 합성 m1·m2 (m2를 먼저 적용).
pub fn mat_mul(m1: [f32; 6], m2: [f32; 6]) -> [f32; 6] {
    let [a1, b1, c1, d1, e1, f1] = m1;
    let [a2, b2, c2, d2, e2, f2] = m2;
    [
        a1 * a2 + c1 * b2,
        b1 * a2 + d1 * b2,
        a1 * c2 + c1 * d2,
        b1 * c2 + d1 * d2,
        a1 * e2 + c1 * f2 + e1,
        b1 * e2 + d1 * f2 + f1,
    ]
}

/// 뒤집기+회전 행렬 — (cx, cy) 중심 기준. flip: 1=가로, 2=세로.
/// rotation_deg는 y-아래 좌표계 시계 방향 양수(HWP 각도 규약).
pub fn flip_rotate_matrix(cx: f32, cy: f32, flip: u8, rotation_deg: f32) -> [f32; 6] {
    let t = rotation_deg.to_radians();
    let (sn, cs) = t.sin_cos();
    let (fx, fy) = (
        if flip & 1 != 0 { -1.0 } else { 1.0 },
        if flip & 2 != 0 { -1.0 } else { 1.0 },
    );
    // T(c) · R(θ) · F · T(-c)
    let rf = [cs * fx, sn * fx, -sn * fy, cs * fy, 0.0, 0.0];
    mat_mul(
        [1.0, 0.0, 0.0, 1.0, cx, cy],
        mat_mul(rf, [1.0, 0.0, 0.0, 1.0, -cx, -cy]),
    )
}

/// 자르기 사각형(HWPUNIT)을 원본 픽셀 크기 기준 비율 (l,t,r,b, 0..1)로 환산.
/// HWPUNIT은 96dpi 규약(px×75). 벗어난 값은 클램프, 퇴화(빈 영역)면 None.
pub fn crop_fractions(crop: [f32; 4], px_w: u32, px_h: u32) -> Option<[f32; 4]> {
    let (nw, nh) = (px_w as f32 * 75.0, px_h as f32 * 75.0);
    if nw <= 0.0 || nh <= 0.0 {
        return None;
    }
    let l = (crop[0] / nw).clamp(0.0, 1.0);
    let t = (crop[1] / nh).clamp(0.0, 1.0);
    let r = (crop[2] / nw).clamp(0.0, 1.0);
    let b = (crop[3] / nh).clamp(0.0, 1.0);
    (r - l > 1e-4 && b - t > 1e-4).then_some([l, t, r, b])
}

/// 밝기/명암 픽셀 맵 (-100..100). 밝기는 가산, 명암은 128 중심 스케일.
/// (한글 날부 곡선은 비공개 — 선형 근사, 한글 대조 후속.)
pub fn apply_brightness_contrast(v: u8, brightness: i8, contrast: i8) -> u8 {
    let b = f32::from(brightness) / 100.0 * 255.0;
    let c = 1.0 + f32::from(contrast) / 100.0;
    ((f32::from(v) + b - 128.0) * c + 128.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// 경로의 경계 상자 (minx, miny, maxx, maxy). pt.
pub fn path_bbox(cmds: &[PathCmd]) -> (f32, f32, f32, f32) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut acc = |x: f32, y: f32| {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    };
    for c in cmds {
        match *c {
            PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => acc(x, y),
            PathCmd::CubicTo(a, b, c2, d, e, f) => {
                acc(a, b);
                acc(c2, d);
                acc(e, f);
            }
            PathCmd::Close => {}
        }
    }
    if x0 > x1 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        (x0, y0, x1, y1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 행렬 합성: 항등 × 임의 = 임의, 평행이동 합성 순서.
    #[test]
    fn 아핀_합성() {
        let id = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let t = [2.0, 0.0, 0.0, 3.0, 10.0, 20.0];
        assert_eq!(mat_mul(id, t), t);
        assert_eq!(mat_mul(t, id), t);
        // T(10,20)·T(1,2) = T(11,22)
        let m = mat_mul(
            [1.0, 0.0, 0.0, 1.0, 10.0, 20.0],
            [1.0, 0.0, 0.0, 1.0, 1.0, 2.0],
        );
        assert_eq!(m[4], 11.0);
        assert_eq!(m[5], 22.0);
    }

    /// 뒤집기/회전: 중심 기준. 가로 뒤집기는 좌우를, 90° 회전은 오른쪽을 아래로.
    #[test]
    fn 뒤집기_회전_행렬() {
        // 가로 뒤집기: (0,0) 중심, (10, 4) → (-10, 4)
        let m = flip_rotate_matrix(0.0, 0.0, 1, 0.0);
        let (x, y) = (
            m[0] * 10.0 + m[2] * 4.0 + m[4],
            m[1] * 10.0 + m[3] * 4.0 + m[5],
        );
        assert!((x + 10.0).abs() < 1e-4 && (y - 4.0).abs() < 1e-4, "{x},{y}");
        // 90° (시계): (10, 0) → (0, 10)
        let m = flip_rotate_matrix(0.0, 0.0, 0, 90.0);
        let (x, y) = (m[0] * 10.0 + m[4], m[1] * 10.0 + m[5]);
        assert!(x.abs() < 1e-4 && (y - 10.0).abs() < 1e-4, "{x},{y}");
        // 중심 (5,5): 중심은 불변
        let m = flip_rotate_matrix(5.0, 5.0, 3, 45.0);
        let (x, y) = (
            m[0] * 5.0 + m[2] * 5.0 + m[4],
            m[1] * 5.0 + m[3] * 5.0 + m[5],
        );
        assert!((x - 5.0).abs() < 1e-3 && (y - 5.0).abs() < 1e-3, "{x},{y}");
    }

    /// 자르기 HWPUNIT→비율: 96dpi 규약(px×75). 퇴화 영역은 None.
    #[test]
    fn 자르기_비율_환산() {
        // 4×2 px → 자연 300×150 HWPUNIT.
        assert_eq!(
            crop_fractions([0.0, 0.0, 150.0, 150.0], 4, 2),
            Some([0.0, 0.0, 0.5, 1.0])
        );
        assert_eq!(crop_fractions([100.0, 0.0, 100.0, 150.0], 4, 2), None);
    }

    /// 밝기/명암 맵: 0이면 항등, 밝기+는 단조 증가, 명암-는 128로 수렴.
    #[test]
    fn 밝기_명암_맵() {
        for v in [0u8, 64, 128, 200, 255] {
            assert_eq!(apply_brightness_contrast(v, 0, 0), v);
        }
        assert!(apply_brightness_contrast(100, 50, 0) > 100);
        assert!(apply_brightness_contrast(200, -50, 0) < 200);
        assert_eq!(apply_brightness_contrast(0, 0, -100), 128);
        assert_eq!(apply_brightness_contrast(255, 0, -100), 128);
    }

    /// 해치 선분: 가로 무늬는 간격마다 1개, bbox 밖으로 나가지 않는다.
    #[test]
    fn 해치_선분_클립() {
        let segs = hatch_segments(1, 0.0, 0.0, 10.0, 7.0);
        assert_eq!(segs.len(), 3); // y = 0, 3, 6
        assert!(
            segs.iter()
                .all(|(a, b)| a.0 == 0.0 && b.0 == 10.0 && a.1 == b.1)
        );
        // 대각선(4=/): 모든 점이 bbox 안.
        let segs = hatch_segments(4, 0.0, 0.0, 10.0, 10.0);
        assert!(!segs.is_empty());
        assert!(segs.iter().all(|(a, b)| {
            [a, b]
                .iter()
                .all(|(x, y)| (0.0..=10.0).contains(x) && (0.0..=10.0).contains(y))
        }));
        // 십자(5) = 가로+세로.
        assert_eq!(
            hatch_segments(5, 0.0, 0.0, 6.0, 6.0).len(),
            hatch_segments(1, 0.0, 0.0, 6.0, 6.0).len()
                + hatch_segments(2, 0.0, 0.0, 6.0, 6.0).len()
        );
        assert!(hatch_segments(0, 0.0, 0.0, 10.0, 10.0).is_empty());
    }
}
