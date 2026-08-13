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
        /// Crop rectangle (left, top, right, bottom) in source-image HWPUNIT
        /// coordinates (96 dpi: pixels x 75). `None` displays the full image.
        /// The cropped region is scaled to fill `(x, y, w, h)`.
        crop: Option<[f32; 4]>,
        /// Flip mode around the region center: 0=none, 1=horizontal,
        /// 2=vertical, 3=both.
        flip: u8,
        /// Clockwise rotation in degrees around the region center.
        rotation_deg: f32,
        /// Brightness/contrast adjustment (-100..100, 0=unchanged). When
        /// both are zero, backends retain their lossless fast paths (PDF
        /// DCTDecode and direct SVG embedding).
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
    /// Hatch fill: `fg` is the pattern color, `bg` is the background color
    /// (`0xFFFFFFFF` means transparent), and `style` is table 29 (1-6).
    Hatch {
        fg: u32,
        bg: u32,
        style: u32,
    },
}

/// Hatch line spacing in points. Hangul's exact dither pattern is private;
/// this value approximates its visual weight.
pub const HATCH_SPACING: f32 = 3.0;
/// Hatch line width in points.
pub const HATCH_LINE_WIDTH: f32 = 0.5;
/// Prevents damaged or adversarial geometry from expanding into an unbounded
/// number of display-list segments.
const MAX_HATCH_SEGMENTS: usize = 16_384;

/// Hatch line segments represented as endpoint pairs.
type Segments = Vec<((f32, f32), (f32, f32))>;

