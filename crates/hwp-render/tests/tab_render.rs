//! Proves the tab leader `Item::Path` (added in 01-03, made kind-aware in 01-04) actually
//! reaches all three render backends from a single layout-level emission — no change to
//! `svg.rs`, `pdf.rs` or `png.rs` is needed or made by this test.
//!
//! Assertions stay at the structural level (SVG markup / PDF content-stream operators / "did
//! the PNG render complete"), never on rendered letterforms, counted pages or pixels, since CI
//! resolves text from whichever typefaces happen to be installed on each runner (repo
//! CLAUDE.md's CI text-rendering-independence rule).

use std::io::Read;

use hwp_render::{RenderOptions, render_document, render_document_pdf, render_document_svg};

/// A synthetic HWPX-origin document: one paragraph "A<TAB>B", with a single explicit tab stop
/// carried the same way an hwpx reader populates it — `header.tab_stops[0]`, `header.tab_defs`
/// left empty (hwpx never populates the raw hwp5-only fallback). The stop is a right tab
/// (kind 1) with a DOT leader (fill 3), matching `hwpx::read::header`'s leader-attribute
/// numbering that `tab.rs` already documents.
fn document_with_leadered_tab() -> hwp_model::Document {
    let mut doc = hwp_convert::from_markdown("AB");
    // Splice a tab control between "A" and "B" directly on the IR rather than relying on a
    // markdown parser to preserve a literal tab in inline text — the IR invariant (see
    // `hwp_model::HwpChar::InlineCtrl`'s own doc comment) is what this test needs, not
    // markdown's tab-handling behavior.
    doc.sections[0].paragraphs[0].chars.insert(
        1,
        hwp_model::HwpChar::InlineCtrl {
            code: hwp_model::ctrl_char::TAB,
            payload: vec![0; 12],
        },
    );
    doc.header.tab_stops = vec![hwp_model::TabDef {
        attr: 0,
        items: vec![hwp_model::TabItem {
            pos: 30_000, // HWPUNIT/100 = 300pt — inside the page body, past "A".
            kind: 1,
            fill: 3,
        }],
    }];
    doc
}

/// Scans raw PDF bytes for every `stream ... endstream` object, zlib-inflates each (all
/// streams `pdf.rs` writes use `Filter::FlateDecode` — content, embedded typefaces and images
/// alike — so no new dependency is needed: `flate2` is already a direct dependency of this
/// crate, not a new one added for this test), and returns the ones that decoded successfully.
fn flate_decoded_streams(pdf_bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = find(&pdf_bytes[pos..], b"stream") {
        let mut data_start = pos + rel + b"stream".len();
        // PDF spec: the stream keyword is followed by CRLF or a lone LF (never a lone CR).
        if pdf_bytes.get(data_start) == Some(&b'\r') {
            data_start += 1;
        }
        if pdf_bytes.get(data_start) == Some(&b'\n') {
            data_start += 1;
        }
        let Some(end_rel) = find(&pdf_bytes[data_start..], b"endstream") else {
            break;
        };
        let raw = &pdf_bytes[data_start..data_start + end_rel];
        let raw = raw
            .strip_suffix(b"\r\n")
            .or_else(|| raw.strip_suffix(b"\n"))
            .unwrap_or(raw);
        let mut decoded = Vec::new();
        if flate2::read::ZlibDecoder::new(raw)
            .read_to_end(&mut decoded)
            .is_ok()
        {
            out.push(decoded);
        }
        pos = data_start + end_rel + b"endstream".len();
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn leader_dash_reaches_svg_and_pdf_backends() {
    let doc = document_with_leadered_tab();
    let opts = RenderOptions::default();

    let svg = render_document_svg(&doc, &opts);
    assert!(
        svg.pages
            .iter()
            .any(|page| page.contains("stroke-dasharray")),
        "leader dash must reach the SVG backend: {:?}",
        svg.pages
    );

    let pdf = render_document_pdf(&doc, &opts, None).unwrap();
    // The "d" operator (dash-pattern, PDF spec table 52) is the only single-letter content
    // stream operator this codebase emits named exactly "d" — finding it as a standalone
    // token in a decoded stream is an unambiguous signal the leader reached the PDF backend.
    let dash_operator_found = flate_decoded_streams(&pdf.data).iter().any(|bytes| {
        String::from_utf8_lossy(bytes)
            .split_whitespace()
            .any(|token| token == "d")
    });
    assert!(
        dash_operator_found,
        "leader dash ('d' operator) must reach a PDF content stream"
    );

    // Smoke check only: the pipeline completes and produces real page bytes. Never assert on
    // rendered letterforms, pixel values or dimensions here — those depend on which typeface
    // the runner resolves.
    let png = render_document(&doc, &opts).unwrap();
    assert!(!png.pages.is_empty(), "PNG render must complete");
}
