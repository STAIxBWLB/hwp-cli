//! PDF 백엔드 — DisplayList를 단일 멀티페이지 PDF로 직렬화.
//!
//! 텍스트는 폰트를 서브셋([`subsetter`])해 Identity-H 합성(Type0/CID) 폰트로
//! 임베드하고 텍스트 연산자로 그린다 → 뷰어에서 선택·검색·복사 가능.
//! ToUnicode CMap으로 검색성을 보장한다. 글리프 위치는 png/svg 백엔드와 같은
//! 셰이핑 advance를 텍스트 행렬로 명시 배치해 픽셀 일치를 노린다.
//! 이미지는 JPEG는 원본을 DCTDecode로, 그 외는 [`image`]로 디코드해 RGB(+SMask)로
//! 임베드한다. 좌표는 DisplayList(좌상단·y아래) → PDF(좌하단·y위)로 뒤집는다.
//!
//! Document metadata emits the six Hancom PDF surface features from design 21 §2:
//! /Lang, /PageLayout, /MarkInfo, XMP /Metadata, /OutputIntents with an embedded sRGB ICC
//! profile, and /Info. Timestamps come only from the document metadata so repeated renders remain
//! byte-identical.
//!
//! 폰트 아웃라인 종류별 임베드:
//! - glyf(트루타입): CIDFontType2 + FontFile2(Length1).
//! - CFF(OTF, `CFF ` 테이블): CIDFontType0 + FontFile3(Subtype=OpenType).
//!   둘 다 서브셋 + Identity-H + ToUnicode로 동일하게 처리한다(렌더·검색·복사
//!   poppler로 검증, tests/pdf_cff.rs). 서브셋 실패 시 전체 폰트를 같은 구조로
//!   임베드한다(CID=GID).

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::{Arc, OnceLock};