/// Enumerates hatch line segments clipped to a page-space bounding box.
/// Table 29 styles: 1=horizontal, 2=vertical, 3=backslash, 4=slash,
/// 5=cross, 6=diagonal cross.
pub fn hatch_segments(style: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> Segments {
    let mut out = Vec::new();
    if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) || x1 <= x0 || y1 <= y0 {
        return out;
    }
    let s = HATCH_SPACING;
    let horiz = |out: &mut Segments| {
        let mut y = y0;
        let mut attempts = 0;
        while y <= y1 && out.len() < MAX_HATCH_SEGMENTS && attempts < MAX_HATCH_SEGMENTS {
            attempts += 1;
            out.push(((x0, y), (x1, y)));
            y += s;
        }
    };
    let vert = |out: &mut Segments| {
        let mut x = x0;
        let mut attempts = 0;
        while x <= x1 && out.len() < MAX_HATCH_SEGMENTS && attempts < MAX_HATCH_SEGMENTS {
            attempts += 1;
            out.push(((x, y0), (x, y1)));
            x += s;
        }
    };
    // Make the axis step s * sqrt(2) so diagonal lines have vertical spacing s.
    let diag = s * std::f32::consts::SQRT_2;
    let backslash = |out: &mut Segments| {
        // y = x + c
        let mut c = (y0 - x1) - (y0 - x1).rem_euclid(diag);
        let mut attempts = 0;
        while c <= y1 - x0 && out.len() < MAX_HATCH_SEGMENTS && attempts < MAX_HATCH_SEGMENTS {
            attempts += 1;
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
        let mut attempts = 0;
        while c <= y1 + x1 && out.len() < MAX_HATCH_SEGMENTS && attempts < MAX_HATCH_SEGMENTS {
            attempts += 1;
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

/// Gradient fill positioned by each backend relative to the shape bounds.
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

/// Converts COLORREF (`0x00BBGGRR`) to RGB. Gradients treat `0xFFFFFFFF` as white.
fn colorref_rgb(c: u32) -> (u8, u8, u8) {
    (
        (c & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        ((c >> 16) & 0xFF) as u8,
    )
}

// Image-transform math shared by all backends (GG-15).
//
// Affine matrices use PDF/SVG order [a b c d e f]: x' = a*x + c*y + e,
// y' = b*x + d*y + f in a y-down coordinate system. tiny-skia consumes the
// same transform through from_row(a, c, b, d, e, f).

/// Composes `m1 * m2`, applying `m2` first.
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

/// Builds a flip-and-rotation matrix around `(cx, cy)`.
/// Flip bits are 1=horizontal and 2=vertical. Positive rotation is clockwise
/// in the HWP y-down coordinate system.
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

/// Converts an HWPUNIT crop rectangle to source-image fractions in `0..=1`.
/// HWPUNIT uses the 96 dpi convention (pixels x 75). Out-of-range values are
/// clamped and empty rectangles return `None`.
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

/// Applies brightness and contrast in the documented -100..=100 range.
/// Brightness is additive and contrast scales around 128. Hangul's exact
/// transfer curve is private, so this is a linear approximation.
pub fn apply_brightness_contrast(v: u8, brightness: i8, contrast: i8) -> u8 {
    let brightness = brightness.clamp(-100, 100);
    let contrast = contrast.clamp(-100, 100);
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

    /// Matrix composition preserves identity and translation order.
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

    /// Flip and rotation operate around the requested center.
    #[test]
    fn 뒤집기_회전_행렬() {
        // Horizontal flip around (0, 0): (10, 4) becomes (-10, 4).
        let m = flip_rotate_matrix(0.0, 0.0, 1, 0.0);
        let (x, y) = (
            m[0] * 10.0 + m[2] * 4.0 + m[4],
            m[1] * 10.0 + m[3] * 4.0 + m[5],
        );
        assert!((x + 10.0).abs() < 1e-4 && (y - 4.0).abs() < 1e-4, "{x},{y}");
        // A clockwise 90-degree rotation maps (10, 0) to (0, 10).
        let m = flip_rotate_matrix(0.0, 0.0, 0, 90.0);
        let (x, y) = (m[0] * 10.0 + m[4], m[1] * 10.0 + m[5]);
        assert!(x.abs() < 1e-4 && (y - 10.0).abs() < 1e-4, "{x},{y}");
        // The center (5, 5) remains fixed.
        let m = flip_rotate_matrix(5.0, 5.0, 3, 45.0);
        let (x, y) = (
            m[0] * 5.0 + m[2] * 5.0 + m[4],
            m[1] * 5.0 + m[3] * 5.0 + m[5],
        );
        assert!((x - 5.0).abs() < 1e-3 && (y - 5.0).abs() < 1e-3, "{x},{y}");
    }

    /// Converts HWPUNIT crops to fractions and rejects empty regions.
    #[test]
    fn 자르기_비율_환산() {
        // A 4x2 image has natural dimensions of 300x150 HWPUNIT at 96 dpi.
        assert_eq!(
            crop_fractions([0.0, 0.0, 150.0, 150.0], 4, 2),
            Some([0.0, 0.0, 0.5, 1.0])
        );
        assert_eq!(crop_fractions([100.0, 0.0, 100.0, 150.0], 4, 2), None);
    }

    /// Zero adjustment is identity; brightness is monotonic; -100 contrast converges on 128.
    #[test]
    fn 밝기_명암_맵() {
        for v in [0u8, 64, 128, 200, 255] {
            assert_eq!(apply_brightness_contrast(v, 0, 0), v);
        }
        assert!(apply_brightness_contrast(100, 50, 0) > 100);
        assert!(apply_brightness_contrast(200, -50, 0) < 200);
        assert_eq!(apply_brightness_contrast(0, 0, -100), 128);
        assert_eq!(apply_brightness_contrast(255, 0, -100), 128);
        assert_eq!(
            apply_brightness_contrast(32, i8::MIN, i8::MAX),
            apply_brightness_contrast(32, -100, 100)
        );
    }

    /// Hatch segments remain inside the bounding box at the configured spacing.
    #[test]
    fn 해치_선분_클립() {
        let segs = hatch_segments(1, 0.0, 0.0, 10.0, 7.0);
        assert_eq!(segs.len(), 3); // y = 0, 3, 6
        assert!(
            segs.iter()
                .all(|(a, b)| a.0 == 0.0 && b.0 == 10.0 && a.1 == b.1)
        );
        // Every endpoint of the slash pattern stays inside the bounds.
        let segs = hatch_segments(4, 0.0, 0.0, 10.0, 10.0);
        assert!(!segs.is_empty());
        assert!(segs.iter().all(|(a, b)| {
            [a, b]
                .iter()
                .all(|(x, y)| (0.0..=10.0).contains(x) && (0.0..=10.0).contains(y))
        }));
        // Cross hatch is the union of horizontal and vertical hatch.
        assert_eq!(
            hatch_segments(5, 0.0, 0.0, 6.0, 6.0).len(),
            hatch_segments(1, 0.0, 0.0, 6.0, 6.0).len()
                + hatch_segments(2, 0.0, 0.0, 6.0, 6.0).len()
        );
        assert!(hatch_segments(0, 0.0, 0.0, 10.0, 10.0).is_empty());
        assert!(hatch_segments(1, 0.0, 0.0, f32::INFINITY, 10.0).is_empty());
        assert!(hatch_segments(1, f32::NAN, 0.0, 10.0, 10.0).is_empty());
        assert_eq!(
            hatch_segments(1, 0.0, 0.0, 10.0, 1.0e30).len(),
            MAX_HATCH_SEGMENTS
        );
        assert!(hatch_segments(6, 0.0, 0.0, 1.0e30, 1.0e30).len() <= MAX_HATCH_SEGMENTS);
    }
}
