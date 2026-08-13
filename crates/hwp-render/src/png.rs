//! PNG 백엔드 — tiny-skia 래스터화.
//!
//! 글리프 윤곽선을 ttf-parser(rustybuzz 재수출)로 추출해 tiny-skia
//! Path로 채운다. 합성 굵게 = fill+stroke, 합성 기울임 = skew 변환.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use image::{ImageFormat, ImageReader, Limits};
use rustybuzz::ttf_parser;
use sha2::{Digest as _, Sha256};
use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point,
    RadialGradient, Shader, SpreadMode, Stroke, Transform,
};

use crate::display::{DisplayList, Fill, Gradient, Item, PageList, PathCmd, path_bbox};
use crate::error::RenderError;
use crate::issues::{RenderIssueAccumulator, RenderIssueCode};

/// 기울임 시뮬레이션 각도의 탄젠트 (≈12°).
const ITALIC_SKEW: f32 = 0.2126;
const MAX_RASTER_DIMENSION: u32 = 16_384;
const MAX_RASTER_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_RASTER_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_SOURCE_IMAGE_DIMENSION: u32 = 8_192;
const MAX_SOURCE_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_SOURCE_IMAGE_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_UNIQUE_IMAGE_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_UNIQUE_IMAGE_DECODED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_IMAGE_REFERENCES: u64 = 100_000;

pub fn render_png(list: &DisplayList, dpi: f32) -> Result<Vec<Pixmap>, RenderError> {
    let mut issues = RenderIssueAccumulator::new();
    render_png_pages_with_issues(list, dpi, None, &mut issues)
}

/// 선택한 1-기반 페이지 번호만 래스터화한다. `None`이면 전체 페이지.
///
/// 레이아웃은 전체 문서 구조를 확인하지만 Pixmap은 선택된 페이지만 만들며, 선택 집합의
/// 총 픽셀 수도 제한해 긴 문서가 페이지별 상한을 우회해 메모리를 소진하지 못하게 한다.
pub fn render_png_pages(
    list: &DisplayList,
    dpi: f32,
    pages: Option<&[usize]>,
) -> Result<Vec<Pixmap>, RenderError> {
    let mut issues = RenderIssueAccumulator::new();
    render_png_pages_with_issues(list, dpi, pages, &mut issues)
}

pub fn render_png_pages_with_issues(
    list: &DisplayList,
    dpi: f32,
    pages: Option<&[usize]>,
    issues: &mut RenderIssueAccumulator,
) -> Result<Vec<Pixmap>, RenderError> {
    crate::validate_dpi(dpi)?;
    let selected = match pages {
        Some(pages) => pages.to_vec(),
        None => (1..=list.pages.len()).collect(),
    };
    let mut total_pixels = 0_u64;
    let mut dimensions = Vec::with_capacity(selected.len());
    for &page_number in &selected {
        let page = list.pages.get(page_number.wrapping_sub(1)).ok_or_else(|| {
            RenderError::Backend(format!(
                "페이지 범위 오류: 문서 {}쪽, 요청 {page_number}",
                list.pages.len()
            ))
        })?;
        let (width, height) = raster_dimensions(page, dpi)?;
        total_pixels = total_pixels
            .checked_add(u64::from(width) * u64::from(height))
            .ok_or_else(|| RenderError::Backend("전체 픽셀 수 계산이 넘쳤습니다".to_string()))?;
        if total_pixels > MAX_TOTAL_RASTER_PIXELS {
            return Err(RenderError::Backend(format!(
                "선택 페이지 래스터가 총 픽셀 상한 {MAX_TOTAL_RASTER_PIXELS}개를 초과합니다: {total_pixels}"
            )));
        }
        dimensions.push((page, width, height));
    }
    let mut images = ImageDecodeContext::default();
    let mut rendered = Vec::with_capacity(dimensions.len());
    for (page, width, height) in dimensions {
        rendered.push(render_page(page, dpi, width, height, issues, &mut images)?);
    }
    Ok(rendered)
}