use flate2::Compression;
use flate2::write::ZlibEncoder;
use pdf_writer::types::{
    CidFontType, FontFlags, OutputIntentSubtype, PageLayout, SystemInfo, TextRenderingMode,
    UnicodeCmap,
};
use pdf_writer::{Content, Date, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use hwp_model::{Metadata, filetime_to_iso8601_utc};
use rustybuzz::ttf_parser;
use subsetter::{GlyphRemapper, subset};

use crate::display::{DisplayList, Fill, Gradient, Item, PathCmd, Stroke, path_bbox};
use crate::error::RenderError;
use crate::fonts::LoadedFont;
use crate::issues::{RenderIssueAccumulator, RenderIssueCode};
use crate::shape::{ShapedRun, glyph_source_sequences};

/// 합성 기울임 탄젠트 (png.rs/svg.rs와 동일, ≈12°).
const ITALIC_SKEW: f32 = 0.2126;
/// 합성 굵게 스트로크 굵기 (글자 크기 대비, png.rs와 동일). 한컴 굵게 대조 보정(4.5%).
const BOLD_STROKE: f32 = 0.045;

/// Render a display list without document metadata.
///
/// This retains the pre-PR-82 public API for callers that build a [`DisplayList`] directly.
pub fn render_pdf(
    list: &DisplayList,
    warnings: &mut RenderIssueAccumulator,
) -> Result<Vec<u8>, RenderError> {
    render_pdf_with_metadata(list, warnings, &Metadata::default())
}

/// Render a display list with document metadata supplied by the high-level document APIs.
pub(crate) fn render_pdf_with_metadata(
    list: &DisplayList,
    warnings: &mut RenderIssueAccumulator,
    meta: &Metadata,
) -> Result<Vec<u8>, RenderError> {
    // ── 1. Collect font variants and complete source mappings for ToUnicode. ──
    // A source font may need multiple PDF font resources when one original GID
    // represents different Unicode sequences (for example, cmap aliases).
    let (mut fonts, glyph_font_index) = collect_font_variants(list)?;

    // ── 2. ref 할당 + 폰트 서브셋 ──
    let mut counter = 1i32;
    let catalog_id = alloc(&mut counter);
    let page_tree_id = alloc(&mut counter);
    let info_id = alloc(&mut counter);
    let xmp_id = alloc(&mut counter);
    let icc_id = alloc(&mut counter);
    for (i, f) in fonts.iter_mut().enumerate() {
        f.res_name = format!("F{i}");
        f.type0_id = alloc(&mut counter);
        f.cid_id = alloc(&mut counter);
        f.desc_id = alloc(&mut counter);
        f.ff_id = alloc(&mut counter);
        f.tounicode_id = alloc(&mut counter);
        match subset(&f.data, f.index, &f.remapper) {
            Ok(bytes) => {
                f.subset_ok = true;
                f.subset_bytes = bytes;
            }
            Err(e) => {
                // 서브셋 실패: 전체 폰트 임베드 + 원본 글리프 ID 사용 (조용한 누락 금지).
                f.subset_ok = false;
                f.subset_bytes = f.data.as_ref().clone();
                warnings.push(RenderIssueCode::FontSubsetFallback, format!("{e:?}"));
            }
        }
        // 서브셋 폰트는 관례상 6글자 태그 접두사("ABCDEF+이름")를 BaseFont에 붙인다.
        f.base_font = if f.subset_ok {
            format!("{}+{}", subset_tag(i), f.res_name)
        } else {
            f.res_name.clone()
        };
        // ToUnicode는 출력 글리프 ID(서브셋이면 재매핑, 아니면 원본) 기준으로 키.
        let mut tu = HashMap::with_capacity(f.orig_to_unicode.len());
        for (&orig, source) in &f.orig_to_unicode {
            tu.insert(out_gid(f.subset_ok, &f.remapper, orig), source.clone());
        }
        f.to_unicode = tu;
    }

    // ── 3. 페이지 콘텐츠 스트림 빌드 ──
    let mut plans: Vec<PagePlan> = Vec::new();
    for page in &list.pages {
        let page_id = alloc(&mut counter);
        let content_id = alloc(&mut counter);
        let (w, h) = (page.width_pt, page.height_pt);

        let mut content = Content::new();
        // 흰 배경 (png.rs:25 / svg.rs:24와 동일 — 투명 겹침을 흰 바탕에 그림).
        content.set_fill_rgb(1.0, 1.0, 1.0);
        content.rect(0.0, 0.0, w, h);
        content.fill_nonzero();

        let mut images: Vec<ImagePlan> = Vec::new();
        for item in &page.items {
            match item {
                Item::Rect {
                    x,
                    y,
                    w: rw,
                    h: rh,
                    fill,
                } => {
                    let (r, g, b) = colorref_rgb(*fill);
                    content.set_fill_rgb(r, g, b);
                    content.rect(*x, h - (*y + *rh), *rw, *rh);
                    content.fill_nonzero();
                }
                Item::Image {
                    x,
                    y,
                    w: iw,
                    h: ih,
                    data,
                } => match decode_image(data) {
                    Some(payload) => {
                        let id = alloc(&mut counter);
                        let smask_id = matches!(
                            &payload,
                            ImagePayload::Raw {
                                alpha_z: Some(_),
                                ..
                            }
                        )
                        .then(|| alloc(&mut counter));
                        let name = format!("Im{}", images.len());
                        content.save_state();
                        content.transform([*iw, 0.0, 0.0, *ih, *x, h - (*y + *ih)]);
                        content.x_object(Name(name.as_bytes()));
                        content.restore_state();
                        images.push(ImagePlan {
                            id,
                            smask_id,
                            name,
                            payload,
                        });
                    }
                    None => {
                        // 디코드 실패: 자홍색 placeholder (png.rs:100과 동일 — 조용한 누락 금지).
                        content.set_fill_rgb(1.0, 0.0, 1.0);
                        content.rect(*x, h - (*y + *ih), *iw, *ih);
                        content.fill_nonzero();
                        warnings.push(RenderIssueCode::ImageDecodePlaceholder, b"pdf");
                    }
                },
                Item::Glyphs { x, y, run } => {
                    let sources = glyph_source_sequences(run);
                    let glyph_fonts: Vec<usize> = run
                        .glyphs
                        .iter()
                        .zip(sources.iter())
                        .filter_map(|(glyph, source)| {
                            glyph_font_index
                                .get(&(font_key(&run.font), glyph.id, source.clone()))
                                .copied()
                        })
                        .collect();
                    if glyph_fonts.len() == run.glyphs.len() {
                        // 글자 음영(배경 하이라이트) — 글리프 뒤 사각형.
                        if run.shade_color != 0xFFFF_FFFF {
                            let (sr, sg, sb) = colorref_rgb(run.shade_color);
                            content.set_fill_rgb(sr, sg, sb);
                            content.rect(
                                *x,
                                h - (*y + run.size_pt * 0.2),
                                run.width_pt,
                                run.size_pt,
                            );
                            content.fill_nonzero();
                        }
                        // 그림자 — 본문 전에 오프셋 복사.
                        if let Some(sc) = run.shadow {
                            let (dx, dy) = run.shadow_offset();
                            write_glyph_run(
                                &mut content,
                                &fonts,
                                &glyph_fonts,
                                *x,
                                *y,
                                h,
                                run,
                                sc,
                                dx,
                                dy,
                            );
                        }
                        // 양각/음각 — 흰 하이라이트 사본 오프셋(양각=좌상, 음각=우하).
                        if run.emboss || run.engrave {
                            let d = run.size_pt * 0.05 * if run.emboss { -1.0 } else { 1.0 };
                            write_glyph_run(
                                &mut content,
                                &fonts,
                                &glyph_fonts,
                                *x,
                                *y,
                                h,
                                run,
                                0x00FF_FFFF,
                                d,
                                d,
                            );
                        }
                        write_glyph_run(
                            &mut content,
                            &fonts,
                            &glyph_fonts,
                            *x,
                            *y,
                            h,
                            run,
                            run.color,
                            0.0,
                            0.0,
                        );
                    }
                }
                Item::Path {
                    commands,
                    fill,
                    stroke,
                } => {
                    let dashed = stroke.as_ref().is_some_and(|s| s.dash.len() >= 2);
                    // 그러데이션 채움: 경로로 클립한 뒤 색 띠/원으로 채운다(실제 그러데이션).
                    if let Some(Fill::Gradient(grad)) = fill {
                        content.save_state();
                        pdf_emit_path(&mut content, commands, h);
                        content.clip_nonzero();
                        content.end_path();
                        pdf_gradient_bands(&mut content, grad, commands, h);
                        content.restore_state();
                        // 테두리(선)는 별도로 다시 그린다.
                        if let Some(s) = stroke {
                            apply_stroke(&mut content, s);
                            pdf_emit_path(&mut content, commands, h);
                            content.stroke();
                        }
                    } else {
                        pdf_emit_path(&mut content, commands, h);
                        if let Some(s) = stroke {
                            apply_stroke(&mut content, s);
                        }
                        let solid = match fill {
                            Some(Fill::Solid(c)) => Some(*c),
                            _ => None,
                        };
                        match (solid, stroke) {
                            (Some(fc), Some(_)) => {
                                let (r, g, b) = colorref_rgb(fc);
                                content.set_fill_rgb(r, g, b);
                                content.fill_nonzero_and_stroke();
                            }
                            (Some(fc), None) => {
                                let (r, g, b) = colorref_rgb(fc);
                                content.set_fill_rgb(r, g, b);
                                content.fill_nonzero();
                            }
                            (None, Some(_)) => {
                                content.stroke();
                            }
                            // 채움·선 없음: 경로를 칠하지 않고 비운다(n) — 누적 방지.
                            (None, None) => {
                                content.end_path();
                            }
                        }
                    }
                    // 점선 상태가 이후 항목(표 테두리 등)으로 새지 않도록 실선 복원.
                    if dashed {
                        content.set_dash_pattern([], 0.0);
                    }
                }
            }
        }

        plans.push(PagePlan {
            page_id,
            content_id,
            w,
            h,
            content: content.finish().into_vec(),
            images,
        });
    }

    // ── 4. PDF 작성 ──
    let mut pdf = Pdf::new();
    pdf.set_version(1, 4);
    {
        let mut cat = pdf.catalog(catalog_id);
        cat.pages(page_tree_id);
        // Hancom PDF document-level surface (design 21 §2).
        cat.lang(TextStr("ko-KR"));
        cat.page_layout(PageLayout::SinglePage);
        cat.mark_info().marked(false);
        cat.metadata(xmp_id);
        {
            let mut intents = cat.output_intents();
            {
                let mut oi = intents.push();
                oi.subtype(OutputIntentSubtype::PDFA);
                oi.output_condition_identifier(TextStr("sRGB2014"));
                oi.info(TextStr("sRGB2014"));
                oi.dest_output_profile(icc_id);
            }
            intents.finish();
        }
        cat.finish();
    }
    // /Info is limited to the six Hancom keys. Author and dates are conditional.
    {
        let mut info = pdf.document_info(info_id);
        if let Some(author) = &meta.author {
            info.author(TextStr(author));
        }
        info.creator(TextStr(PRODUCER));
        info.producer(TextStr(PRODUCER));
        if let Some(d) = meta.create_time.and_then(filetime_to_date) {
            info.creation_date(d);
        }
        if let Some(d) = meta.modify_time.and_then(filetime_to_date) {
            info.modified_date(d);
        }
        info.pair(Name(b"PDFVersion"), TextStr("1.4"));
        info.finish();
    }
    {
        let xmp = xmp_packet(meta);
        pdf.metadata(xmp_id, xmp.as_bytes()).finish();
    }
    {
        let mut icc = pdf.icc_profile(icc_id, srgb_icc());
        icc.n(3);
        icc.alternate().device_rgb();
        icc.finish();
    }
    {
        let kids: Vec<Ref> = plans.iter().map(|p| p.page_id).collect();
        pdf.pages(page_tree_id)
            .kids(kids.iter().copied())
            .count(kids.len() as i32);
    }

    for f in &fonts {
        write_font(&mut pdf, f)?;
    }
    for plan in &plans {
        write_page(&mut pdf, plan, page_tree_id, &fonts);
    }

    Ok(pdf.finish())
}

/// Return the text sequence that the PDF backend is expected to emit through
/// text-showing operators. This is intentionally derived before ToUnicode
/// serialization so corpus validation can detect an incorrect CMap mapping.
///
/// Decorative shadow and emboss/engrave passes emit the same glyphs again, so
/// their repetitions are included to match the content stream exactly.
pub(crate) fn expected_text_trace(list: &DisplayList) -> Result<String, RenderError> {
    let mut trace = String::new();
    for (page_index, page) in list.pages.iter().enumerate() {
        if page_index > 0 {
            trace.push('\n');
        }
        for item in &page.items {
            let Item::Glyphs { run, .. } = item else {
                continue;
            };
            let emitted_text = glyph_source_sequences(run).concat();
            if run.shadow.is_some() {
                trace.push_str(&emitted_text);
            }
            if run.emboss || run.engrave {
                trace.push_str(&emitted_text);
            }
            trace.push_str(&emitted_text);
        }
    }
    Ok(trace)
}

/// Deterministic /Info Producer and Creator value, including the package version.
const PRODUCER: &str = concat!("hwp-cli ", env!("CARGO_PKG_VERSION"));

/// ICC-maintained v2 sRGB profile embedded in /OutputIntents.
///
/// The committed representation is hexadecimal text so its license and provenance remain easy to
/// audit. Decoding happens once per process. See the adjacent LICENSE file and design 21 §2.
fn srgb_icc() -> &'static [u8] {
    static PROFILE: OnceLock<Vec<u8>> = OnceLock::new();
    PROFILE
        .get_or_init(|| {
            let digits: Vec<u8> = include_str!("../assets/sRGB2014.icc.hex")
                .bytes()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            assert!(digits.len().is_multiple_of(2), "invalid embedded ICC hex");
            digits
                .chunks_exact(2)
                .map(|pair| {
                    let hi = (pair[0] as char)
                        .to_digit(16)
                        .expect("invalid embedded ICC hex");
                    let lo = (pair[1] as char)
                        .to_digit(16)
                        .expect("invalid embedded ICC hex");
                    ((hi << 4) | lo) as u8
                })
                .collect()
        })
        .as_slice()
}

