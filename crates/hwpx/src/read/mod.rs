//! Reads HWPX into the IR.
//!
//! Aligns IR semantics with hwp5: `hp:secPr`/`hp:ctrl(colPr)`/`hp:tbl` become an extended control
//! character (8 WCHAR) plus `Control`, sharing position arithmetic and extraction code between the two
//! formats.
pub mod header;
pub(crate) mod password;
pub mod section;
mod xml;

use std::path::Path;

use hwp_model::{DocMeta, Document};
use quick_xml::events::Event;

use crate::error::Result;
use crate::package::HwpxPackage;
use xml::resolve_entity;

pub struct ReadResult {
    pub document: Document,
    pub warnings: Vec<String>,
}

/// Per-call options for the HWPX reader. Password bytes are used exactly as
/// provided and do not survive the reader invocation.
#[derive(Clone, Copy, Default)]
pub struct ReadOptions<'a> {
    pub password: Option<&'a str>,
}

/// Reads an HWPX file into the IR.
pub fn read_document(path: &Path) -> Result<ReadResult> {
    read_document_with_options(path, &ReadOptions::default())
}

/// Reads an HWPX document with the explicit per-call password option.
pub fn read_document_with_options(path: &Path, options: &ReadOptions<'_>) -> Result<ReadResult> {
    read_document_impl(path, true, options)
}

/// Reads the HWPX structure and XML without loading `BinData/*` payloads.
///
/// Streams every non-directory entry to EOF through `sink` to validate its CRC/compressed stream. Image
/// bytes are not loaded into the IR, keeping memory independent of attachment size for large valid
/// documents.
pub fn read_structure(path: &Path) -> Result<ReadResult> {
    read_document_impl(path, false, &ReadOptions::default())
}

