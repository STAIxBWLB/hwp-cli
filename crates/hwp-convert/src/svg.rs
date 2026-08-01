//! SVG validation and rasterization — the shared path for DocumentSpec v2 svg visuals and
//! from_html `<img src="*.svg">`.
//!
//! Policy: SVG has no native representation in any output format — it always goes through
//! **closed-subset validation (sanitize) + deterministic PNG rasterization (resvg)**. External
//! references, scripts, event handlers, CSS URLs, DTD, PIs, and text nodes (no deterministic
//! font renderer available) are all rejected. This module is the single source of the
//! implementation carried over from `hwp-cli/document_spec_v2.rs`.

use std::collections::BTreeMap;
use std::io::Cursor;

use image::ImageEncoder as _;

/// SVG input byte limit (common to pre- and post-normalization).
pub const MAX_SVG_BYTES: usize = 1024 * 1024;
const MAX_SVG_ELEMENTS: usize = 10_000;
const MAX_SVG_DEPTH: usize = 64;

/// Canonicalized SVG and complexity statistics — used for workload budget calculation.
pub struct SanitizedSvg {
    pub canonical: String,
    pub elements: usize,
    pub geometry_tokens: usize,
}

/// Returns the SVG string validated and canonicalized to the closed subset.
pub fn sanitize_svg(input: &str) -> Result<String, String> {
    Ok(sanitize_svg_with_stats(input)?.canonical)
}

/// Variant that also collects complexity statistics (the caller computes the render workload budget).
pub fn sanitize_svg_with_stats(input: &str) -> Result<SanitizedSvg, String> {
    if input.len() > MAX_SVG_BYTES {
        return Err("SVG exceeds the 1 MiB limit".into());
    }
    let mut reader = quick_xml::Reader::from_str(input);
    reader.config_mut().trim_text(true);
    let mut depth = 0usize;
    let mut elements = 0usize;
    let mut geometry_tokens = 0usize;
    let mut root_seen = false;
    let mut output = String::with_capacity(input.len());
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(element)) => {
                if depth == 0 && root_seen {
                    return Err("SVG must have exactly one root".into());
                }
                depth += 1;
                let (name, attributes) = validate_svg_element(
                    &element,
                    reader.decoder(),
                    &mut root_seen,
                    &mut elements,
                    &mut geometry_tokens,
                    depth,
                )?;
                write_canonical_svg_start(&mut output, &name, &attributes, false);
            }
            Ok(quick_xml::events::Event::Empty(element)) => {
                if depth == 0 && root_seen {
                    return Err("SVG must have exactly one root".into());
                }
                let (name, attributes) = validate_svg_element(
                    &element,
                    reader.decoder(),
                    &mut root_seen,
                    &mut elements,
                    &mut geometry_tokens,
                    depth + 1,
                )?;
                write_canonical_svg_start(&mut output, &name, &attributes, true);
            }
            Ok(quick_xml::events::Event::End(element)) => {
                let raw_name = element.name();
                let name_bytes = element.local_name();
                if raw_name.as_ref() != name_bytes.as_ref() {
                    return Err("prefixed SVG element names are forbidden".into());
                }
                let name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|_| "SVG end name is not UTF-8".to_string())?;
                output.push_str("</");
                output.push_str(name);
                output.push('>');
                depth = depth.saturating_sub(1);
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if !text.iter().all(u8::is_ascii_whitespace) {
                    return Err(
                        "SVG text nodes require an unavailable deterministic font renderer".into(),
                    );
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(quick_xml::events::Event::Comment(_)) => {
                return Err("SVG comments are forbidden in canonical input".into());
            }
            Ok(_) => {
                return Err(
                    "SVG DTD, declaration, CDATA, or processing instruction is forbidden".into(),
                );
            }
            Err(error) => return Err(format!("invalid SVG: {error}")),
        }
    }
    if !root_seen || depth != 0 {
        return Err("SVG root or nesting is invalid".into());
    }
    Ok(SanitizedSvg {
        canonical: output,
        elements,
        geometry_tokens,
    })
}