/// Convert a FILETIME value to a UTC PDF date.
fn filetime_to_date(ft: u64) -> Option<Date> {
    let (y, mo, d, h, mi, sec) = filetime_to_utc(ft)?;
    Some(
        Date::new(y as u16)
            .month(mo)
            .day(d)
            .hour(h)
            .minute(mi)
            .second(sec)
            .utc_offset_hour(0),
    )
}

/// Convert FILETIME to UTC components through the model's canonical conversion.
fn filetime_to_utc(ft: u64) -> Option<(i32, u8, u8, u8, u8, u8)> {
    let iso = supported_iso8601_utc(ft)?;
    Some((
        iso.get(0..4)?.parse().ok()?,
        iso.get(5..7)?.parse().ok()?,
        iso.get(8..10)?.parse().ok()?,
        iso.get(11..13)?.parse().ok()?,
        iso.get(14..16)?.parse().ok()?,
        iso.get(17..19)?.parse().ok()?,
    ))
}

/// Keep PDF /Info and XMP on the same four-digit year domain.
fn supported_iso8601_utc(ft: u64) -> Option<String> {
    let iso = filetime_to_iso8601_utc(ft)?;
    if iso.len() != 20 || !iso.as_bytes().get(0..4)?.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let year: u16 = iso.get(0..4)?.parse().ok()?;
    (year > 0).then_some(iso)
}