fn read_document_impl(
    path: &Path,
    load_binary_data: bool,
    options: &ReadOptions<'_>,
) -> Result<ReadResult> {
    let mut pkg = HwpxPackage::open(path)?;
    // GATE-02: refuse an encrypted package before the integrity sweep or any content
    // part is read. An encrypted package can fail integrity for reasons unrelated to
    // the user's actual problem; the typed encryption message is the one that helps.
    match options.password {
        Some(password) if pkg.has_encryption_marker()? => pkg.unlock_with_password(password)?,
        Some(_) => {}
        None => pkg.check_body_readable()?,
    }
    // Also detect corruption in Preview/BinData/extension parts not used by the parser. Do not retain
    // their bytes; reread only the parts needed later.

    pkg.verify_integrity()?;
    let mut warnings = Vec::new();

    let header_xml = pkg.read_entry_string("Contents/header.xml")?;
    let (mut doc_header, header_warnings) = header::parse_header(&header_xml)?;
    warnings.extend(
        header_warnings
            .into_iter()
            .map(|w| format!("[header.xml] {w}")),
    );

    let mut sections = Vec::new();
    let mut hwpx_section_xmlns: Vec<String> = Vec::new();
    for entry in pkg.section_entries()? {
        let xml = pkg.read_entry_string(&entry)?;
        // Preserve the union of root-only extension prefixes across sections, deduplicated by prefix:
        // emitting different URIs for one prefix as duplicate attributes would be malformed XML.

        for decl in section::extract_section_root_xmlns(&xml) {
            let prefix = decl
                .strip_prefix("xmlns:")
                .and_then(|rest| rest.split('=').next())
                .unwrap_or("");
            let dominated = hwpx_section_xmlns.iter().any(|existing| {
                existing
                    .strip_prefix("xmlns:")
                    .and_then(|rest| rest.split('=').next())
                    .unwrap_or("")
                    == prefix
            });
            if !dominated {
                hwpx_section_xmlns.push(decl);
            }
        }
        let (section, sec_warnings) = section::parse_section(&xml)?;
        warnings.extend(sec_warnings.into_iter().map(|w| format!("[{entry}] {w}")));
        sections.push(section);
    }
    doc_header.properties.section_count = sections.len() as u16;

    // Attached binaries (images, etc.); BinRef::ItemRef uses the entry-name heuristic.
    let mut bin_streams = Vec::new();
    if load_binary_data {
        for entry in pkg.entries()? {
            if entry.name.starts_with("BinData/") {
                let data = pkg.read_entry(&entry.name)?;
                bin_streams.push(hwp_model::BinStream {
                    name: entry.name,
                    data,
                });
            }
        }
    }

    let version = pkg
        .version_info()?
        .into_iter()
        .filter(|(k, _)| ["major", "minor", "micro", "buildNumber"].contains(&k.as_str()))
        .map(|(_, v)| v)
        .collect::<Vec<_>>()
        .join(".");

    // Preserve every package entry not regenerated or held in dedicated writer slots, in its original
    // order and bytes (META-INF/*, DocOptions, scripts, extra Preview parts, etc.). Never discard
    // unknown data. BinData/* belongs to bin_streams; section/header/settings/version/preview already
    // have dedicated slots.

    let mut hwpx_extra_entries = Vec::new();
    let was_unlocked_with_password = pkg.was_unlocked_with_password();
    if load_binary_data {
        const REGENERATED: &[&str] = &[
            "mimetype",
            "version.xml",
            "Contents/content.hpf",
            "Contents/header.xml",
            "Preview/PrvText.txt",
            "Preview/PrvImage.png",
            "settings.xml",
        ];
        for entry in pkg.entries()? {
            let name = &entry.name;
            if name.ends_with('/')
                || REGENERATED.contains(&name.as_str())
                // Derive the section list from actual package entries, using the same rule as
                // section_entries.
                || (name.starts_with("Contents/section") && name.ends_with(".xml"))
                || name.starts_with("BinData/")
            {
                continue;
            }
            let data = pkg.read_entry(name)?;
            let data = if was_unlocked_with_password && name == "META-INF/manifest.xml" {
                strip_manifest_encryption_data(&data)?
            } else {
                data
            };
            // Entries regenerated byte-identically from writer templates are not extras. Capturing them
            // would make semantic verification misread the presence of default-value slots as a
            // difference.

            if is_writer_default_entry(name, &data) {
                continue;
            }
            hwpx_extra_entries.push((name.clone(), data));
        }
    }

    // Document metadata (content.hpf OPF), best effort so diagnosis continues if missing or corrupt.
    let content_hpf = pkg.read_entry_string("Contents/content.hpf").ok();
    let metadata = content_hpf
        .as_deref()
        .map(parse_content_meta)
        .unwrap_or_default();
    // OPF BinData (id, href) mappings seed BinCollector with original IDs during full rewrites, keeping
    // binaryItemIDRef in raw-captured objects consistent.

    let hwpx_bin_manifest = content_hpf
        .as_deref()
        .map(parse_bin_manifest)
        .unwrap_or_default();
    // Manifest entries for extension parts not regenerated by the writer are listed again when
    // content.hpf is regenerated, preventing raw-copied entries from becoming orphan parts.

    let hwpx_opf_extra_items = content_hpf
        .as_deref()
        .map(parse_opf_extra_items)
        .unwrap_or_default();

    // Raw passthrough slots preserve whole settings.xml (application settings/caret) and version.xml
    // (version metadata). Missing values are None and use writer defaults. Never discard unknown data.

    let hwpx_settings_xml = pkg.read_entry_string("settings.xml").ok();
    let hwpx_version_xml = pkg.read_entry_string("version.xml").ok();
    let has_preview_image = pkg
        .entries()?
        .iter()
        .any(|entry| entry.name == "Preview/PrvImage.png");
    let hwpx_preview_image = if load_binary_data && has_preview_image {
        Some(pkg.read_entry("Preview/PrvImage.png")?)
    } else {
        None
    };

    Ok(ReadResult {
        document: Document {
            meta: DocMeta {
                source_format: "hwpx".to_string(),
                source_version: version,
            },
            metadata,
            header: doc_header,
            sections,
            bin_streams,
            hwpx_settings_xml,
            hwpx_version_xml,
            hwpx_preview_image,
            // HWPX input has no HWP5-only storage, independently of the GE-beta7/beta8 path.
            hwp5_xml_template: Vec::new(),
            hwp5_doc_history: Vec::new(),
            hwpx_extra_entries,
            hwpx_bin_manifest,
            hwpx_opf_extra_items,
            hwpx_section_xmlns,
        },
        warnings,
    })
}

/// Whether this META-INF entry is regenerated byte-identically by a writer template. Such entries stay
/// out of `hwpx_extra_entries`; regeneration already preserves them.
fn is_writer_default_entry(name: &str, bytes: &[u8]) -> bool {
    let default = match name {
        "META-INF/container.rdf" => crate::write::templates::CONTAINER_RDF,
        "META-INF/container.xml" => crate::write::templates::CONTAINER_XML,
        "META-INF/manifest.xml" => crate::write::templates::MANIFEST_XML,
        _ => return false,
    };
    bytes == default.as_bytes()
}