fn raster_dimensions(page: &PageList, dpi: f32) -> Result<(u32, u32), RenderError> {
    crate::validate_dpi(dpi)?;
    let px_scale = dpi / 72.0;
    let dimension = |points: f32, label: &str| -> Result<u32, RenderError> {
        let pixels = f64::from(points) * f64::from(px_scale);
        if !points.is_finite() || points <= 0.0 || !pixels.is_finite() || pixels <= 0.0 {
            return Err(RenderError::Backend(format!(
                "페이지 {label}가 유효한 양의 유한값이 아닙니다: {points}pt"
            )));
        }
        let pixels = pixels.ceil();
        if pixels > f64::from(MAX_RASTER_DIMENSION) {
            return Err(RenderError::Backend(format!(
                "페이지 {label}가 래스터 상한 {MAX_RASTER_DIMENSION}px을 초과합니다: {pixels}px"
            )));
        }
        Ok(pixels as u32)
    };
    let w = dimension(page.width_pt, "너비")?;
    let h = dimension(page.height_pt, "높이")?;
    let pixels = u64::from(w)
        .checked_mul(u64::from(h))
        .ok_or_else(|| RenderError::Backend("페이지 픽셀 수 계산이 넘쳤습니다".to_string()))?;
    if pixels > MAX_RASTER_PIXELS {
        return Err(RenderError::Backend(format!(
            "페이지 래스터가 픽셀 상한 {MAX_RASTER_PIXELS}개를 초과합니다: {w}x{h}={pixels}"
        )));
    }
    Ok((w, h))
}