/// Build the minimal dc/pdf/xmp metadata packet.
fn xmp_packet(meta: &Metadata) -> String {
    let mut s = String::new();
    s.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    s.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    s.push_str("<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    s.push_str("<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n");
    if let Some(t) = &meta.title {
        s.push_str("<dc:title><rdf:Alt><rdf:li xml:lang=\"ko-KR\">");
        s.push_str(&xml_escape(t));
        s.push_str("</rdf:li></rdf:Alt></dc:title>\n");
    }
    if let Some(a) = &meta.author {
        s.push_str("<dc:creator><rdf:Seq><rdf:li>");
        s.push_str(&xml_escape(a));
        s.push_str("</rdf:li></rdf:Seq></dc:creator>\n");
    }
    s.push_str("<pdf:Producer>");
    s.push_str(PRODUCER);
    s.push_str("</pdf:Producer>\n");
    if let Some(d) = meta.create_time.and_then(supported_iso8601_utc) {
        s.push_str("<xmp:CreateDate>");
        s.push_str(&d);
        s.push_str("</xmp:CreateDate>\n");
    }
    if let Some(d) = meta.modify_time.and_then(supported_iso8601_utc) {
        s.push_str("<xmp:ModifyDate>");
        s.push_str(&d);
        s.push_str("</xmp:ModifyDate>\n");
    }
    s.push_str("</rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
    s
}

/// Escape an XML text node and replace characters forbidden by XML 1.0.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{9}'
            | '\u{a}'
            | '\u{d}'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => out.push(ch),
            _ => out.push('\u{fffd}'),
        }
    }
    out
}

/// 폰트 1개의 PDF 객체(FontFile·Descriptor·CIDFont·Type0·ToUnicode)를 쓴다.
fn write_font(pdf: &mut Pdf, f: &FontInfo) -> Result<(), RenderError> {
    let face = ttf_parser::Face::parse(&f.data, f.index)
        .map_err(|e| RenderError::Pdf(format!("폰트 파싱 실패: {e:?}")))?;
    let upem = face.units_per_em() as f32;
    let s = 1000.0 / upem; // 폰트 단위 → PDF 1000-em 글리프 공간
    let is_cff = face.tables().cff.is_some();

    // FontFile 스트림 (서브셋 바이트, FlateDecode)
    {
        let z = zlib(&f.subset_bytes);
        let mut st = pdf.stream(f.ff_id, &z);
        st.filter(Filter::FlateDecode);
        if is_cff {
            st.pair(Name(b"Subtype"), Name(b"OpenType"));
        } else {
            st.pair(Name(b"Length1"), f.subset_bytes.len() as i32);
        }
        st.finish();
    }

    // FontDescriptor
    let bbox = face.global_bounding_box();
    {
        let mut d = pdf.font_descriptor(f.desc_id);
        d.name(Name(f.base_font.as_bytes()));
        d.flags(FontFlags::SYMBOLIC);
        d.bbox(Rect::new(
            bbox.x_min as f32 * s,
            bbox.y_min as f32 * s,
            bbox.x_max as f32 * s,
            bbox.y_max as f32 * s,
        ));
        d.italic_angle(0.0);
        d.ascent(face.ascender() as f32 * s);
        d.descent(face.descender() as f32 * s);
        d.cap_height(face.capital_height().unwrap_or_else(|| face.ascender()) as f32 * s);
        d.stem_v(80.0);
        if is_cff {
            d.font_file3(f.ff_id);
        } else {
            d.font_file2(f.ff_id);
        }
        d.finish();
    }

    // CIDFont (+ W 폭 배열, 출력 글리프 ID 순서)
    {
        let widths: Vec<f32> = if f.subset_ok {
            f.remapper
                .remapped_gids()
                .map(|old| glyph_width(&face, old) * s)
                .collect()
        } else {
            (0..face.number_of_glyphs())
                .map(|old| glyph_width(&face, old) * s)
                .collect()
        };
        let mut cid = pdf.cid_font(f.cid_id);
        cid.subtype(if is_cff {
            CidFontType::Type0
        } else {
            CidFontType::Type2
        });
        cid.base_font(Name(f.base_font.as_bytes()));
        cid.system_info(SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        });
        cid.font_descriptor(f.desc_id);
        if !is_cff {
            cid.cid_to_gid_map_predefined(Name(b"Identity"));
        }
        cid.default_width(1000.0);
        cid.widths().consecutive(0, widths);
        cid.finish();
    }

    // Type0 (composite) 폰트
    {
        let mut t = pdf.type0_font(f.type0_id);
        t.base_font(Name(f.base_font.as_bytes()));
        t.encoding_predefined(Name(b"Identity-H"));
        t.descendant_font(f.cid_id);
        t.to_unicode(f.tounicode_id);
        t.finish();
    }

    // ToUnicode CMap (검색·복사용)
    {
        let mut entries: Vec<(u16, &str)> = f
            .to_unicode
            .iter()
            .map(|(&glyph, source)| (glyph, source.as_str()))
            .collect();
        entries.sort_unstable_by_key(|e| e.0);
        let mut cmap = UnicodeCmap::new(
            Name(b"Adobe-Identity-UCS"),
            SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"UCS"),
                supplement: 0,
            },
        );
        for (glyph, source) in entries {
            cmap.pair_with_multiple(glyph, source.chars());
        }
        let buf = cmap.finish();
        pdf.cmap(f.tounicode_id, &buf);
    }

    Ok(())
}