fn validate_svg_element(
    element: &quick_xml::events::BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
    root_seen: &mut bool,
    elements: &mut usize,
    geometry_tokens: &mut usize,
    depth: usize,
) -> Result<(String, BTreeMap<String, String>), String> {
    *elements += 1;
    if *elements > MAX_SVG_ELEMENTS || depth > MAX_SVG_DEPTH {
        return Err("SVG complexity budget exceeded".into());
    }
    let raw_name = element.name();
    let name_bytes = element.local_name();
    if raw_name.as_ref() != name_bytes.as_ref() {
        return Err("prefixed SVG element names are forbidden".into());
    }
    let name = std::str::from_utf8(name_bytes.as_ref())
        .map_err(|_| "SVG element name is not UTF-8".to_string())?;
    const ELEMENTS: &[&str] = &[
        "svg", "g", "rect", "ellipse", "circle", "line", "polyline", "polygon",
    ];
    if !ELEMENTS.contains(&name) {
        return Err(format!("forbidden SVG element: {name}"));
    }
    let is_root = !*root_seen;
    if is_root {
        if name != "svg" {
            return Err("SVG root element must be svg".into());
        }
        *root_seen = true;
    } else if name == "svg" {
        return Err("nested SVG roots are forbidden".into());
    }
    let allowed_attributes: &[&str] = match name {
        "svg" => &["xmlns", "width", "height", "viewBox"],
        "g" => &[],
        "rect" => &[
            "x",
            "y",
            "width",
            "height",
            "fill",
            "stroke",
            "stroke-width",
            "stroke-dasharray",
        ],
        "ellipse" => &[
            "cx",
            "cy",
            "rx",
            "ry",
            "fill",
            "stroke",
            "stroke-width",
            "stroke-dasharray",
        ],
        "circle" => &[
            "cx",
            "cy",
            "r",
            "fill",
            "stroke",
            "stroke-width",
            "stroke-dasharray",
        ],
        "line" => &[
            "x1",
            "y1",
            "x2",
            "y2",
            "stroke",
            "stroke-width",
            "stroke-dasharray",
        ],
        "polyline" | "polygon" => &[
            "points",
            "fill",
            "stroke",
            "stroke-width",
            "stroke-dasharray",
        ],
        _ => unreachable!(),
    };
    let mut has_namespace = false;
    let mut canonical_attributes = BTreeMap::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| format!("invalid SVG attribute: {error}"))?;
        let key = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|_| "SVG attribute name is not UTF-8".to_string())?;
        if !allowed_attributes.contains(&key)
            || key.starts_with("on")
            || key.eq_ignore_ascii_case("href")
            || key.eq_ignore_ascii_case("xlink:href")
        {
            return Err(format!("forbidden SVG attribute: {key}"));
        }
        let value = attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, decoder)
            .map_err(|error| format!("invalid SVG value: {error}"))?;
        if key == "xmlns" {
            if !is_root || value != "http://www.w3.org/2000/svg" {
                return Err("SVG root requires the exact SVG namespace".into());
            }
            has_namespace = true;
            if canonical_attributes
                .insert(key.to_string(), value.to_string())
                .is_some()
            {
                return Err("duplicate SVG attribute".into());
            }
            continue;
        }
        let lower = value.to_ascii_lowercase();
        if lower.contains("url(")
            || lower.contains("http:")
            || lower.contains("https:")
            || lower.contains("file:")
            || lower.contains("@import")
        {
            return Err("external SVG reference is forbidden".into());
        }
        let canonical = canonical_svg_attribute(key, &value)?;
        if key == "points" {
            *geometry_tokens = geometry_tokens.saturating_add(canonical.split(' ').count());
        }
        if canonical_attributes
            .insert(key.to_string(), canonical)
            .is_some()
        {
            return Err("duplicate SVG attribute".into());
        }
    }
    if is_root && !has_namespace {
        return Err("SVG root requires the exact SVG namespace".into());
    }
    if is_root && !canonical_attributes.contains_key("viewBox") {
        return Err("canonical SVG root requires a bounded viewBox".into());
    }
    validate_svg_geometry(name, &canonical_attributes)?;
    Ok((name.to_string(), canonical_attributes))
}