fn render_page(
    page: &PageList,
    dpi: f32,
    w: u32,
    h: u32,
    issues: &mut RenderIssueAccumulator,
    images: &mut ImageDecodeContext,
) -> Result<Pixmap, RenderError> {
    let px_scale = dpi / 72.0;
    let mut pixmap =
        Pixmap::new(w, h).ok_or_else(|| RenderError::Backend("Pixmap 생성 실패".to_string()))?;
    pixmap.fill(Color::WHITE);

    for item in &page.items {
        match item {
            Item::Rect {
                x,
                y,
                w: rw,
                h: rh,
                fill,
            } => {
                if let Some(rect) = tiny_skia::Rect::from_xywh(
                    *x * px_scale,
                    *y * px_scale,
                    rw * px_scale,
                    rh * px_scale,
                ) {
                    let mut paint = Paint::default();
                    let (r, g, b) = colorref_rgb(*fill);
                    paint.set_color_rgba8(r, g, b, 255);
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
            Item::Image {
                x,
                y,
                w: iw,
                h: ih,
                data,
            } => {
                match images.decode(data, issues)? {
                    Some(src) => {
                        let sx = (iw * px_scale) / src.width() as f32;
                        let sy = (ih * px_scale) / src.height() as f32;
                        let t = Transform::from_scale(sx, sy)
                            .post_translate(*x * px_scale, *y * px_scale);
                        pixmap.draw_pixmap(
                            0,
                            0,
                            src.as_ref().as_ref(),
                            &tiny_skia::PixmapPaint::default(),
                            t,
                            None,
                        );
                    }
                    None => {
                        issues.push(RenderIssueCode::ImageDecodePlaceholder, b"png");
                        // 디코드 실패: 자홍색 placeholder (조용한 누락 금지)
                        if let Some(rect) = tiny_skia::Rect::from_xywh(
                            *x * px_scale,
                            *y * px_scale,
                            iw * px_scale,
                            ih * px_scale,
                        ) {
                            let mut paint = Paint::default();
                            paint.set_color_rgba8(255, 0, 255, 120);
                            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                        }
                    }
                }
            }
            Item::Glyphs { x, y, run } => {
                let face = match ttf_parser::Face::parse(&run.font.data, run.font.index) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                // 글자 음영(배경 하이라이트) — 글리프 뒤에 사각형.
                if run.shade_color != 0xFFFF_FFFF
                    && let Some(rect) = tiny_skia::Rect::from_xywh(
                        *x * px_scale,
                        (*y - run.size_pt * 0.8) * px_scale,
                        run.width_pt * px_scale,
                        run.size_pt * px_scale,
                    )
                {
                    let (sr, sg, sb) = colorref_rgb(run.shade_color);
                    let mut sp = Paint::default();
                    sp.set_color_rgba8(sr, sg, sb, 255);
                    pixmap.fill_rect(rect, &sp, Transform::identity(), None);
                }
                // 그림자 — 본문 전에 오프셋 복사.
                if let Some(sc) = run.shadow {
                    let (dx, dy) = run.shadow_offset();
                    draw_glyph_run(&mut pixmap, &face, run, *x, *y, px_scale, sc, dx, dy);
                }
                // 양각/음각 — 흰 하이라이트 사본 오프셋(양각=좌상, 음각=우하).
                if run.emboss || run.engrave {
                    let d = run.size_pt * 0.05 * if run.emboss { -1.0 } else { 1.0 };
                    draw_glyph_run(&mut pixmap, &face, run, *x, *y, px_scale, 0x00FF_FFFF, d, d);
                }
                draw_glyph_run(
                    &mut pixmap,
                    &face,
                    run,
                    *x,
                    *y,
                    px_scale,
                    run.color,
                    0.0,
                    0.0,
                );
            }
            Item::Path {
                commands,
                fill,
                stroke,
            } => {
                let mut pb = PathBuilder::new();
                for cmd in commands {
                    match *cmd {
                        PathCmd::MoveTo(x, y) => pb.move_to(x, y),
                        PathCmd::LineTo(x, y) => pb.line_to(x, y),
                        PathCmd::CubicTo(a, b, c, d, e, f) => pb.cubic_to(a, b, c, d, e, f),
                        PathCmd::Close => pb.close(),
                    }
                }
                if let Some(path) = pb.finish() {
                    let t = Transform::from_scale(px_scale, px_scale);
                    if let Some(f) = fill {
                        let mut paint = Paint {
                            anti_alias: true,
                            ..Default::default()
                        };
                        match f {
                            Fill::Solid(c) => {
                                let (r, g, b) = colorref_rgb(*c);
                                paint.set_color_rgba8(r, g, b, 255);
                            }
                            Fill::Gradient(grad) => match gradient_shader(grad, commands, px_scale)
                            {
                                Some(sh) => paint.shader = sh,
                                None => {
                                    let (r, g, b) = grad
                                        .stops
                                        .first()
                                        .map_or((0, 0, 0), |&(_, c)| colorref_rgb(c));
                                    paint.set_color_rgba8(r, g, b, 255);
                                }
                            },
                        }
                        pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
                    }
                    if let Some(s) = stroke {
                        let (r, g, b) = colorref_rgb(s.color);
                        let mut paint = Paint::default();
                        paint.set_color_rgba8(r, g, b, 255);
                        paint.anti_alias = true;
                        let dash = (s.dash.len() >= 2)
                            .then(|| tiny_skia::StrokeDash::new(s.dash.clone(), 0.0))
                            .flatten();
                        let stroke = Stroke {
                            width: s.width.max(0.05),
                            dash,
                            ..Stroke::default()
                        };
                        pixmap.stroke_path(&path, &paint, &stroke, t, None);
                    }
                }
            }
        }
    }
    Ok(pixmap)
}

/// 그러데이션 → tiny-skia 셰이더. 경로 bbox(pt) 기준, transform=px_scale로 device 정합.
fn gradient_shader(g: &Gradient, cmds: &[PathCmd], px_scale: f32) -> Option<Shader<'static>> {
    let (x0, y0, x1, y1) = path_bbox(cmds);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let stops: Vec<GradientStop> = g
        .stops
        .iter()
        .map(|&(p, c)| {
            let (r, gg, b) = colorref_rgb(c);
            GradientStop::new(p, Color::from_rgba8(r, gg, b, 255))
        })
        .collect();
    if stops.len() < 2 {
        return None;
    }
    let xf = Transform::from_scale(px_scale, px_scale);
    if g.radial {
        let radius = ((x1 - x0).max(y1 - y0) / 2.0).max(0.1);
        RadialGradient::new(
            Point::from_xy(cx, cy),
            0.0,
            Point::from_xy(cx, cy),
            radius,
            stops,
            SpreadMode::Pad,
            xf,
        )
    } else {
        let a = g.angle_deg.to_radians();
        let (dx, dy) = (a.cos(), a.sin());
        let proj = |x: f32, y: f32| (x - cx) * dx + (y - cy) * dy;
        let ps = [proj(x0, y0), proj(x1, y0), proj(x1, y1), proj(x0, y1)];
        let tmin = ps.iter().cloned().fold(f32::MAX, f32::min);
        let tmax = ps.iter().cloned().fold(f32::MIN, f32::max);
        if (tmax - tmin).abs() < 0.01 {
            return None;
        }
        LinearGradient::new(
            Point::from_xy(cx + dx * tmin, cy + dy * tmin),
            Point::from_xy(cx + dx * tmax, cy + dy * tmax),
            stops,
            SpreadMode::Pad,
            xf,
        )
    }
}

/// 인코딩된 이미지를 tiny-skia Pixmap으로 디코드한다 (premultiplied RGBA).
#[derive(Default)]
struct ImageDecodeContext {
    identity_keys: HashMap<(usize, usize), (String, String)>,
    decoded: HashMap<(String, String), Option<Arc<Pixmap>>>,
    unique_pixels: u64,
    unique_decoded_bytes: u64,
    references: u64,
}

impl ImageDecodeContext {
    fn decode(
        &mut self,
        data: &Arc<Vec<u8>>,
        issues: &mut RenderIssueAccumulator,
    ) -> Result<Option<Arc<Pixmap>>, RenderError> {
        self.references = self.references.saturating_add(1);
        if self.references > MAX_IMAGE_REFERENCES {
            return image_budget_error(issues, "references");
        }
        let identity = (data.as_ptr() as usize, data.len());
        let key = if let Some(key) = self.identity_keys.get(&identity) {
            key.clone()
        } else {
            let digest = hex_digest(Sha256::digest(data.as_slice()).as_slice());
            let format = image::guess_format(data)
                .map(image_format_key)
                .unwrap_or("unknown")
                .to_string();
            let key = (digest, format);
            self.identity_keys.insert(identity, key.clone());
            key
        };
        if let Some(cached) = self.decoded.get(&key) {
            return Ok(cached.clone());
        }

        let mut dimensions_reader =
            match ImageReader::new(Cursor::new(data.as_slice())).with_guessed_format() {
                Ok(reader) => reader,
                Err(_) => {
                    self.decoded.insert(key, None);
                    return Ok(None);
                }
            };
        dimensions_reader.limits(image_limits());
        let (width, height) = match dimensions_reader.into_dimensions() {
            Ok(dimensions) => dimensions,
            Err(image::ImageError::Limits(_)) => {
                return image_budget_error(issues, "dimensions");
            }
            Err(_) => {
                self.decoded.insert(key, None);
                return Ok(None);
            }
        };
        let pixels = u64::from(width) * u64::from(height);
        let decoded_bytes = pixels * 4;
        if width > MAX_SOURCE_IMAGE_DIMENSION
            || height > MAX_SOURCE_IMAGE_DIMENSION
            || pixels > MAX_SOURCE_IMAGE_PIXELS
            || decoded_bytes > MAX_SOURCE_IMAGE_DECODED_BYTES
        {
            return image_budget_error(issues, "source_dimensions");
        }
        let next_pixels = self.unique_pixels.saturating_add(pixels);
        let next_bytes = self.unique_decoded_bytes.saturating_add(decoded_bytes);
        if next_pixels > MAX_TOTAL_UNIQUE_IMAGE_PIXELS
            || next_bytes > MAX_TOTAL_UNIQUE_IMAGE_DECODED_BYTES
        {
            return image_budget_error(issues, "aggregate");
        }

        let mut reader = match ImageReader::new(Cursor::new(data.as_slice())).with_guessed_format()
        {
            Ok(reader) => reader,
            Err(_) => {
                self.decoded.insert(key, None);
                return Ok(None);
            }
        };
        reader.limits(image_limits());
        let dynamic = match reader.decode() {
            Ok(image) => image,
            Err(image::ImageError::Limits(_)) => {
                return image_budget_error(issues, "decoder_allocation");
            }
            Err(_) => {
                self.decoded.insert(key, None);
                return Ok(None);
            }
        };
        let mut rgba = dynamic.into_rgba8().into_raw();
        for pixel in rgba.chunks_exact_mut(4) {
            let alpha = u16::from(pixel[3]);
            pixel[0] = (u16::from(pixel[0]) * alpha / 255) as u8;
            pixel[1] = (u16::from(pixel[1]) * alpha / 255) as u8;
            pixel[2] = (u16::from(pixel[2]) * alpha / 255) as u8;
        }
        let Some(size) = tiny_skia::IntSize::from_wh(width, height) else {
            return image_budget_error(issues, "dimensions");
        };
        let Some(pixmap) = Pixmap::from_vec(rgba, size) else {
            return image_budget_error(issues, "pixmap_allocation");
        };
        self.unique_pixels = next_pixels;
        self.unique_decoded_bytes = next_bytes;
        let pixmap = Arc::new(pixmap);
        self.decoded.insert(key, Some(pixmap.clone()));
        Ok(Some(pixmap))
    }
}

fn image_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_SOURCE_IMAGE_DECODED_BYTES);
    limits
}