/// 페이지 1개의 이미지 XObject·콘텐츠 스트림·페이지 dict를 쓴다.
fn write_page(pdf: &mut Pdf, plan: &PagePlan, page_tree_id: Ref, fonts: &[FontInfo]) {
    for img in &plan.images {
        match &img.payload {
            ImagePayload::Jpeg { bytes, w, h, gray } => {
                let mut x = pdf.image_xobject(img.id, bytes);
                x.filter(Filter::DctDecode);
                x.width(*w);
                x.height(*h);
                if *gray {
                    x.color_space().device_gray();
                } else {
                    x.color_space().device_rgb();
                }
                x.bits_per_component(8);
                x.finish();
            }
            ImagePayload::Raw {
                rgb_z,
                alpha_z,
                w,
                h,
            } => {
                {
                    let mut x = pdf.image_xobject(img.id, rgb_z);
                    x.filter(Filter::FlateDecode);
                    x.width(*w);
                    x.height(*h);
                    x.color_space().device_rgb();
                    x.bits_per_component(8);
                    if let Some(sid) = img.smask_id {
                        x.s_mask(sid);
                    }
                    x.finish();
                }
                if let (Some(sid), Some(az)) = (img.smask_id, alpha_z) {
                    let mut sm = pdf.image_xobject(sid, az);
                    sm.filter(Filter::FlateDecode);
                    sm.width(*w);
                    sm.height(*h);
                    sm.color_space().device_gray();
                    sm.bits_per_component(8);
                    sm.finish();
                }
            }
        }
    }

    {
        let z = zlib(&plan.content);
        pdf.stream(plan.content_id, &z).filter(Filter::FlateDecode);
    }

    {
        let mut p = pdf.page(plan.page_id);
        p.parent(page_tree_id);
        p.media_box(Rect::new(0.0, 0.0, plan.w, plan.h));
        p.contents(plan.content_id);
        let mut res = p.resources();
        {
            let mut fd = res.fonts();
            for f in fonts {
                fd.pair(Name(f.res_name.as_bytes()), f.type0_id);
            }
            fd.finish();
        }
        if !plan.images.is_empty() {
            let mut xo = res.x_objects();
            for img in &plan.images {
                xo.pair(Name(img.name.as_bytes()), img.id);
            }
            xo.finish();
        }
        res.finish();
        p.finish();
    }
}

/// 글리프 런을 텍스트 객체로 그린다. 각 글리프를 셰이핑 좌표에 명시 배치해
/// png/svg 백엔드와 위치를 일치시킨다.
/// 글리프 런을 PDF 텍스트로 그린다. color로 채우고 (dx, dy)만큼 평행이동(그림자용).
#[allow(clippy::too_many_arguments)]
fn write_glyph_run(
    content: &mut Content,
    fonts: &[FontInfo],
    glyph_fonts: &[usize],
    x: f32,
    y: f32,
    page_h: f32,
    run: &ShapedRun,
    color: u32,
    dx: f32,
    dy: f32,
) {
    content.begin_text();
    content.set_horizontal_scaling(run.x_scale * 100.0); // 장평(Tz)
    let (r, g, b) = colorref_rgb(color);
    content.set_fill_rgb(r, g, b);
    if run.outline {
        // 외곽선 = 윤곽선만(채움 없음).
        content.set_text_rendering_mode(TextRenderingMode::Stroke);
        content.set_stroke_rgb(r, g, b);
        content.set_line_width(run.size_pt * 0.025);
    } else if run.bold {
        // 합성 굵게 = 채움+스트로크.
        content.set_text_rendering_mode(TextRenderingMode::FillStroke);
        content.set_stroke_rgb(r, g, b);
        content.set_line_width(run.size_pt * BOLD_STROKE);
    } else {
        content.set_text_rendering_mode(TextRenderingMode::Fill);
    }
    let shear = if run.italic { ITALIC_SKEW } else { 0.0 };

    let mut pen_x = x + dx;
    for (gl, &font_index) in run.glyphs.iter().zip(glyph_fonts) {
        let f = &fonts[font_index];
        content.set_font(Name(f.res_name.as_bytes()), run.size_pt);
        let gid = out_gid(f.subset_ok, &f.remapper, gl.id);
        // Tm: 크기·장평은 Tf/Tz가 적용, 여기선 기울임 시어(c)·베이스라인 이동만.
        // y 뒤집기: PDF는 y-위 → page_h - (y - y_offset). 그림자는 dy만큼 더 내린다.
        content.set_text_matrix([
            1.0,
            0.0,
            shear,
            1.0,
            pen_x + gl.x_offset,
            page_h - (y - gl.y_offset) - dy,
        ]);
        let code = gid.to_be_bytes();
        content.show(Str(&code));
        pen_x += gl.x_advance; // png.rs:154 / svg.rs:103과 동일 누적
    }
    content.end_text();
}

