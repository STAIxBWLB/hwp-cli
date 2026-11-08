//! Top-level document model.

use serde::{Deserialize, Serialize};

use crate::control::{BinRef, Control, SectionDef};
use crate::header::DocHeader;
use crate::paragraph::Paragraph;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Source information, including original format and version.
    pub meta: DocMeta,
    /// Document properties (title/author/subject/keywords): HWP5 `\x05HwpSummaryInformation`,
    /// and HWPX `Contents/content.hpf` (OPF dc:*).
    #[serde(default)]
    pub metadata: Metadata,
    pub header: DocHeader,
    pub sections: Vec<Section>,
    /// Attached binaries, such as images, keyed by their original container entry names.
    /// (hwp5: "BIN0001.png", hwpx: "BinData/image1.png").
    pub bin_streams: Vec<BinStream>,
    /// Original HWPX `settings.xml` (application settings and caret position), passed through
    /// unchanged when rewriting through the IR. HWP5 inputs use `None` and writer defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hwpx_settings_xml: Option<String>,
    /// Original HWPX `version.xml` (version metadata), passed through unchanged when rewriting
    /// through the IR. HWP5 inputs use `None` and writer defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hwpx_version_xml: Option<String>,
    /// Raw HWPX preview image bytes. The payload is intentionally excluded from
    /// JSON, like [`BinStream::data`], and exists only for same-format pass-through.
    #[serde(skip)]
    pub hwpx_preview_image: Option<Vec<u8>>,
    /// Original streams from HWP5 `/XMLTemplate` storage (CFB stream paths and bytes before
    /// decompression). Passes through XML schema binding parts (such as electronic-government
    /// forms, §3.2.10) unchanged when rewriting through the IR; their content is not interpreted.
    /// Empty for HWPX inputs and HWP5 documents without this storage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwp5_xml_template: Vec<(String, Vec<u8>)>,
    /// Original streams from HWP5 `/DocHistory` storage (CFB stream paths and bytes before
    /// decompression). Passes through document history (§3.2.11 and §4.4) unchanged on rewrite;
    /// content is not interpreted (§4.4 parsing is out of scope). Empty for HWPX or absent history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwp5_doc_history: Vec<(String, Vec<u8>)>,
    /// Original HWPX package entries the writer does not regenerate, preserving order and bytes.
    /// Examples: DocOptions, scripts, extra Preview and original META-INF/*. Empty for non-HWPX inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwpx_extra_entries: Vec<(String, Vec<u8>)>,
    /// BinData (id, href) mappings from the original HWPX `Contents/content.hpf` OPF manifest.
    /// A full rewrite seeds BinCollector ids from the original manifest so `binaryItemIDRef`
    /// attributes in captured objects continue to reference the original bytes. Empty for HWP5
    /// and synthesized documents, which retain the existing image{N} allocation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwpx_bin_manifest: Vec<(String, String)>,
    /// Extension-part (id, href, media-type) entries from the original HWPX OPF manifest that
    /// the writer does not regenerate: entries outside BinData/header/section/settings, such as
    /// DocOptions/Layout.xml. Re-registers them when regenerating content.hpf so package entries
    /// do not become orphaned. Empty for HWP5 and synthesized documents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwpx_opf_extra_items: Vec<(String, String, String)>,
    /// Additional xmlns declarations from original HWPX section roots (raw attribute strings
    /// such as `xmlns:foo="uri"`, excluding the writer standard hs/hp/hc, unioned across sections).
    /// Captured objects may use extension prefixes (hp10: or vendor prefixes) declared only at
    /// the root; retain them on rewritten roots for namespace validity. Empty for HWP5 or synthesized inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hwpx_section_xmlns: Vec<String>,
}

/// Document-level metadata (summary information / OPF metadata).
///
/// All fields are `Option` with `#[serde(default)]`, preserving JSON round-trip compatibility.
/// Missing values serialize as empty strings on write, retaining the sample structure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    /// Description (comments): HWP5 summary information PIDSI 6, HWPX OPF `meta[name=description]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Last saved by: HWP5 summary information PIDSI 8, HWPX OPF `meta[name=lastsaveby]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_saved_by: Option<String>,
    /// Creation time, preserving the HWP5 summary information PIDSI 12 (VT_FILETIME) **raw u64**
    /// in 100 ns units since 1601-01-01 UTC. Preserve losslessly: no timezone conversion or stringification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_time: Option<u64>,
    /// Modification (last-save) time: preserves HWP5 summary information PIDSI 13 (VT_FILETIME) as **raw u64**.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modify_time: Option<u64>,
}

impl Metadata {
    /// Whether every field is empty.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.author.is_none()
            && self.subject.is_none()
            && self.keywords.is_none()
            && self.description.is_none()
            && self.last_saved_by.is_none()
            && self.create_time.is_none()
            && self.modify_time.is_none()
    }
}

/// One attached binary. Bytes are omitted from JSON serialization to avoid oversized L2 output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinStream {
    pub name: String,
    #[serde(skip)]
    pub data: Vec<u8>,
}