fn strip_manifest_encryption_data(manifest: &[u8]) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(manifest);
    let mut writer = quick_xml::Writer::new(Vec::with_capacity(manifest.len()));
    let mut skipped_depth = 0usize;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| crate::error::HwpxError::Xml {
                entry: "META-INF/manifest.xml".to_string(),
                message: error.to_string(),
            })?;
        match &event {
            Event::Start(element)
                if skipped_depth > 0
                    || element.local_name().as_ref().as_bytes() == b"encryption-data" =>
            {
                skipped_depth =
                    skipped_depth
                        .checked_add(1)
                        .ok_or_else(|| crate::error::HwpxError::Xml {
                            entry: "META-INF/manifest.xml".to_string(),
                            message: "encryption-data nesting overflow".to_string(),
                        })?;
            }
            Event::Empty(element)
                if skipped_depth > 0
                    || element.local_name().as_ref().as_bytes() == b"encryption-data" => {}
            Event::End(_) if skipped_depth > 0 => skipped_depth -= 1,
            Event::Eof => {
                if skipped_depth != 0 {
                    return Err(crate::error::HwpxError::Xml {
                        entry: "META-INF/manifest.xml".to_string(),
                        message: "unclosed encryption-data element".to_string(),
                    });
                }
                writer
                    .write_event(event)
                    .map_err(|error| crate::error::HwpxError::Xml {
                        entry: "META-INF/manifest.xml".to_string(),
                        message: error.to_string(),
                    })?;
                break;
            }
            _ if skipped_depth > 0 => {}
            _ => writer
                .write_event(event)
                .map_err(|error| crate::error::HwpxError::Xml {
                    entry: "META-INF/manifest.xml".to_string(),
                    message: error.to_string(),
                })?,
        }
    }
    Ok(writer.into_inner())
}