/// 출력 글리프 ID: 서브셋 성공 시 재매핑 ID, 실패 시 원본 ID.
fn out_gid(subset_ok: bool, remapper: &GlyphRemapper, orig: u16) -> u16 {
    if subset_ok {
        remapper.get(orig).unwrap_or(0)
    } else {
        orig
    }
}

fn glyph_width(face: &ttf_parser::Face<'_>, gid: u16) -> f32 {
    face.glyph_hor_advance(ttf_parser::GlyphId(gid))
        .unwrap_or(0) as f32
}

fn font_key(font: &Arc<LoadedFont>) -> (usize, u32) {
    (font.data.as_ptr() as usize, font.index)
}

fn alloc(counter: &mut i32) -> Ref {
    let r = Ref::new(*counter);
    *counter += 1;
    r
}

/// 서브셋 폰트용 6글자 대문자 태그 ("AAAAAA", "BAAAAA" …).
fn subset_tag(mut i: usize) -> String {
    let mut s = String::with_capacity(6);
    for _ in 0..6 {
        s.push((b'A' + (i % 26) as u8) as char);
        i /= 26;
    }
    s
}

/// 선 스타일(색·굵기·점선)을 콘텐츠 상태에 적용한다.
fn apply_stroke(content: &mut Content, s: &Stroke) {
    let (r, g, b) = colorref_rgb(s.color);
    content.set_stroke_rgb(r, g, b);
    content.set_line_width(s.width.max(0.1));
    if s.dash.len() >= 2 {
        content.set_dash_pattern(s.dash.iter().copied(), 0.0);
    }
}

/// 경로 명령을 PDF 콘텐츠로(y 뒤집기 h-y).
fn pdf_emit_path(content: &mut Content, cmds: &[PathCmd], h: f32) {
    for cmd in cmds {
        match *cmd {
            PathCmd::MoveTo(x, y) => {
                content.move_to(x, h - y);
            }
            PathCmd::LineTo(x, y) => {
                content.line_to(x, h - y);
            }
            PathCmd::CubicTo(a, b, c, e, f, g) => {
                content.cubic_to(a, h - b, c, h - e, f, h - g);
            }
            PathCmd::Close => {
                content.close_path();
            }
        }
    }
}

/// 클립된 영역에 색 띠(선형)/동심원(방사형)으로 그러데이션을 그린다. (PDF 셰이딩 대체 근사)
fn pdf_gradient_bands(content: &mut Content, g: &Gradient, cmds: &[PathCmd], h: f32) {
    const N: usize = 48;
    let (x0, y0, x1, y1) = path_bbox(cmds);
    let set = |content: &mut Content, t: f32| {
        let (r, gg, b) = g.color_at(t);
        content.set_fill_rgb(r as f32 / 255.0, gg as f32 / 255.0, b as f32 / 255.0);
    };
    if g.radial {
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let rmax = ((x1 - x0).max(y1 - y0) / 2.0 * 1.05).max(0.1);
        // 가장자리(t=1) → 중심(t=0): 큰 원부터 그려 작은 원이 위에.
        for i in 0..N {
            let t = 1.0 - i as f32 / (N - 1) as f32;
            set(content, t);
            pdf_circle(content, cx, h - cy, (rmax * t).max(0.02));
            content.fill_nonzero();
        }
    } else {
        let a = g.angle_deg.to_radians();
        let horizontal = a.cos().abs() >= a.sin().abs();
        for i in 0..N {
            let t = i as f32 / (N - 1) as f32;
            set(content, t);
            if horizontal {
                let bx = x0 + (x1 - x0) * t;
                content.rect(bx, h - y1, (x1 - x0) / N as f32 + 0.5, y1 - y0);
            } else {
                let by = y0 + (y1 - y0) * t;
                let bh = (y1 - y0) / N as f32 + 0.5;
                content.rect(x0, h - (by + bh), x1 - x0, bh);
            }
            content.fill_nonzero();
        }
    }
}

/// 4개 큐빅으로 원(중심 cx,cy 반지름 r) 경로를 만든다 (PDF 좌표 그대로).
fn pdf_circle(content: &mut Content, cx: f32, cy: f32, r: f32) {
    let k = 0.552_285 * r;
    content.move_to(cx + r, cy);
    content.cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r);
    content.cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy);
    content.cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r);
    content.cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy);
    content.close_path();
}