fn canonical_svg_attribute(key: &str, value: &str) -> Result<String, String> {
    match key {
        "fill" | "stroke" => {
            if value == "none" {
                return Ok(value.to_string());
            }
            let color = value
                .strip_prefix('#')
                .filter(|hex| hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
            color
                .map(|hex| format!("#{}", hex.to_ascii_uppercase()))
                .ok_or_else(|| "SVG color must be none or #RRGGBB".to_string())
        }
        "viewBox" => canonical_svg_numbers(value, 4, 4),
        "points" => {
            let canonical = canonical_svg_numbers(value, 4, 4_096)?;
            if canonical.split(' ').count() % 2 != 0 {
                return Err("SVG points require coordinate pairs".into());
            }
            Ok(canonical)
        }
        "stroke-dasharray" => canonical_svg_numbers(value, 1, 64),
        _ => canonical_svg_numbers(value, 1, 1),
    }
}

fn validate_svg_geometry(
    element: &str,
    attributes: &BTreeMap<String, String>,
) -> Result<(), String> {
    let number = |key: &str| {
        attributes
            .get(key)
            .and_then(|value| value.parse::<f64>().ok())
    };
    if element == "svg" {
        let values = attributes["viewBox"]
            .split(' ')
            .filter_map(|value| value.parse::<f64>().ok())
            .collect::<Vec<_>>();
        if values.len() != 4 || values[2] <= 0.0 || values[3] <= 0.0 {
            return Err("SVG viewBox width and height must be positive".into());
        }
    }
    for key in ["width", "height", "stroke-width"] {
        if number(key).is_some_and(|value| value < 0.0) {
            return Err("SVG widths must be non-negative".into());
        }
    }
    for key in ["r", "rx", "ry"] {
        if number(key).is_some_and(|value| value <= 0.0) {
            return Err("SVG radii must be positive".into());
        }
    }
    if attributes.get("stroke-dasharray").is_some_and(|value| {
        value
            .split(' ')
            .filter_map(|item| item.parse::<f64>().ok())
            .any(|item| item < 0.0)
    }) {
        return Err("SVG dash lengths must be non-negative".into());
    }
    Ok(())
}

fn canonical_svg_numbers(value: &str, minimum: usize, maximum: usize) -> Result<String, String> {
    if value.len() > 65_536 {
        return Err("SVG numeric data is too long".into());
    }
    let values = value
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let number = part
                .parse::<f64>()
                .map_err(|_| "SVG number is invalid".to_string())?;
            if !number.is_finite() || number.abs() > 1_000_000.0 {
                return Err("SVG number must be finite and within +/-1000000".to_string());
            }
            let number = if number == 0.0 { 0.0 } else { number };
            Ok(format!("{number}"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    if !(minimum..=maximum).contains(&values.len()) {
        return Err("SVG numeric list exceeds the closed-subset budget".into());
    }
    Ok(values.join(" "))
}

fn write_canonical_svg_start(
    output: &mut String,
    name: &str,
    attributes: &BTreeMap<String, String>,
    empty: bool,
) {
    output.push('<');
    output.push_str(name);
    for (key, value) in attributes {
        output.push(' ');
        output.push_str(key);
        output.push_str("=\"");
        for character in value.chars() {
            match character {
                '&' => output.push_str("&amp;"),
                '<' => output.push_str("&lt;"),
                '"' => output.push_str("&quot;"),
                _ => output.push(character),
            }
        }
        output.push('"');
    }
    output.push_str(if empty { "/>" } else { ">" });
}

/// Returns the intrinsic size (px, at 96dpi) of a canonicalized SVG.
pub fn size_px(canonical_svg: &str) -> Result<(f32, f32), String> {
    let tree = parse_tree(canonical_svg)?;
    Ok((tree.size().width(), tree.size().height()))
}

/// Rasterizes a canonicalized SVG to a PNG of the explicit pixel size (deterministic — same
/// input, same bytes). `max_bytes` is the limit for the output PNG.
pub fn rasterize_svg_png(
    canonical_svg: &str,
    width_px: u32,
    height_px: u32,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let tree = parse_tree(canonical_svg)?;
    if tree.has_text_nodes() {
        return Err("SVG text requires an unavailable deterministic font pipeline".into());
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width_px, height_px)
        .ok_or_else(|| "SVG pixel buffer allocation failed".to_string())?;
    let scale_x = width_px as f32 / tree.size().width();
    let scale_y = height_px as f32 / tree.size().height();
    if !scale_x.is_finite() || !scale_y.is_finite() || scale_x <= 0.0 || scale_y <= 0.0 {
        return Err("SVG viewport is invalid".into());
    }
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale_x, scale_y),
        &mut pixmap.as_mut(),
    );
    if !pixmap.pixels().iter().any(|pixel| pixel.alpha() != 0) {
        return Err("SVG render is empty or fully transparent".into());
    }
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.pixels() {
        if pixel.alpha() == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let color = pixel.demultiply();
            rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
        }
    }
    let image = image::RgbaImage::from_raw(width_px, height_px, rgba)
        .ok_or_else(|| "SVG RGBA buffer is invalid".to_string())?;
    let png = encode_png(image)?;
    if png.len() as u64 > max_bytes {
        return Err("SVG PNG output exceeds the asset byte budget".into());
    }
    Ok(png)
}

