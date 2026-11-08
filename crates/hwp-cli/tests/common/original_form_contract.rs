//! Test-only original-form contract. No writer rules or production editing engine.
use std::collections::BTreeMap;
use std::fmt;

use hwp_model::{Control, Document, HwpChar, Paragraph};
use hwp_render::segment_map::SegmentMap;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, PartialEq, Eq)]
pub enum Atom {
    Text(char),
    Control(u16),
}

/// Own paragraph content: boundaries/whitespace survive; object payloads are checked separately.
fn atoms(para: &Paragraph) -> Vec<Atom> {
    para.chars
        .iter()
        .filter_map(|ch| match ch {
            HwpChar::Text(ch) => Some(Atom::Text(*ch)),
            HwpChar::CharCtrl(code) if *code != hwp_model::ctrl_char::PARA_BREAK => {
                Some(Atom::Control(*code))
            }
            HwpChar::InlineCtrl { code, .. } => Some(Atom::Control(*code)),
            _ => None,
        })
        .collect()
}

pub fn direct_text(para: &Paragraph) -> String {
    atoms(para)
        .into_iter()
        .filter_map(|atom| match atom {
            Atom::Text(ch) => Some(ch),
            Atom::Control(hwp_model::ctrl_char::LINE_BREAK) => Some('\n'),
            Atom::Control(hwp_model::ctrl_char::TAB) => Some('\t'),
            _ => None,
        })
        .collect()
}

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Diagnostics carry a content-free stable ID, never cell text or document coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractViolation {
    pub layer: &'static str,
    pub code: &'static str,
    pub id: String,
}
impl fmt::Display for ContractViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "original-form {}: {} (id={}); validate the document and inspect the local expected projection",
            self.layer, self.code, self.id
        )
    }
}
fn condition(layer: &'static str, code: &'static str, key: &str) -> ContractViolation {
    ContractViolation {
        layer,
        code,
        id: digest(format!("{layer}:{code}:{key}").as_bytes())[..16].into(),
    }
}
fn violation(layer: &'static str, key: &str) -> ContractViolation {
    condition(layer, "mismatch", key)
}
fn same<T: PartialEq>(
    expected: &T,
    actual: &T,
    layer: &'static str,
    key: &str,
) -> Result<(), ContractViolation> {
    if expected == actual {
        Ok(())
    } else {
        Err(violation(layer, key))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CellProjection {
    pub row: u16,
    pub col: u16,
    pub row_span: u16,
    pub col_span: u16,
    pub width: i32,
    pub minimum_height: i32,
    pub paragraphs: Vec<String>,
    values: Vec<Vec<Atom>>,
    paragraph_keys: Vec<String>,
    style: Value,
}
#[derive(Clone, PartialEq, Eq)]
pub struct TableProjection {
    pub key: String,
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<CellProjection>,
    style: Value,
}
#[derive(Clone, PartialEq, Eq)]
pub struct ImageProjection {
    pub key: String,
    pub payload_sha256: Option<String>,
    geometry: Value,
}
#[derive(Clone, PartialEq, Eq)]
struct ParagraphProjection {
    key: String,
    values: Vec<Atom>,
    style: Value,
    break_type: u8,
}
#[derive(Clone, PartialEq, Eq)]
pub struct FormProjection {
    pub tables: Vec<TableProjection>,
    pub images: Vec<ImageProjection>,
    pub resource_hashes: Vec<String>,
    paragraphs: Vec<ParagraphProjection>,
    pages: Vec<Value>,
    invalid_references: Vec<ContractViolation>,
}

/// Closed expected values, addressed by table occurrence and grid coordinate, never by label.
#[derive(Clone)]
pub struct ExpectedCell {
    pub table: usize,
    pub row: u16,
    pub col: u16,
    pub paragraphs: Vec<String>,
}

fn border(doc: &Document, id: u16) -> Value {
    if id == 0 {
        return Value::Null;
    }
    let Some(fill) = doc.header.border_fills.get(id as usize - 1) else {
        return json!({"unresolved_border":true});
    };
    let line = |line: &hwp_model::BorderLine| {
        if line.is_visible() {
            json!([line.line_type, line.width, line.color])
        } else {
            Value::Null
        }
    };
    let sides: Vec<_> = fill.sides.iter().map(line).collect();
    let diagonal = line(&fill.diagonal);
    let bg = fill.visible_bg();
    let hatch = fill.visible_hatch();
    let gradient = fill.visible_gradient();
    if sides.iter().all(Value::is_null)
        && diagonal.is_null()
        && bg.is_none()
        && hatch.is_none()
        && gradient.is_none()
    {
        return Value::Null;
    }
    json!({"attr":fill.attr,"sides":sides,"diagonal":diagonal,"bg":bg,"hatch":hatch,"gradient":gradient})
}
fn paragraph_style(doc: &Document, para: &Paragraph) -> Value {
    let paragraph = doc.header.para_shapes.get(para.para_shape.0 as usize).map(|ps| {
        let tab = doc.header.tab_stops.get(ps.tab_def_id as usize).filter(|tab| tab.attr != 0 || !tab.items.is_empty());
        let numbering = match ps.head_type() {
            2 => json!(doc.header.numbering_levels.get(ps.numbering_id as usize)),
            3 => json!(doc.header.bullet_chars.get(ps.numbering_id as usize)),
            _ => Value::Null,
        };
        json!({"align":ps.alignment(),"indent":ps.indent,"left":ps.margin_left,"right":ps.margin_right,
            "before":ps.spacing_top,"after":ps.spacing_bottom,"spacing_type":ps.line_spacing_type,
            "spacing":if ps.line_spacing != 0 || ps.line_spacing_type != 0 {ps.line_spacing} else {ps.line_spacing_old},
            "head_type":ps.head_type(),"head_level":ps.head_level(),"numbering":numbering,
            "keep_next":ps.keep_with_next(),"keep_lines":ps.keep_lines(),"widow_orphan":ps.widow_orphan(),
            "break_before":ps.page_break_before(),"tabs":tab,"border":border(doc,ps.border_fill_id),"border_offsets":ps.border_offsets})
    });
    let runs: Vec<_> = para.char_shape_runs.iter().map(|(at,id)| {
        let shape = doc.header.char_shapes.get(id.0 as usize).map(|cs| {
            let fonts: Vec<_> = cs.face_ids.iter().enumerate().map(|(slot,id)| {
                doc.header.fonts[slot].get(*id as usize).map(|face| json!({"name":face.name,"alternate":face.alt_name}))
            }).collect();
            json!({"fonts":fonts,"size":cs.base_size,"ratios":cs.ratios,"spacings":cs.spacings,
                "relative_sizes":cs.rel_sizes,"offsets":cs.offsets,"bold":cs.is_bold(),"italic":cs.is_italic(),
                "strike":cs.strike,"text_color":cs.text_color,"shade_color":cs.shade_color,
                "underline":if cs.has_underline() {json!([cs.underline_kind(),cs.underline_shape_code(),cs.underline_color])} else {Value::Null},
                "border":border(doc,cs.border_fill_id)})
        });
        json!({"at":at,"shape":shape})
    }).collect();
    let content_end: u32 = para
        .chars
        .iter()
        .filter(|ch| !matches!(ch, HwpChar::CharCtrl(hwp_model::ctrl_char::PARA_BREAK)))
        .map(HwpChar::wchar_width)
        .sum();
    let mut canonical: Vec<Value> = Vec::new();
    for run in runs {
        let at = run["at"].as_u64().unwrap();
        if at > 0 && at >= u64::from(content_end) {
            continue;
        }
        if canonical
            .last()
            .is_some_and(|last| last["shape"] == run["shape"])
        {
            continue;
        }
        canonical.push(run);
    }
    json!({"paragraph":paragraph,"runs":canonical})
}

pub fn project_form(doc: &Document) -> FormProjection {
    let mut result = FormProjection {
        tables: Vec::new(),
        images: Vec::new(),
        resource_hashes: doc
            .bin_streams
            .iter()
            .map(|bin| digest(&bin.data))
            .collect(),
        paragraphs: Vec::new(),
        pages: Vec::new(),
        invalid_references: Vec::new(),
    };
    result.resource_hashes.sort();
    for (section_index, section) in doc.sections.iter().enumerate() {
        result.pages.push(json!(
            section.section_def().and_then(|def| def.page.as_ref())
        ));
        for (index, para) in section.paragraphs.iter().enumerate() {
            visit_paragraph(
                doc,
                para,
                &format!("s{section_index}/p{index}"),
                &mut result,
            );
        }
    }
    result
}
fn invalid_border(doc: &Document, id: u16) -> bool {
    id != 0 && id as usize > doc.header.border_fills.len()
}
fn check_references(doc: &Document, para: &Paragraph, key: &str, result: &mut FormProjection) {
    let mut invalid = false;
    if let Some(ps) = doc.header.para_shapes.get(para.para_shape.0 as usize) {
        invalid |= invalid_border(doc, ps.border_fill_id);
        invalid |= ps.tab_def_id != 0 && doc.header.tab_stops.get(ps.tab_def_id as usize).is_none();
        invalid |= match ps.head_type() {
            2 => doc
                .header
                .numbering_levels
                .get(ps.numbering_id as usize)
                .is_none(),
            3 => doc
                .header
                .bullet_chars
                .get(ps.numbering_id as usize)
                .is_none(),
            _ => false,
        };
    } else {
        invalid = true;
    }
    for (_, id) in &para.char_shape_runs {
        if let Some(cs) = doc.header.char_shapes.get(id.0 as usize) {
            invalid |= invalid_border(doc, cs.border_fill_id);
            invalid |= cs
                .face_ids
                .iter()
                .enumerate()
                .any(|(slot, id)| doc.header.fonts[slot].get(*id as usize).is_none());
        } else {
            invalid = true;
        }
    }
    if invalid {
        result
            .invalid_references
            .push(condition("references", "unresolved_reference", key));
    }
}
fn visit_paragraph(doc: &Document, para: &Paragraph, key: &str, result: &mut FormProjection) {
    check_references(doc, para, key, result);
    result.paragraphs.push(ParagraphProjection {
        key: key.into(),
        values: atoms(para),
        style: paragraph_style(doc, para),
        break_type: para.header.break_type & 4,
    });
    for (index, control) in para.controls.iter().enumerate() {
        let key = format!("{key}/c{index}");
        match control {
            Control::Table(table) => {
                if invalid_border(doc, table.border_fill.0)
                    || table
                        .cells
                        .iter()
                        .any(|cell| invalid_border(doc, cell.border_fill.0))
                {
                    result.invalid_references.push(condition(
                        "references",
                        "unresolved_reference",
                        &key,
                    ));
                }
                let mut cells: Vec<_> = table.cells.iter().collect();
                cells.sort_by_key(|cell| (cell.row, cell.col));
                let projected_cells = cells.iter().map(|cell| {
                    // Picture-only paragraphs are resource references, not text bindings.
                    let values: Vec<_> = cell.paragraphs.iter().enumerate().filter(|(_,para)| {
                        !(atoms(para).is_empty() && para.controls.iter().any(|c| matches!(c,Control::Picture(_))))
                    }).collect();
                    CellProjection {
                        row:cell.row,col:cell.col,row_span:cell.row_span,col_span:cell.col_span,
                        width:cell.width.0,minimum_height:cell.height.0,
                        paragraphs:values.iter().map(|(_,p)|direct_text(p)).collect(),
                        values:values.iter().map(|(_,p)|atoms(p)).collect(),
                        paragraph_keys:values.iter().map(|(index,_)|format!("{key}/r{}c{}/p{index}",cell.row,cell.col)).collect(),
                        style:json!({"margins":cell.margins,"vertical_align":(cell.list_attr >> 5) & 3,"border":border(doc,cell.border_fill.0)}),
                    }
                }).collect();
                result.tables.push(TableProjection {key:key.clone(),rows:table.rows,cols:table.cols,cells:projected_cells,
                    style:json!({"inner_margins":table.inner_margins,"cell_spacing":table.cell_spacing,"border":border(doc,table.border_fill.0)})});
                for cell in cells {
                    for (index, para) in cell.paragraphs.iter().enumerate() {
                        visit_paragraph(
                            doc,
                            para,
                            &format!("{key}/r{}c{}/p{index}", cell.row, cell.col),
                            result,
                        );
                    }
                }
                if let Some(caption) = &table.caption {
                    visit_list(doc, &caption.paragraphs, &format!("{key}/caption"), result);
                }
            }
            Control::Picture(picture) => {
                if doc.resolve_bin(&picture.bin_ref).is_none() {
                    result.invalid_references.push(condition(
                        "references",
                        "unresolved_reference",
                        &key,
                    ));
                }
                result.images.push(ImageProjection {key:key.clone(),payload_sha256:doc.resolve_bin(&picture.bin_ref).map(digest),
                    geometry:json!({"width":picture.width.0,"height":picture.height.0,"inline":picture.treat_as_char,"crop":picture.crop,"flip":picture.flip,"rotation":picture.rotation,"description":picture.description})});
                if let Some(caption) = &picture.caption {
                    visit_list(doc, &caption.paragraphs, &format!("{key}/caption"), result);
                }
            }
            Control::Generic(generic) => {
                for (index, list) in generic.paragraph_lists.iter().enumerate() {
                    visit_list(doc, &list.paragraphs, &format!("{key}/list{index}"), result);
                }
                if let Some(caption) = &generic.caption {
                    visit_list(doc, &caption.paragraphs, &format!("{key}/caption"), result);
                }
            }
            Control::SectionDef(_) => {}
        }
    }
}
fn visit_list(doc: &Document, paras: &[Paragraph], key: &str, result: &mut FormProjection) {
    for (index, para) in paras.iter().enumerate() {
        visit_paragraph(doc, para, &format!("{key}/p{index}"), result);
    }
}

pub fn compare_expected_cells(
    expected: &[ExpectedCell],
    actual: &FormProjection,
) -> Result<(), ContractViolation> {
    let mut unique = BTreeMap::new();
    for expected in expected {
        let key = (expected.table, expected.row, expected.col);
        if unique.insert(key, ()).is_some() {
            return Err(condition("expected_cells", "duplicate_target", "selection"));
        }
        let cell = actual.tables.get(expected.table).and_then(|table| {
            table
                .cells
                .iter()
                .find(|cell| cell.row == expected.row && cell.col == expected.col)
        });
        let Some(cell) = cell else {
            return Err(violation("cell_values", &format!("{key:?}")));
        };
        let values: Vec<_> = expected
            .paragraphs
            .iter()
            .map(|text| string_atoms(text))
            .collect();
        same(&values, &cell.values, "cell_values", &format!("{key:?}"))?;
    }
    Ok(())
}
pub fn compare_form_contract(
    expected: &FormProjection,
    actual: &FormProjection,
) -> Result<(), ContractViolation> {
    if let Some(invalid) = expected
        .invalid_references
        .first()
        .or_else(|| actual.invalid_references.first())
    {
        return Err(invalid.clone());
    }
    same(
        &expected.tables.len(),
        &actual.tables.len(),
        "grid",
        "table_count",
    )?;
    for (index, (before, after)) in expected.tables.iter().zip(&actual.tables).enumerate() {
        same(
            &(before.rows, before.cols),
            &(after.rows, after.cols),
            "grid",
            &index.to_string(),
        )?;
        same(
            &before.cells.len(),
            &after.cells.len(),
            "grid",
            &index.to_string(),
        )?;
        for (before, after) in before.cells.iter().zip(&after.cells) {
            let key = format!("{index}/{}/{}", before.row, before.col);
            same(&before.values, &after.values, "cell_values", &key)?;
            same(
                &(
                    before.row,
                    before.col,
                    before.row_span,
                    before.col_span,
                    before.width,
                    before.minimum_height,
                ),
                &(
                    after.row,
                    after.col,
                    after.row_span,
                    after.col_span,
                    after.width,
                    after.minimum_height,
                ),
                "grid",
                &key,
            )?;
            same(&before.style, &after.style, "style", &key)?;
        }
        same(&before.style, &after.style, "style", &index.to_string())?;
    }
    same(
        &expected.paragraphs.len(),
        &actual.paragraphs.len(),
        "paragraphs",
        "count",
    )?;
    for (before, after) in expected.paragraphs.iter().zip(&actual.paragraphs) {
        same(&before.key, &after.key, "structure", &before.key)?;
        same(
            &before.values,
            &after.values,
            "paragraph_values",
            &before.key,
        )?;
        same(&before.style, &after.style, "style", &before.key)?;
        same(&before.break_type, &after.break_type, "breaks", &before.key)?;
    }
    same(&expected.pages, &actual.pages, "page_setup", "sections")?;
    same(
        &expected.resource_hashes,
        &actual.resource_hashes,
        "resources",
        "inventory",
    )?;
    same(&expected.images, &actual.images, "resources", "references")?;
    Ok(())
}

#[derive(Clone)]
pub struct AnchorPositions {
    pub body_pages: Vec<Vec<usize>>,
    pub table_fragments: Vec<(usize, Option<[f32; 4]>)>,
}
pub fn body_paths(doc: &Document) -> Vec<(usize, usize)> {
    doc.sections
        .iter()
        .enumerate()
        .flat_map(|(s, section)| {
            section
                .paragraphs
                .iter()
                .enumerate()
                .filter_map(move |(p, para)| {
                    let object = para.controls.iter().any(|control| match control {
                        Control::Table(_) | Control::Picture(_) => true,
                        Control::Generic(generic) => {
                            !generic.gso_shapes.is_empty()
                                || generic.equation.is_some()
                                || generic.container_box.is_some()
                        }
                        Control::SectionDef(_) => false,
                    });
                    (!atoms(para).is_empty() || object).then_some((s, p))
                })
        })
        .collect()
}
pub fn anchor_positions(
    doc: &Document,
    map: &SegmentMap,
    selected: &[(usize, usize)],
) -> Result<AnchorPositions, ContractViolation> {
    if map.truncated {
        return Err(condition("anchors", "truncated", "selection"));
    }
    if selected.is_empty() {
        return Err(condition("anchors", "empty_selection", "selection"));
    }
    let mut body_pages = Vec::new();
    for &(section, paragraph) in selected {
        let Some(para) = doc
            .sections
            .get(section)
            .and_then(|s| s.paragraphs.get(paragraph))
        else {
            return Err(condition("anchors", "address", "selection"));
        };
        let id = hwp_render::segment_id::paragraph_id(
            &hwp_render::segment_id::SegmentPath {
                section,
                indices: vec![paragraph],
            },
            para,
        );
        let rows: Vec<_> = map
            .rows
            .iter()
            .filter(|row| row.id == id)
            .map(|row| row.page)
            .collect();
        if rows.is_empty() {
            return Err(condition("anchors", "missing", "selection"));
        }
        body_pages.push(rows);
    }
    let table_fragments: Vec<_> = map
        .rows
        .iter()
        .filter(|row| row.kind == "table")
        .map(|row| (row.page, row.bbox.map(|b| [b.x0, b.y0, b.x1, b.y1])))
        .collect();
    if body_pages.iter().all(Vec::is_empty) && table_fragments.is_empty() {
        return Err(condition("anchors", "empty_evidence", "selection"));
    }
    Ok(AnchorPositions {
        body_pages,
        table_fragments,
    })
}
/// Relative evidence under one font environment; no fixed page-count or glyph golden.
pub fn compare_anchor_positions(
    expected: &AnchorPositions,
    actual: &AnchorPositions,
) -> Result<(), ContractViolation> {
    same(
        &expected.body_pages,
        &actual.body_pages,
        "body_anchors",
        "pages",
    )?;
    same(
        &expected.table_fragments.len(),
        &actual.table_fragments.len(),
        "table_anchors",
        "count",
    )?;
    for (index, ((page, bbox), (other_page, other_bbox))) in expected
        .table_fragments
        .iter()
        .zip(&actual.table_fragments)
        .enumerate()
    {
        same(page, other_page, "table_anchors", &index.to_string())?;
        match (bbox, other_bbox) {
            (Some(a), Some(b))
                if a.iter()
                    .zip(b)
                    .all(|(a, b)| a.is_finite() && b.is_finite() && (a - b).abs() < 0.05) => {}
            (None, None) => return Err(condition("table_anchors", "missing_box", "fragments")),
            _ => return Err(violation("table_anchors", &index.to_string())),
        }
    }
    Ok(())
}

/// Observed font bytes and outcomes are separate from requested style family names.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct FontResolutionIdentity {
    pub requested: String,
    pub bold: bool,
    pub resolved: Option<String>,
    pub sha256: Option<String>,
    pub face_index: Option<u32>,
    pub outcome: String,
}
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct FontIdentity {
    pub entries: Vec<FontResolutionIdentity>,
}
impl FontIdentity {
    pub fn from_store(store: &hwp_render::FontStore) -> Self {
        Self {
            entries: store
                .resolutions
                .iter()
                .map(|entry| FontResolutionIdentity {
                    requested: entry.requested.clone(),
                    bold: entry.requested_bold,
                    resolved: entry.resolved.clone(),
                    sha256: entry.resolved_sha256.clone(),
                    face_index: entry.resolved_face_index,
                    outcome: format!("{:?}", entry.outcome),
                })
                .collect(),
        }
    }
}
pub fn compare_font_identity(
    expected: &FontIdentity,
    actual: &FontIdentity,
) -> Result<(), ContractViolation> {
    if expected.entries.is_empty() || actual.entries.is_empty() {
        return Err(condition("fonts", "missing_observation", "identity"));
    }
    same(expected, actual, "fonts", "resolved_identity")
}

fn string_atoms(text: &str) -> Vec<Atom> {
    text.chars()
        .map(|ch| match ch {
            '\n' => Atom::Control(hwp_model::ctrl_char::LINE_BREAK),
            '\t' => Atom::Control(hwp_model::ctrl_char::TAB),
            _ => Atom::Text(ch),
        })
        .collect()
}

/// Build an expected-value budget; this edits an expectation, never a document or native bytes.
/// Paragraph insertion and mixed-style replacement require an explicit expected document instead.
pub fn expect_cell_values(
    baseline: &FormProjection,
    changes: &[ExpectedCell],
) -> Result<FormProjection, ContractViolation> {
    if let Some(invalid) = baseline.invalid_references.first() {
        return Err(invalid.clone());
    }
    let mut expected = baseline.clone();
    let mut unique = BTreeMap::new();
    for change in changes {
        let key = (change.table, change.row, change.col);
        if unique.insert(key, ()).is_some() {
            return Err(condition(
                "expected_cells",
                "duplicate_target",
                &format!("{key:?}"),
            ));
        }
        let Some(cell) = expected.tables.get_mut(change.table).and_then(|table| {
            table
                .cells
                .iter_mut()
                .find(|cell| cell.row == change.row && cell.col == change.col)
        }) else {
            return Err(condition(
                "expected_cells",
                "unknown_target",
                &format!("{key:?}"),
            ));
        };
        if cell.values.iter().flatten().any(|atom|matches!(atom,Atom::Control(code) if *code!=hwp_model::ctrl_char::LINE_BREAK && *code!=hwp_model::ctrl_char::TAB)) {
            return Err(condition("expected_cells","control_rich_requires_expected_document",&format!("{key:?}")));
        }
        if cell.paragraph_keys.len() != change.paragraphs.len() {
            return Err(condition(
                "expected_cells",
                "paragraph_count_budget",
                &format!("{key:?}"),
            ));
        }
        for (paragraph_key, text) in cell.paragraph_keys.iter().zip(&change.paragraphs) {
            let para = expected
                .paragraphs
                .iter_mut()
                .find(|para| &para.key == paragraph_key)
                .unwrap();
            if para.style["runs"]
                .as_array()
                .is_some_and(|runs| runs.len() > 1)
            {
                return Err(condition(
                    "expected_cells",
                    "mixed_style_requires_expected_document",
                    paragraph_key,
                ));
            }
            para.values = string_atoms(text);
        }
        cell.paragraphs = change.paragraphs.clone();
        cell.values = cell
            .paragraph_keys
            .iter()
            .map(|key| {
                expected
                    .paragraphs
                    .iter()
                    .find(|para| &para.key == key)
                    .unwrap()
                    .values
                    .clone()
            })
            .collect();
    }
    Ok(expected)
}