/// COLORREF(0x00BBGGRR) → (r, g, b) 0..1. 없음(0xFFFFFFFF)은 검정 (png/svg 규칙).
fn colorref_rgb(c: u32) -> (f32, f32, f32) {
    if c == 0xFFFF_FFFF {
        return (0.0, 0.0, 0.0);
    }
    (
        (c & 0xFF) as f32 / 255.0,
        ((c >> 8) & 0xFF) as f32 / 255.0,
        ((c >> 16) & 0xFF) as f32 / 255.0,
    )
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// 임베드할 폰트 정보 (고유 폰트 1개).
struct FontInfo {
    data: Arc<Vec<u8>>,
    index: u32,
    remapper: GlyphRemapper,
    /// Original GID to source Unicode sequence for this PDF font variant.
    orig_to_unicode: HashMap<u16, String>,
    subset_ok: bool,
    subset_bytes: Vec<u8>,
    /// Output GID to source Unicode sequence for the ToUnicode CMap.
    to_unicode: HashMap<u16, String>,
    /// 페이지 리소스 키 ("F0" …).
    res_name: String,
    /// /BaseFont 값 (서브셋이면 "ABCDEF+F0").
    base_font: String,
    type0_id: Ref,
    cid_id: Ref,
    desc_id: Ref,
    ff_id: Ref,
    tounicode_id: Ref,
}

impl FontInfo {
    fn new(font: Arc<LoadedFont>) -> Self {
        Self {
            data: font.data.clone(),
            index: font.index,
            remapper: GlyphRemapper::new(),
            orig_to_unicode: HashMap::new(),
            subset_ok: false,
            subset_bytes: Vec::new(),
            to_unicode: HashMap::new(),
            res_name: String::new(),
            base_font: String::new(),
            type0_id: Ref::new(1),
            cid_id: Ref::new(1),
            desc_id: Ref::new(1),
            ff_id: Ref::new(1),
            tounicode_id: Ref::new(1),
        }
    }
}

type FontKey = (usize, u32);
type GlyphFontKey = (FontKey, u16, String);

fn collect_font_variants(
    list: &DisplayList,
) -> Result<(Vec<FontInfo>, HashMap<GlyphFontKey, usize>), RenderError> {
    let mut fonts = Vec::<FontInfo>::new();
    let mut variants = HashMap::<FontKey, Vec<usize>>::new();
    let mut lookup = HashMap::<GlyphFontKey, usize>::new();

    for page in &list.pages {
        for item in &page.items {
            let Item::Glyphs { run, .. } = item else {
                continue;
            };
            let font_key = font_key(&run.font);
            for (glyph, source) in run.glyphs.iter().zip(glyph_source_sequences(run)) {
                let key = (font_key, glyph.id, source.clone());
                if lookup.contains_key(&key) {
                    continue;
                }
                let compatible = variants.get(&font_key).and_then(|indices| {
                    indices.iter().copied().find(|&index| {
                        fonts[index]
                            .orig_to_unicode
                            .get(&glyph.id)
                            .is_none_or(|existing| existing == &source)
                    })
                });
                let index = if let Some(index) = compatible {
                    index
                } else {
                    if fonts.len() >= crate::fonts::MAX_FONT_RESOLUTIONS {
                        return Err(RenderError::Pdf(
                            "PDF font variant limit exceeded".to_string(),
                        ));
                    }
                    fonts.push(FontInfo::new(run.font.clone()));
                    let index = fonts.len() - 1;
                    variants.entry(font_key).or_default().push(index);
                    index
                };
                fonts[index].remapper.remap(glyph.id);
                fonts[index]
                    .orig_to_unicode
                    .insert(glyph.id, source.clone());
                lookup.insert(key, index);
            }
        }
    }
    Ok((fonts, lookup))
}

struct PagePlan {
    page_id: Ref,
    content_id: Ref,
    w: f32,
    h: f32,
    content: Vec<u8>,
    images: Vec<ImagePlan>,
}

struct ImagePlan {
    id: Ref,
    smask_id: Option<Ref>,
    name: String,
    payload: ImagePayload,
}

enum ImagePayload {
    /// JPEG 원본 — DCTDecode로 그대로 임베드.
    Jpeg {
        bytes: Arc<Vec<u8>>,
        w: i32,
        h: i32,
        gray: bool,
    },
    /// 디코드된 RGB(+선택적 알파 SMask), FlateDecode.
    Raw {
        rgb_z: Vec<u8>,
        alpha_z: Option<Vec<u8>>,
        w: i32,
        h: i32,
    },
}

/// 인코딩 이미지 바이트를 PDF 임베드용 페이로드로 디코드한다.
fn decode_image(data: &Arc<Vec<u8>>) -> Option<ImagePayload> {
    // JPEG 빠른 경로: 회색/RGB는 원본을 DCTDecode로 그대로. (CMYK·파싱 실패는 디코드 경로로)
    if data.len() >= 2
        && data[0] == 0xFF
        && data[1] == 0xD8
        && let Some((w, h, comps)) = jpeg_info(data)
        && (comps == 1 || comps == 3)
    {
        return Some(ImagePayload::Jpeg {
            bytes: data.clone(),
            w: w as i32,
            h: h as i32,
            gray: comps == 1,
        });
    }

    let dynamic = image::load_from_memory(data).ok()?;
    let rgba = dynamic.to_rgba8();
    let (w, h) = rgba.dimensions();
    let rgb: Vec<u8> = rgba.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let alpha: Option<Vec<u8>> = dynamic
        .color()
        .has_alpha()
        .then(|| rgba.pixels().map(|p| p[3]).collect());
    Some(ImagePayload::Raw {
        rgb_z: zlib(&rgb),
        alpha_z: alpha.as_deref().map(zlib),
        w: w as i32,
        h: h as i32,
    })
}

/// JPEG SOF 마커에서 (가로, 세로, 성분 수)를 읽는다.
fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8)> {
    let mut i = 2; // SOI(FFD8) 건너뜀
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // 길이 없는 standalone 마커.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = ((data[i + 2] as usize) << 8) | data[i + 3] as usize;
        // SOF: C0–CF (단 C4=DHT, C8=JPG, CC=DAC 제외).
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            let h = ((data[i + 5] as u32) << 8) | data[i + 6] as u32;
            let w = ((data[i + 7] as u32) << 8) | data[i + 8] as u32;
            let comps = data[i + 9];
            return Some((w, h, comps));
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_run(text: &str, glyph_ids: &[u16]) -> ShapedRun {
        let glyphs: Vec<crate::shape::Glyph> = glyph_ids
            .iter()
            .copied()
            .map(|id| crate::shape::Glyph {
                id,
                x_advance: 10.0,
                x_offset: 0.0,
                y_offset: 0.0,
            })
            .collect();
        ShapedRun {
            font: Arc::new(LoadedFont {
                data: Arc::new(Vec::new()),
                index: 0,
                family: String::new(),
            }),
            size_pt: 10.0,
            x_scale: 1.0,
            color: 0,
            bold: false,
            italic: false,
            underline_kind: 0,
            underline_shape: 0,
            strike: false,
            strike_shape: 0,
            emphasis: 0,
            underline_color: 0xFFFF_FFFF,
            shade_color: 0xFFFF_FFFF,
            shadow: None,
            shadow_gap: (0, 0),
            outline: false,
            emboss: false,
            engrave: false,
            border_fill_id: 0,
            width_pt: glyphs.iter().map(|glyph| glyph.x_advance).sum(),
            glyphs,
            text: text.to_string(),
            start_wchar: 0,
        }
    }

    fn synthetic_run_with_font(font: Arc<LoadedFont>, text: &str, glyph_ids: &[u16]) -> ShapedRun {
        let mut run = synthetic_run(text, glyph_ids);
        run.font = font;
        run
    }

    fn empty_font() -> Arc<LoadedFont> {
        Arc::new(LoadedFont {
            data: Arc::new(Vec::new()),
            index: 0,
            family: String::new(),
        })
    }

    fn one_run_list(run: ShapedRun) -> DisplayList {
        DisplayList {
            pages: vec![crate::display::PageList {
                width_pt: 200.0,
                height_pt: 100.0,
                items: vec![Item::Glyphs {
                    x: 10.0,
                    y: 30.0,
                    run,
                }],
            }],
        }
    }

    #[test]
    fn filetime_변환() {
        // 1970-01-01T00:00:00Z
        assert_eq!(
            filetime_to_utc(11_644_473_600 * 10_000_000),
            Some((1970, 1, 1, 0, 0, 0))
        );
        // 2024-01-02T03:04:05Z
        assert_eq!(
            filetime_to_utc(133_486_382_450_000_000),
            Some((2024, 1, 2, 3, 4, 5))
        );
        // Leap-day boundary: 2000-02-29T12:00:00Z.
        assert_eq!(
            filetime_to_utc(125_962_992_000_000_000),
            Some((2000, 2, 29, 12, 0, 0))
        );
        // Zero means unspecified in the IR; valid pre-Unix FILETIME values are preserved.
        assert_eq!(filetime_to_utc(0), None);
        let pre_unix = hwp_model::iso8601_utc_to_filetime("1900-03-01T00:00:00Z").unwrap();
        assert_eq!(filetime_to_utc(pre_unix), Some((1900, 3, 1, 0, 0, 0)));
        assert_eq!(supported_iso8601_utc(u64::MAX), None);
    }

    #[test]
    fn xmp_패킷_필드() {
        let mut meta = Metadata::default();
        let empty = xmp_packet(&meta);
        assert!(empty.contains("<pdf:Producer>hwp-cli "));
        assert!(!empty.contains("dc:title"));
        assert!(!empty.contains("xmp:CreateDate"));

        meta.title = Some("보고서 & <초안> 😀".to_string());
        meta.author = Some("홍길동\u{0}".to_string());
        meta.create_time = Some(133_486_382_450_000_000);
        let xmp = xmp_packet(&meta);
        assert!(xmp.contains(">보고서 &amp; &lt;초안&gt; 😀<"));
        assert!(xmp.contains("<rdf:li>홍길동\u{fffd}</rdf:li>"));
        assert!(xmp.contains("<xmp:CreateDate>2024-01-02T03:04:05Z</xmp:CreateDate>"));
        assert!(!xmp.contains("xmp:ModifyDate"));
    }

    #[test]
    fn tounicode_preserves_ligatures_and_gid_aliases() {
        let ligature_list = one_run_list(synthetic_run("ffi", &[42]));
        let (fonts, _) = collect_font_variants(&ligature_list).unwrap();
        assert!(
            fonts
                .iter()
                .any(|font| { font.orig_to_unicode.values().any(|source| source == "ffi") })
        );

        let font = empty_font();
        let alias_run = synthetic_run_with_font(font, ";\u{37e}", &[28, 28]);
        assert_eq!(alias_run.glyphs.len(), 2);
        assert_eq!(alias_run.glyphs[0].id, alias_run.glyphs[1].id);
        let alias_list = one_run_list(alias_run);
        let (fonts, lookup) = collect_font_variants(&alias_list).unwrap();
        let entries: Vec<_> = lookup.values().copied().collect();
        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0], entries[1]);
        assert_eq!(fonts.len(), 2);
    }
}
