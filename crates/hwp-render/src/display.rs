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
    },
    /// 임의 경로 — 그리기 개체(선/사각형/타원/호/다각형)·테두리·선분(밑줄 등). 좌표 pt, 페이지 공간.
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