impl Document {
    /// Index into [`Document::bin_streams`] of the stream a `BinRef` names, or `None` when the
    /// document carries no such stream. [`Document::resolve_bin`] is this lookup plus the bytes,
    /// so a caller that needs to know WHICH stream a picture uses (resource closure of an
    /// extracted range) cannot drift from the rule the renderer and writers use.
    pub fn bin_stream_index(&self, bin_ref: &BinRef) -> Option<usize> {
        match bin_ref {
            BinRef::Id(id) => {
                let item = self.header.bin_data.get((id.0 as usize).checked_sub(1)?)?;
                let storage_id = item.storage_id?;
                let ext = item.extension.as_deref().unwrap_or("");
                let name = format!("BIN{storage_id:04X}.{ext}");
                self.bin_streams
                    .iter()
                    .position(|s| s.name.eq_ignore_ascii_case(&name))
            }
            BinRef::ItemRef(item) => {
                // The package manifest is authoritative: an item id need not equal the file
                // name (`id="logo" href="BinData/p1.png"`). An id the manifest lists resolves
                // to its href's stream or to nothing: when the href names a stream the
                // document does not carry, a same-stem stream of another item must not stand
                // in for it. Names are the fallback only for ids the manifest does not list
                // (a synthesized document, or one read from HWP5, has no manifest map).
                if let Some((_, href)) = self.hwpx_bin_manifest.iter().find(|(id, _)| id == item) {
                    return self.bin_streams.iter().position(|s| s.name == *href);
                }
                self.bin_streams.iter().position(|s| {
                    s.name == *item
                        || s.name.ends_with(&format!("/{item}"))
                        || s.name
                            .rsplit('/')
                            .next()
                            .and_then(|f| f.split('.').next())
                            .is_some_and(|stem| stem == item)
                })
            }
        }
    }

    /// Resolves a Picture BinRef to its actual bytes.
    pub fn resolve_bin(&self, bin_ref: &BinRef) -> Option<&[u8]> {
        self.bin_stream_index(bin_ref)
            .map(|index| self.bin_streams[index].data.as_slice())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocMeta {
    /// "hwp5" | "hwpx"
    pub source_format: String,
    /// Original file version, such as "5.1.0.1".
    pub source_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub paragraphs: Vec<Paragraph>,
    /// Top-level records outside paragraphs (empty in well-formed files).
    pub extras: Vec<crate::opaque::OpaqueRecord>,
}

impl Section {
    /// Section definition control, normally the first control of the first paragraph.
    pub fn section_def(&self) -> Option<&SectionDef> {
        self.paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::SectionDef(sd) => Some(sd),
                _ => None,
            })
    }
}

#[cfg(test)]
mod bin_stream_index_tests {
    use super::*;

    fn stream(name: &str, data: &[u8]) -> BinStream {
        BinStream {
            name: name.to_string(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn manifest_id_that_differs_from_the_file_stem_resolves_through_the_manifest() {
        let doc = Document {
            bin_streams: vec![
                stream("BinData/p1.png", b"one"),
                stream("BinData/p2.png", b"two"),
            ],
            hwpx_bin_manifest: vec![
                ("logo".into(), "BinData/p1.png".into()),
                ("seal".into(), "BinData/p2.png".into()),
            ],
            ..Document::default()
        };
        assert_eq!(
            doc.bin_stream_index(&BinRef::ItemRef("logo".into())),
            Some(0)
        );
        assert_eq!(
            doc.bin_stream_index(&BinRef::ItemRef("seal".into())),
            Some(1)
        );
        assert_eq!(
            doc.resolve_bin(&BinRef::ItemRef("seal".into())),
            Some(&b"two"[..])
        );
    }

    #[test]
    fn the_manifest_wins_over_a_name_that_happens_to_match_another_stream() {
        // `id="p2"` points at p1.png even though a stream is named p2.png.
        let doc = Document {
            bin_streams: vec![
                stream("BinData/p1.png", b"one"),
                stream("BinData/p2.png", b"two"),
            ],
            hwpx_bin_manifest: vec![("p2".into(), "BinData/p1.png".into())],
            ..Document::default()
        };
        assert_eq!(doc.bin_stream_index(&BinRef::ItemRef("p2".into())), Some(0));
    }

    #[test]
    fn a_manifest_id_whose_href_is_missing_does_not_fall_back_to_a_same_stem_stream() {
        // `logo` is listed, but its href names a stream the document does not carry. An
        // unrelated `BinData/logo.png` shares the stem and must not stand in for it.
        let doc = Document {
            bin_streams: vec![stream("BinData/logo.png", b"someone else's logo")],
            hwpx_bin_manifest: vec![("logo".into(), "BinData/missing.png".into())],
            ..Document::default()
        };
        assert_eq!(doc.bin_stream_index(&BinRef::ItemRef("logo".into())), None);
        assert_eq!(doc.resolve_bin(&BinRef::ItemRef("logo".into())), None);
        // An id the manifest does not list still resolves by name.
        assert_eq!(
            doc.bin_stream_index(&BinRef::ItemRef("BinData/logo.png".into())),
            Some(0)
        );
    }

    #[test]
    fn without_a_manifest_entry_the_name_heuristics_still_apply() {
        let doc = Document {
            bin_streams: vec![stream("BinData/image1.png", b"one")],
            hwpx_bin_manifest: vec![("other".into(), "BinData/missing.png".into())],
            ..Document::default()
        };
        assert_eq!(
            doc.bin_stream_index(&BinRef::ItemRef("image1".into())),
            Some(0)
        );
        assert_eq!(doc.bin_stream_index(&BinRef::ItemRef("other".into())), None);
        assert_eq!(
            doc.bin_stream_index(&BinRef::ItemRef("nothing".into())),
            None
        );
    }
}