fn image_format_key(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::Gif => "gif",
        ImageFormat::Bmp => "bmp",
        _ => "other",
    }
}

fn image_budget_error<T>(
    issues: &mut RenderIssueAccumulator,
    reason: &'static str,
) -> Result<T, RenderError> {
    issues.push_once(
        RenderIssueCode::ImageDecodeBudgetExceeded,
        reason.as_bytes(),
    );
    Err(RenderError::ImageDecodeBudgetExceeded {
        resource: reason.to_string(),
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 글리프 런 하나를 (x, y) 베이스라인에 그린다. (dx, dy) 평행이동(그림자용),
/// color로 채운다. bold/italic/장평/글리프 y_offset(첨자) 반영.
#[allow(clippy::too_many_arguments)]
fn draw_glyph_run(
    pixmap: &mut Pixmap,
    face: &ttf_parser::Face<'_>,
    run: &crate::shape::ShapedRun,
    x: f32,
    y: f32,
    px_scale: f32,
    color: u32,
    dx: f32,
    dy: f32,
) {
    let upem = face.units_per_em() as f32;
    let glyph_scale = run.size_pt / upem;
    let (r, g, b) = colorref_rgb(color);
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, 255);
    paint.anti_alias = true;
    let mut pen_x = x;
    for glyph in &run.glyphs {
        let mut builder = OutlinePath::default();
        if face
            .outline_glyph(ttf_parser::GlyphId(glyph.id), &mut builder)
            .is_some()
            && let Some(path) = builder.path.finish()
        {
            // 크기 스케일·y뒤집기(폰트 y-up)·장평·기울임·베이스라인 이동·DPI 스케일
            let mut t = Transform::from_scale(glyph_scale * run.x_scale, -glyph_scale);
            if run.italic {
                t = t.post_concat(Transform::from_skew(-ITALIC_SKEW, 0.0));
            }
            t = t.post_translate(pen_x + glyph.x_offset + dx, y - glyph.y_offset + dy);
            t = t.post_scale(px_scale, px_scale);
            if run.outline {
                // 외곽선: 채움 없이 윤곽선만(빈 글자).
                let stroke = Stroke {
                    width: run.size_pt * 0.025 / glyph_scale,
                    ..Stroke::default()
                };
                pixmap.stroke_path(&path, &paint, &stroke, t, None);
            } else {
                pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
                if run.bold {
                    // 합성 굵게 4.5% (한컴 굵게 대조 보정 — pdf.rs BOLD_STROKE와 동일)
                    let stroke = Stroke {
                        width: run.size_pt * 0.045 / glyph_scale,
                        ..Stroke::default()
                    };
                    pixmap.stroke_path(&path, &paint, &stroke, t, None);
                }
            }
        }
        pen_x += glyph.x_advance;
    }
}

/// COLORREF(0x00BBGGRR) → (r, g, b). 0xFFFFFFFF(없음)는 검정 취급.
fn colorref_rgb(c: u32) -> (u8, u8, u8) {
    if c == 0xFFFF_FFFF {
        return (0, 0, 0);
    }
    (
        (c & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        ((c >> 16) & 0xFF) as u8,
    )
}

#[derive(Default)]
struct OutlinePath {
    path: PathBuilder,
}

impl ttf_parser::OutlineBuilder for OutlinePath {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{DisplayList, PageList};

    fn encoded(format: ImageFormat) -> Vec<u8> {
        let image = image::DynamicImage::new_rgba8(1, 1);
        let mut cursor = Cursor::new(Vec::new());
        image.write_to(&mut cursor, format).unwrap();
        cursor.into_inner()
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        !crc
    }

    fn huge_png() -> Vec<u8> {
        let mut png = encoded(ImageFormat::Png);
        png[16..20].copy_from_slice(&9_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&9_000u32.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    fn huge_jpeg() -> Vec<u8> {
        let mut jpeg = encoded(ImageFormat::Jpeg);
        let sof = jpeg
            .windows(2)
            .position(|bytes| matches!(bytes, [0xff, 0xc0] | [0xff, 0xc2]))
            .expect("JPEG SOF");
        jpeg[sof + 5..sof + 7].copy_from_slice(&9_000u16.to_be_bytes());
        jpeg[sof + 7..sof + 9].copy_from_slice(&9_000u16.to_be_bytes());
        jpeg
    }

    fn page(width_pt: f32, height_pt: f32) -> DisplayList {
        DisplayList {
            pages: vec![PageList {
                width_pt,
                height_pt,
                items: Vec::new(),
            }],
        }
    }

    #[test]
    fn rejects_non_finite_and_out_of_range_dpi() {
        let normal = page(100.0, 100.0);
        for dpi in [0.0, -1.0, f32::NAN, f32::INFINITY, crate::MAX_DPI + 1.0] {
            assert!(render_png(&normal, dpi).is_err(), "dpi={dpi}");
        }
    }

    #[test]
    fn rejects_huge_page_before_pixmap_allocation() {
        let huge = page(f32::MAX, 100.0);
        let error = render_png(&huge, 96.0).unwrap_err().to_string();
        assert!(
            error.contains("래스터 상한") || error.contains("유효한"),
            "{error}"
        );

        let too_many_pixels = page(10_000.0, 10_000.0);
        let error = render_png(&too_many_pixels, 96.0).unwrap_err().to_string();
        assert!(error.contains("픽셀 상한"), "{error}");
    }

    #[test]
    fn selected_page_render_avoids_unselected_aggregate_budget() {
        let list = DisplayList {
            pages: (0..135)
                .map(|_| PageList {
                    width_pt: 1_024.0,
                    height_pt: 1_024.0,
                    items: Vec::new(),
                })
                .collect(),
        };
        assert!(render_png_pages(&list, 72.0, None).is_err());
        let selected = render_png_pages(&list, 72.0, Some(&[2])).unwrap();
        assert_eq!(selected.len(), 1);
    }

    #[test]
    fn tiny_encoded_huge_dimension_png_and_jpeg_fail_before_decode() {
        for bytes in [huge_png(), huge_jpeg()] {
            assert!(bytes.len() < 1024);
            let mut context = ImageDecodeContext::default();
            let mut issues = RenderIssueAccumulator::new();
            let error = context.decode(&Arc::new(bytes), &mut issues).unwrap_err();
            assert!(matches!(
                error,
                RenderError::ImageDecodeBudgetExceeded { .. }
            ));
            let report = issues.finish();
            assert_eq!(report.issue_count, 1);
            assert_eq!(
                report.issues[0].code,
                RenderIssueCode::ImageDecodeBudgetExceeded
            );
        }
    }

    #[test]
    fn twenty_thousand_equal_image_references_decode_once() {
        let bytes = encoded(ImageFormat::Png);
        let first = Arc::new(bytes.clone());
        let duplicate_arc = Arc::new(bytes);
        let mut context = ImageDecodeContext::default();
        let mut issues = RenderIssueAccumulator::new();
        for index in 0..20_000 {
            let source = if index % 2 == 0 {
                &first
            } else {
                &duplicate_arc
            };
            assert!(context.decode(source, &mut issues).unwrap().is_some());
        }
        assert_eq!(context.references, 20_000);
        assert_eq!(context.decoded.len(), 1);
        assert_eq!(context.unique_pixels, 1);
        assert_eq!(context.unique_decoded_bytes, 4);
    }
}
