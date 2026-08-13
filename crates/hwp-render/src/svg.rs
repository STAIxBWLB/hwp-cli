//! SVG 백엔드 — DisplayList를 페이지별 SVG 문서로 직렬화.
//!
//! 글리프는 윤곽선 `<path>`로 그린다(뷰어 폰트 의존 제거 — 픽셀 정확도
//! 우선). 이미지는 base64 data URI로 임베드한다.

use std::collections::HashMap;
use std::fmt::Write as _;

use rustybuzz::ttf_parser;

use crate::display::{DisplayList, Fill, Gradient, Item, PageList, PathCmd, path_bbox};

pub fn render_svg(list: &DisplayList) -> Vec<String> {
    list.pages.iter().map(render_page).collect()
}

fn render_page(page: &PageList) -> String {
    let (w, h) = (page.width_pt, page.height_pt);
    let mut out = String::with_capacity(64 * 1024);
    let _ = write!(
        out,
        r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{w:.2}pt" height="{h:.2}pt" viewBox="0 0 {w:.2} {h:.2}">
<rect width="{w:.2}" height="{h:.2}" fill="#ffffff"/>
"##
    );

    // (폰트 데이터 주소, 글리프 ID) → path d 캐시
    let mut outline_cache: HashMap<(usize, u16), Option<String>> = HashMap::new();
    let mut grad_id = 0usize;
    let mut clip_id = 0usize;

    for item in &page.items {
        match item {
            Item::Rect {
                x,
                y,
                w: rw,
                h: rh,
                fill,
            } => {
                let _ = writeln!(
                    out,
                    r#"<rect x="{x:.2}" y="{y:.2}" width="{rw:.2}" height="{rh:.2}" fill="{}"/>"#,
                    hex_color(*fill)
                );
            }
            Item::Image {
                x,
                y,
                w: iw,
                h: ih,
                data,
                crop,
                flip,
                rotation_deg,
                brightness,
                contrast,
            } => {
                // Preserve original bytes when adjustments are zero. Otherwise decode,
                // adjust pixels, and re-encode as PNG.
                let (mime, payload) = if *brightness == 0 && *contrast == 0 {
                    (sniff_mime(data), base64(data))
                } else {
                    match effect_png_data_url(data, *brightness, *contrast) {
                        Some(png) => ("image/png", png),
                        None => (sniff_mime(data), base64(data)),
                    }
                };
                // Expand a cropped image to fill the destination box and clip its viewport.
                let (mut vx, mut vy, mut vw, mut vh) = (*x, *y, *iw, *ih);
                let mut clip_attr = String::new();
                if let Some(c) = crop
                    && let Some((pw, ph)) = image_dimensions(data)
                    && let Some([fl, ft, fr, fb]) = crate::display::crop_fractions(*c, pw, ph)
                    && (fl > 0.0 || ft > 0.0 || fr < 1.0 || fb < 1.0)
                {
                    vw = *iw / (fr - fl);
                    vh = *ih / (fb - ft);
                    vx = *x - fl * vw;
                    vy = *y - ft * vh;
                    let id = format!("clip{clip_id}");
                    clip_id += 1;
                    let _ = writeln!(
                        out,
                        r#"<clipPath id="{id}"><rect x="{x:.2}" y="{y:.2}" width="{iw:.2}" height="{ih:.2}"/></clipPath>"#
                    );
                    clip_attr = format!(r#" clip-path="url(#{id})""#);
                }
                let mut transform_attr = String::new();
                if *flip != 0 || *rotation_deg != 0.0 {
                    let m = crate::display::flip_rotate_matrix(
                        *x + *iw / 2.0,
                        *y + *ih / 2.0,
                        *flip,
                        *rotation_deg,
                    );
                    transform_attr = format!(
                        r#" transform="matrix({:.4} {:.4} {:.4} {:.4} {:.2} {:.2})""#,
                        m[0], m[1], m[2], m[3], m[4], m[5]
                    );
                }
                let _ = writeln!(
                    out,
                    r#"<image x="{vx:.2}" y="{vy:.2}" width="{vw:.2}" height="{vh:.2}" preserveAspectRatio="none"{transform_attr}{clip_attr} href="data:{mime};base64,{payload}"/>"#
                );
            }
            Item::Glyphs { x, y, run } => {
                let Ok(face) = ttf_parser::Face::parse(&run.font.data, run.font.index) else {
                    continue;
                };
                let upem = face.units_per_em() as f32;
                let s = run.size_pt / upem;
                let font_key = run.font.data.as_ptr() as usize;
                let color = hex_color(run.color);
                let skew_c = if run.italic { 0.2126 * s } else { 0.0 };
                // 외곽선=채움없이 윤곽선만, 굵게=채움+스트로크, 기본=채움.
                let main_attr = if run.outline {
                    format!(
                        r#" fill="none" stroke="{color}" stroke-width="{:.1}""#,
                        0.025 * upem
                    )
                } else if run.bold {
                    // 합성 굵게 4.5% (한컴 굵게 대조 보정 — pdf.rs BOLD_STROKE와 동일)
                    format!(
                        r#" fill="{color}" stroke="{color}" stroke-width="{:.1}""#,
                        0.045 * upem
                    )
                } else {
                    format!(r#" fill="{color}""#)
                };

                // 글자 음영(배경 하이라이트) — 글리프 뒤 사각형.
                if run.shade_color != 0xFFFF_FFFF {
                    let _ = writeln!(
                        out,
                        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}"/>"#,
                        x,
                        y - run.size_pt * 0.8,
                        run.width_pt,
                        run.size_pt,
                        hex_color(run.shade_color)
                    );
                }
                // 그림자 — 본문 전에 오프셋 복사.
                if let Some(sc) = run.shadow {
                    let (sdx, sdy) = run.shadow_offset();
                    let shc = hex_color(sc);
                    let mut pen_x = *x;
                    for glyph in &run.glyphs {
                        let d = outline_cache
                            .entry((font_key, glyph.id))
                            .or_insert_with(|| glyph_path(&face, glyph.id))
                            .clone();
                        if let Some(d) = d {
                            let (a, dd) = (s * run.x_scale, -s);
                            let (e, f) = (pen_x + glyph.x_offset + sdx, y - glyph.y_offset + sdy);
                            let _ = writeln!(
                                out,
                                r#"<path transform="matrix({a:.4} 0 {skew_c:.4} {dd:.4} {e:.2} {f:.2})" d="{d}" fill="{shc}"/>"#
                            );
                        }
                        pen_x += glyph.x_advance;
                    }
                }
                // 양각/음각 — 흰 하이라이트 사본 오프셋(양각=좌상, 음각=우하).
                if run.emboss || run.engrave {
                    let rd = run.size_pt * 0.05 * if run.emboss { -1.0 } else { 1.0 };
                    let mut pen_x = *x;
                    for glyph in &run.glyphs {
                        let d = outline_cache
                            .entry((font_key, glyph.id))
                            .or_insert_with(|| glyph_path(&face, glyph.id))
                            .clone();
                        if let Some(d) = d {
                            let (a, dd) = (s * run.x_scale, -s);
                            let (e, f) = (pen_x + glyph.x_offset + rd, y - glyph.y_offset + rd);
                            let _ = writeln!(
                                out,
                                r##"<path transform="matrix({a:.4} 0 {skew_c:.4} {dd:.4} {e:.2} {f:.2})" d="{d}" fill="#ffffff"/>"##
                            );
                        }
                        pen_x += glyph.x_advance;
                    }
                }

                let mut pen_x = *x;
                for glyph in &run.glyphs {
                    let d = outline_cache
                        .entry((font_key, glyph.id))
                        .or_insert_with(|| glyph_path(&face, glyph.id))
                        .clone();
                    if let Some(d) = d {
                        let (a, dd) = (s * run.x_scale, -s);
                        let (e, f) = (pen_x + glyph.x_offset, y - glyph.y_offset);
                        let _ = writeln!(
                            out,
                            r#"<path transform="matrix({a:.4} 0 {skew_c:.4} {dd:.4} {e:.2} {f:.2})" d="{d}"{main_attr}/>"#
                        );
                    }
                    pen_x += glyph.x_advance;
                }
            }
            Item::Path {
                commands,
                fill,
                stroke,
            } => {
                let mut d = String::new();
                for cmd in commands {
                    match *cmd {
                        PathCmd::MoveTo(x, y) => {
                            let _ = write!(d, "M{x:.2} {y:.2}");
                        }
                        PathCmd::LineTo(x, y) => {
                            let _ = write!(d, "L{x:.2} {y:.2}");
                        }
                        PathCmd::CubicTo(a, b, c, e, f, g) => {
                            let _ = write!(d, "C{a:.2} {b:.2} {c:.2} {e:.2} {f:.2} {g:.2}");
                        }
                        PathCmd::Close => d.push('Z'),
                    }
                }
                let fill_attr = match fill {
                    None => "none".to_string(),
                    Some(Fill::Solid(c)) => hex_color(*c),
                    Some(Fill::Gradient(g)) => {
                        let id = format!("grad{grad_id}");
                        grad_id += 1;
                        out.push_str(&svg_gradient_def(&id, g, commands));
                        format!("url(#{id})")
                    }
                    Some(Fill::Hatch { fg, bg, style }) => {
                        let id = format!("hatch{grad_id}");
                        grad_id += 1;
                        out.push_str(&svg_hatch_def(&id, *fg, *bg, *style));
                        format!("url(#{id})")
                    }
                };
                let stroke_attr = match stroke {
                    Some(s) => {
                        let dash = if s.dash.len() >= 2 {
                            let arr: Vec<String> =
                                s.dash.iter().map(|v| format!("{v:.2}")).collect();
                            format!(r#" stroke-dasharray="{}""#, arr.join(","))
                        } else {
                            String::new()
                        };
                        format!(
                            r#" stroke="{}" stroke-width="{:.2}"{dash}"#,
                            hex_color(s.color),
                            s.width
                        )
                    }
                    None => String::new(),
                };
                let _ = writeln!(out, r#"<path d="{d}" fill="{fill_attr}"{stroke_attr}/>"#);
            }
        }
    }
    out.push_str("</svg>\n");
    out
}

fn glyph_path(face: &ttf_parser::Face<'_>, glyph_id: u16) -> Option<String> {
    let mut builder = SvgPath(String::new());
    face.outline_glyph(ttf_parser::GlyphId(glyph_id), &mut builder)?;
    Some(builder.0)
}

struct SvgPath(String);

impl ttf_parser::OutlineBuilder for SvgPath {
    fn move_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "M{x:.1} {y:.1}");
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let _ = write!(self.0, "L{x:.1} {y:.1}");
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let _ = write!(self.0, "Q{x1:.1} {y1:.1} {x:.1} {y:.1}");
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let _ = write!(self.0, "C{x1:.1} {y1:.1} {x2:.1} {y2:.1} {x:.1} {y:.1}");
    }
    fn close(&mut self) {
        self.0.push('Z');
    }
}

/// 그러데이션 정의(<linearGradient>/<radialGradient>) 문자열. userSpaceOnUse(경로 좌표 pt).
fn svg_gradient_def(id: &str, g: &Gradient, cmds: &[PathCmd]) -> String {
    let (x0, y0, x1, y1) = path_bbox(cmds);
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let mut stops = String::new();
    for &(p, c) in &g.stops {
        let _ = write!(
            stops,
            r#"<stop offset="{p:.3}" stop-color="{}"/>"#,
            hex_color(c)
        );
    }
    if g.radial {
        let r = ((x1 - x0).max(y1 - y0) / 2.0).max(0.1);
        format!(
            "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" cx=\"{cx:.2}\" cy=\"{cy:.2}\" r=\"{r:.2}\">{stops}</radialGradient>\n"
        )
    } else {
        let a = g.angle_deg.to_radians();
        let (dx, dy) = (a.cos(), a.sin());
        let proj = |x: f32, y: f32| (x - cx) * dx + (y - cy) * dy;
        let ps = [proj(x0, y0), proj(x1, y0), proj(x1, y1), proj(x0, y1)];
        let tmin = ps.iter().cloned().fold(f32::MAX, f32::min);
        let tmax = ps.iter().cloned().fold(f32::MIN, f32::max);
        let (gx0, gy0) = (cx + dx * tmin, cy + dy * tmin);
        let (gx1, gy1) = (cx + dx * tmax, cy + dy * tmax);
        format!(
            "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{gx0:.2}\" y1=\"{gy0:.2}\" x2=\"{gx1:.2}\" y2=\"{gy1:.2}\">{stops}</linearGradient>\n"
        )
    }
}

/// Defines a hatch `<pattern>` in user space so adjacent cells share phase.
/// Diagonal tiles are scaled by sqrt(2) to preserve `HATCH_SPACING` vertically,
/// matching the PNG and PDF backends.
fn svg_hatch_def(id: &str, fg: u32, bg: u32, style: u32) -> String {
    let diagonal = matches!(style, 3 | 4 | 6);
    let tile = if diagonal {
        crate::display::HATCH_SPACING * std::f32::consts::SQRT_2
    } else {
        crate::display::HATCH_SPACING
    };
    let lw = crate::display::HATCH_LINE_WIDTH;
    let fg = hex_color(fg);
    let mut content = String::new();
    if bg != 0xFFFF_FFFF {
        let _ = write!(
            content,
            r#"<rect width="{tile:.3}" height="{tile:.3}" fill="{}"/>"#,
            hex_color(bg)
        );
    }
    let line = |x1: f32, y1: f32, x2: f32, y2: f32| {
        format!(
            r#"<line x1="{x1:.3}" y1="{y1:.3}" x2="{x2:.3}" y2="{y2:.3}" stroke="{fg}" stroke-width="{lw}"/>"#
        )
    };
    match style {
        1 => content.push_str(&line(0.0, 0.0, tile, 0.0)),
        2 => content.push_str(&line(0.0, 0.0, 0.0, tile)),
        3 => content.push_str(&line(0.0, 0.0, tile, tile)),
        4 => content.push_str(&line(0.0, tile, tile, 0.0)),
        5 => {
            content.push_str(&line(0.0, 0.0, tile, 0.0));
            content.push_str(&line(0.0, 0.0, 0.0, tile));
        }
        6 => {
            content.push_str(&line(0.0, 0.0, tile, tile));
            content.push_str(&line(0.0, tile, tile, 0.0));
        }
        _ => {}
    }
    format!(
        r#"<pattern id="{id}" patternUnits="userSpaceOnUse" width="{tile:.3}" height="{tile:.3}">{content}</pattern>
"#
    )
}

/// COLORREF(0x00BBGGRR) → "#rrggbb". 없음(0xFFFFFFFF)은 검정.
fn hex_color(c: u32) -> String {
    if c == 0xFFFF_FFFF {
        return "#000000".to_string();
    }
    format!(
        "#{:02x}{:02x}{:02x}",
        c & 0xFF,
        (c >> 8) & 0xFF,
        (c >> 16) & 0xFF
    )
}

fn sniff_mime(data: &[u8]) -> &'static str {
    match data {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// Reads image dimensions from the header for crop-fraction conversion.
fn image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Applies brightness and contrast, then returns a base64 PNG data-URL payload.
fn effect_png_data_url(data: &[u8], brightness: i8, contrast: i8) -> Option<String> {
    let dynamic = image::load_from_memory(data).ok()?;
    let mut rgba = dynamic.to_rgba8();
    for pixel in rgba.pixels_mut() {
        let [r, g, b, _] = &mut pixel.0;
        *r = crate::display::apply_brightness_contrast(*r, brightness, contrast);
        *g = crate::display::apply_brightness_contrast(*g, brightness, contrast);
        *b = crate::display::apply_brightness_contrast(*b, brightness, contrast);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;
    Some(base64(&buf.into_inner()))
}

/// 표준 base64 인코딩 (의존성 없이).
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_인코딩() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn 색_변환() {
        assert_eq!(hex_color(0x00FF0000), "#0000ff"); // BGR → 파랑
        assert_eq!(hex_color(0), "#000000");
    }
}