/// Extracts summary information from content.hpf OPF metadata (best effort).
///
/// Prefers the forms observed in genuine samples:
/// - `<opf:title>`, `<opf:language>` (ignored)
/// - `<opf:meta name="creator|subject|description|lastsaveby|keyword" content="text">value</opf:meta>`
///   (element text holds the value; `keyword` is singular).
/// - `<opf:meta name="CreatedDate|ModifiedDate" content="text">ISO8601</opf:meta>`
///   -> [`iso8601_utc_to_filetime`] reconstructs a raw FILETIME u64 at second precision, losing
///   subsecond 100ns units.
/// - `<opf:meta name="date">` (a Korean KST-derived value) is ignored and derived again from
///   create_time.
///
/// Legacy forms remain supported: `<dc:creator>`/`<dc:subject>` element text and `<opf:meta
/// name="keywords" content="value"/>` (plural name plus content attribute).
pub fn parse_content_meta(xml: &str) -> hwp_model::Metadata {
    use quick_xml::events::Event;
    let mut meta = hwp_model::Metadata::default();
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut capture: Option<(&'static str, String)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref().as_bytes() {
                // Legacy dc:title/dc:creator/dc:subject element text.
                b"title" => capture = Some(("title", String::new())),
                b"creator" => capture = Some(("author", String::new())),
                b"subject" => capture = Some(("subject", String::new())),
                b"meta" => {
                    keywords_from_meta(&e, &mut meta);
                    // The following Text events supply metadata stored as element text.
                    capture = meta_capture(&e).map(|field| (field, String::new()));
                }
                _ => capture = None,
            },
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref().as_bytes() == b"meta" {
                    // Even an empty element may carry the legacy keywords content attribute.
                    keywords_from_meta(&e, &mut meta);
                }
            }
            Ok(Event::Text(t)) => {
                if let Some((_, value)) = &mut capture {
                    value.push_str(&t.xml10_content());
                }
            }
            // quick-xml emits references such as `&gt;` as separate GeneralRef events, not inside Text.
            // Ending capture after the first Text would truncate `A=&gt;B` to `A=`; collect and resolve
            // everything until the closing tag.
            Ok(Event::GeneralRef(r)) => {
                if let Some((_, value)) = &mut capture
                    && let Some(c) = resolve_entity(&r)
                {
                    value.push(c);
                }
            }
            Ok(Event::CData(t)) => {
                if let Some((_, value)) = &mut capture {
                    value.push_str(&t.xml10_content());
                }
            }
            Ok(Event::End(_)) => {
                if let Some((field, value)) = capture.take() {
                    set_metadata_field(&mut meta, field, value.trim());
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    meta
}

/// Reads all `<opf:item>` entries in the content.hpf OPF manifest as (id, href, media-type), best
/// effort. Skip entries without id/href; missing media-type becomes None. Preserve raw escaped attribute
/// values to avoid double escaping on output.
fn parse_opf_items(xml: &str) -> Vec<(String, String, Option<String>)> {
    use quick_xml::events::Event;

    let mut items = Vec::new();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.name().local_name().as_ref().as_bytes() != b"item" {
                    continue;
                }
                let mut id = None;
                let mut href = None;
                let mut mime = None;
                for attr in event.attributes().flatten() {
                    let value = attr.value.into_owned();
                    match attr.key.local_name().as_ref().as_bytes() {
                        b"id" => id = Some(value),
                        b"href" => href = Some(value),
                        b"media-type" => mime = Some(value),
                        _ => {}
                    }
                }
                if let (Some(id), Some(href)) = (id, href) {
                    items.push((id, href, mime));
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    items
}

/// Reads BinData (id, href) mappings from the content.hpf OPF manifest (best effort).
///
/// Full rewrites seed BinCollector with original manifest IDs so binaryItemIDRef inside raw-captured
/// objects (e.g. hp:container) continues pointing to the original bytes after serialization. Parse
/// failures or absent BinData entries yield an empty vector, retaining the caller's image{N} allocation
/// path.
pub fn parse_bin_manifest(xml: &str) -> Vec<(String, String)> {
    parse_opf_items(xml)
        .into_iter()
        .filter(|(_, href, _)| href.starts_with("BinData/"))
        .map(|(id, href, _)| (id, href))
        .collect()
}

/// Reads (id, href, media-type) for extension parts not regenerated by the writer, best effort: entries
/// outside BinData/header/section/settings (e.g. DocOptions/Layout.xml). List them again when
/// content.hpf is regenerated so raw-copied package entries remain in the manifest rather than becoming
/// orphan parts.
pub fn parse_opf_extra_items(xml: &str) -> Vec<(String, String, String)> {
    parse_opf_items(xml)
        .into_iter()
        .filter(|(_, href, _)| {
            !href.starts_with("BinData/")
                && href != "Contents/header.xml"
                && href != "settings.xml"
                && !(href.starts_with("Contents/section") && href.ends_with(".xml"))
        })
        .map(|(id, href, mime)| {
            (
                id,
                href,
                mime.unwrap_or_else(|| "application/octet-stream".to_string()),
            )
        })
        .collect()
}

fn set_metadata_field(meta: &mut hwp_model::Metadata, field: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    match field {
        "title" => meta.title = Some(value.to_string()),
        "author" => meta.author = Some(value.to_string()),
        "subject" => meta.subject = Some(value.to_string()),
        "keywords" => meta.keywords = Some(value.to_string()),
        "description" => meta.description = Some(value.to_string()),
        "last_saved_by" => meta.last_saved_by = Some(value.to_string()),
        "create_time" => meta.create_time = hwp_model::iso8601_utc_to_filetime(value),
        "modify_time" => meta.modify_time = hwp_model::iso8601_utc_to_filetime(value),
        _ => {}
    }
}

/// Maps the name attribute of `<opf:meta name="...">` to the field capturing its element text.
fn meta_capture(e: &quick_xml::events::BytesStart<'_>) -> Option<&'static str> {
    match xml::attr(e, "name").as_deref() {
        Some("creator") => Some("author"),
        Some("subject") => Some("subject"),
        Some("keyword") => Some("keywords"), // Genuine form: singular-name element text.
        Some("description") => Some("description"),
        Some("lastsaveby") => Some("last_saved_by"),
        Some("CreatedDate") => Some("create_time"),
        Some("ModifiedDate") => Some("modify_time"),
        _ => None,
    }
}

/// Legacy compatibility: `<opf:meta name="keywords" content="value"/>` (plural name plus content
/// attribute).
fn keywords_from_meta(e: &quick_xml::events::BytesStart<'_>, meta: &mut hwp_model::Metadata) {
    if xml::attr(e, "name").as_deref() == Some("keywords")
        && let Some(v) = xml::attr(e, "content").filter(|v| !v.is_empty())
    {
        meta.keywords = Some(v);
    }
}