fn parse_tree(canonical_svg: &str) -> Result<resvg::usvg::Tree, String> {
    let options = resvg::usvg::Options::default();
    if options.resources_dir.is_some() {
        return Err("SVG resource resolution must remain disabled".into());
    }
    resvg::usvg::Tree::from_str(canonical_svg, &options)
        .map_err(|_| "sanitized SVG could not be parsed".to_string())
}

/// PNG encoder (deterministic — Best compression + Adaptive filter). Shared with the image fallback path.
pub fn encode_png(image: image::RgbaImage) -> Result<Vec<u8>, String> {
    let mut output = Cursor::new(Vec::new());
    image::codecs::png::PngEncoder::new_with_quality(
        &mut output,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|error| format!("PNG encode failed: {error}"))?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\">\
        <rect x=\"10\" y=\"10\" width=\"80\" height=\"60\" fill=\"#FF0000\"/></svg>";

    #[test]
    fn 정규화와_래스터화() {
        let canonical = sanitize_svg(RECT).unwrap();
        let (w, h) = size_px(&canonical).unwrap();
        assert_eq!((w, h), (100.0, 100.0));
        let png = rasterize_svg_png(&canonical, 100, 100, 1_000_000).unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
        // Determinism: same input, same bytes.
        let again = rasterize_svg_png(&canonical, 100, 100, 1_000_000).unwrap();
        assert_eq!(png, again);
    }

    #[test]
    fn 위험_요소는_거부() {
        for hostile in [
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"><script>alert(1)</script></svg>",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"><rect x=\"0\" y=\"0\" width=\"1\" height=\"1\" fill=\"url(http://evil)\"/></svg>",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\"><text x=\"0\" y=\"1\">t</text></svg>",
            "<svg viewBox=\"0 0 1 1\"><rect x=\"0\" y=\"0\" width=\"1\" height=\"1\"/></svg>", // no xmlns
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect x=\"0\" y=\"0\" width=\"1\" height=\"1\"/></svg>", // no viewBox
        ] {
            assert!(sanitize_svg(hostile).is_err(), "accepted: {hostile}");
        }
    }
}
