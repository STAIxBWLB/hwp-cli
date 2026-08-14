//! IR → HWP 5.0 파일 쓰기.
//!
//! 모든 방출 함수는 doc_info/body_text 파서와 **거울 대칭**이다 —
//! "알려진 prefix + tail" 규칙 덕에 hwp5에서 읽은 문서는 바이트
//! 수준으로 복원된다(단순 컨트롤 한정; gso 등 평탄화된 구조는 의미
//! 수준). ID_MAPPINGS 카운트는 테이블 길이에서 유도한다(수동 동기화
//! 금지) — 원본에 버전별 추가 카운트가 있으면 꼬리만 보존.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::Path;

use hwp_model::{
    BorderLine, Caption, CaptionDirection, CaptionSide, Cell, CharShape, Control, Document,
    FaceName, HwpChar, LANG_COUNT, Metadata, OpaqueRecord, ParaShape, Paragraph, Picture,
    PreservationCode, PreservationDisposition, PreservationResourceKind, RawEntry, Section,
    SectionDef, Style, Table, WriteReport,
};

use crate::codec::{ByteWriter, compress, decompress};
use crate::error::Result;
use crate::file_header::{FILE_HEADER_SIZE, FileHeader, HwpVersion};
use crate::record::{RecordNode, ScanMode, scan_stream, tag};

#[derive(Default)]
pub struct WriteOptions {
    /// PrvImage 스트림 내용 (PNG 권장 — 없으면 스트림 생략)
    pub prv_image: Option<Vec<u8>>,
    /// 줄 배치(PARA_LINE_SEG) 보존 여부.
    ///
    /// 한글은 줄 배치 캐시가 내용과 정합하지 않으면 "변조" 보안 경고를
    /// 띄운다 (한컴 공식: 내용 수정 시 제거 권장). 기본 false — 한글이
    /// 열 때 재계산한다. 무수정 바이트 왕복에만 true를 쓸 것.
    pub preserve_linesegs: bool,
    /// 편집된 hwp5 원본을 다시 쓸 때 참.
    ///
    /// hwp5 출신이라도 내용을 바꿨으면 합성 경로의 문단 불변식(문단끝 0x0d·
    /// nchars bit31=마지막 문단·빈 문단 PARA_TEXT 생략·줄 배치 방출)을 다시
    /// 세워야 한글이 수용한다. 단 그림은 이미 hwp5 도형 레코드를 가지므로
    /// 재합성(synthesize_pictures)은 **하지 않는다**(출처 기준으로만 수행).
    pub edited: bool,
}

#[derive(Debug)]
struct HwpStreamMaterialization {
    storages: BTreeSet<String>,
    streams: BTreeMap<String, Vec<u8>>,
    bin_stream_paths: BTreeMap<String, String>,
    report: WriteReport,
}

#[derive(Debug)]
struct HwpMutationPlan {
    metadata_changed: bool,
    header_changed: bool,
    changed_sections: BTreeSet<usize>,
    removed_sections: BTreeSet<usize>,
    changed_bins: BTreeSet<String>,
    removed_bins: BTreeSet<String>,
}

/// 문서를 HWP 5.0 파일로 저장한다. 경고(평탄화/드롭) 목록을 반환한다.
///
/// This is the legacy compatibility surface. New callers should use
/// [`write_document_with_report`] so preservation events remain typed rather
/// than being reconstructed from warning strings.
pub fn write_document(doc: &Document, path: &Path, opts: &WriteOptions) -> Result<Vec<String>> {
    write_document_with_report(doc, path, opts).map(WriteReport::into_legacy_warnings)
}

/// Writes an HWP 5.0 document and returns typed warnings and preservation events.
pub fn write_document_with_report(
    doc: &Document,
    path: &Path,
    opts: &WriteOptions,
) -> Result<WriteReport> {
    let materialization = materialize_document(doc, opts, true, false)?;
    write_materialized_document(path, &materialization)?;
    Ok(materialization.report)
}

/// Rewrites an HWP document by copying its immutable source container and
/// replacing only streams selected by a mutation plan derived from the
/// original and edited IR values.
///
/// A no-op is an exact file copy. Opaque streams, storages, directory metadata,
/// FileHeader, scripts, document options, history, and every unrelated binary
/// stream remain owned by the source container rather than being regenerated.
pub fn rewrite_document_with_report(
    source: &Path,
    original: &Document,
    edited: &Document,
    path: &Path,
    opts: &WriteOptions,
) -> Result<WriteReport> {
    if source_output_alias(source, path)? {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "source and output paths must be different".to_string(),
        ));
    }
    let source_document = crate::read_document(source)?.document;
    if source_document != *original {
        return Err(crate::error::Hwp5Error::SourceSnapshotMismatch);
    }
    validate_source_rewrite_contract(original, edited)?;

    if original == edited {
        std::fs::copy(source, path)?;
        return Ok(WriteReport::new());
    }

    let plan = HwpMutationPlan::derive(original, edited);
    let mut source_container = crate::Hwp5Container::open(source)?;
    let compressed = source_container.file_header().is_compressed();
    let source_bin_paths = source_container
        .list_streams()
        .into_iter()
        .filter_map(|entry| {
            let normalized = entry
                .path
                .strip_prefix("/BinData/")
                .map(normalized_bin_name);
            normalized.map(|name| (name, entry.path))
        })
        .collect::<BTreeMap<_, _>>();
    let source_section_roots = plan
        .changed_sections
        .iter()
        .filter(|index| **index < original.sections.len())
        .map(|index| {
            let stream_path = format!("/BodyText/Section{index}");
            let bytes = source_container.read_record_stream(&stream_path)?;
            let scan = scan_stream(&bytes, ScanMode::Strict)?;
            Ok((*index, scan.roots))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    drop(source_container);

    let mut materialization = materialize_document(edited, opts, compressed, true)?;
    preserve_unchanged_body_subtrees(
        original,
        edited,
        compressed,
        &source_section_roots,
        &mut materialization,
    )?;
    std::fs::copy(source, path)?;
    patch_source_container(path, &plan, &source_bin_paths, &materialization)?;
    Ok(materialization.report)
}

fn source_output_alias(source: &Path, output: &Path) -> Result<bool> {
    if source == output {
        return Ok(true);
    }
    let source_metadata = std::fs::metadata(source)?;
    let output_metadata = match std::fs::metadata(output) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(source_metadata.dev() == output_metadata.dev()
            && source_metadata.ino() == output_metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (source_metadata, output_metadata);
        Ok(std::fs::canonicalize(source)? == std::fs::canonicalize(output)?)
    }
}

/// Replaces regenerated, unchanged typed controls with their immutable source
/// subtrees. This keeps level-sensitive records such as a section definition's
/// CTRL_DATA/PAGE_DEF sequence byte-identical while still allowing the edited
/// paragraph records around them to change.
fn preserve_unchanged_body_subtrees(
    original: &Document,
    edited: &Document,
    compressed: bool,
    source_sections: &BTreeMap<usize, Vec<RecordNode>>,
    materialization: &mut HwpStreamMaterialization,
) -> Result<()> {
    for (index, source_roots) in source_sections {
        let stream_path = format!("/BodyText/Section{index}");
        let materialized = required_materialized_stream(materialization, &stream_path)?;
        let decoded = if compressed {
            decompress(materialized, &stream_path)?
        } else {
            materialized.to_vec()
        };
        let emitted_roots = scan_stream(&decoded, ScanMode::Strict)?.roots;
        let original_section = original.sections.get(*index).ok_or_else(|| {
            crate::error::Hwp5Error::SourceRewrite(format!(
                "source section {index} is missing from the original IR"
            ))
        })?;
        let edited_section = edited.sections.get(*index).ok_or_else(|| {
            crate::error::Hwp5Error::SourceRewrite(format!(
                "changed section {index} is missing from the edited IR"
            ))
        })?;
        let merged = merge_section_roots(
            source_roots,
            &emitted_roots,
            original_section,
            edited_section,
        )?;
        materialization.streams.insert(
            stream_path,
            encode_compressed_stream(RecordNode::serialize_forest(&merged), compressed),
        );
    }
    Ok(())
}

fn merge_section_roots(
    source_roots: &[RecordNode],
    emitted_roots: &[RecordNode],
    original: &Section,
    edited: &Section,
) -> Result<Vec<RecordNode>> {
    let source_paragraphs = source_roots
        .iter()
        .filter(|node| node.tag == tag::PARA_HEADER)
        .collect::<Vec<_>>();
    let emitted_paragraphs = emitted_roots
        .iter()
        .filter(|node| node.tag == tag::PARA_HEADER)
        .collect::<Vec<_>>();
    if source_paragraphs.len() != original.paragraphs.len()
        || emitted_paragraphs.len() != edited.paragraphs.len()
    {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "paragraph records cannot be matched to the source and edited IR".to_string(),
        ));
    }

    let matches = match_paragraphs(&original.paragraphs, &edited.paragraphs);
    if original.extras == edited.extras
        && source_paragraphs.len() == emitted_paragraphs.len()
        && matches
            .iter()
            .enumerate()
            .all(|(edited_index, matched)| *matched == Some(edited_index))
    {
        let mut paragraph_index = 0usize;
        return Ok(source_roots
            .iter()
            .map(|source_node| {
                if source_node.tag != tag::PARA_HEADER {
                    return source_node.clone();
                }
                let merged = merge_paragraph_node(
                    source_node,
                    emitted_paragraphs[paragraph_index],
                    &original.paragraphs[paragraph_index],
                    &edited.paragraphs[paragraph_index],
                );
                paragraph_index += 1;
                merged
            })
            .collect());
    }

    let mut roots = Vec::with_capacity(emitted_roots.len());
    for (edited_index, emitted_node) in emitted_paragraphs.iter().enumerate() {
        if let Some(original_index) = matches[edited_index] {
            roots.push(merge_paragraph_node(
                source_paragraphs[original_index],
                emitted_node,
                &original.paragraphs[original_index],
                &edited.paragraphs[edited_index],
            ));
        } else {
            roots.push((*emitted_node).clone());
        }
    }

    if original.extras == edited.extras {
        roots.extend(
            source_roots
                .iter()
                .filter(|node| node.tag != tag::PARA_HEADER)
                .cloned(),
        );
    } else {
        roots.extend(
            emitted_roots
                .iter()
                .filter(|node| node.tag != tag::PARA_HEADER)
                .cloned(),
        );
    }
    Ok(roots)
}

fn match_paragraphs(original: &[Paragraph], edited: &[Paragraph]) -> Vec<Option<usize>> {
    let mut used = vec![false; original.len()];
    let mut matches = vec![None; edited.len()];
    for (edited_index, paragraph) in edited.iter().enumerate() {
        let instance_id = paragraph.header.instance_id;
        if instance_id == 0
            || edited
                .iter()
                .filter(|candidate| candidate.header.instance_id == instance_id)
                .count()
                != 1
        {
            continue;
        }
        let candidates = original
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                !used[*index] && candidate.header.instance_id == instance_id
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            used[candidates[0]] = true;
            matches[edited_index] = Some(candidates[0]);
        }
    }
    for (edited_index, paragraph) in edited.iter().enumerate() {
        if matches[edited_index].is_some() {
            continue;
        }
        if let Some((original_index, _)) = original
            .iter()
            .enumerate()
            .find(|(index, candidate)| !used[*index] && *candidate == paragraph)
        {
            used[original_index] = true;
            matches[edited_index] = Some(original_index);
        }
    }
    if original.len() == edited.len() {
        for edited_index in 0..edited.len() {
            if matches[edited_index].is_none() && !used[edited_index] {
                used[edited_index] = true;
                matches[edited_index] = Some(edited_index);
            }
        }
    }
    matches
}

fn merge_paragraph_node(
    source: &RecordNode,
    emitted: &RecordNode,
    original: &Paragraph,
    edited: &Paragraph,
) -> RecordNode {
    if original == edited {
        return source.clone();
    }
    let source_controls = source
        .children
        .iter()
        .filter(|node| node.tag == tag::CTRL_HEADER)
        .collect::<Vec<_>>();
    let control_matches = match_controls(&original.controls, &edited.controls);
    let mut merged = emitted.clone();
    let mut edited_control_index = 0usize;
    for child in &mut merged.children {
        if child.tag != tag::CTRL_HEADER {
            continue;
        }
        if let Some(Some(original_control_index)) = control_matches.get(edited_control_index)
            && let (Some(source_control), Some(original_control), Some(edited_control)) = (
                source_controls.get(*original_control_index),
                original.controls.get(*original_control_index),
                edited.controls.get(edited_control_index),
            )
        {
            *child = merge_control_node(source_control, child, original_control, edited_control);
        }
        edited_control_index += 1;
    }
    merged
}

fn match_controls(original: &[Control], edited: &[Control]) -> Vec<Option<usize>> {
    let mut used = vec![false; original.len()];
    let mut matches = vec![None; edited.len()];
    for (edited_index, control) in edited.iter().enumerate() {
        if edited
            .iter()
            .filter(|candidate| *candidate == control)
            .count()
            != 1
        {
            continue;
        }
        let candidates = original
            .iter()
            .enumerate()
            .filter(|(index, candidate)| !used[*index] && *candidate == control)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            used[candidates[0]] = true;
            matches[edited_index] = Some(candidates[0]);
        }
    }
    if original.len() == edited.len() {
        for edited_index in 0..edited.len() {
            if matches[edited_index].is_none() && !used[edited_index] {
                used[edited_index] = true;
                matches[edited_index] = Some(edited_index);
            }
        }
    }
    matches
}

fn merge_control_node(
    source: &RecordNode,
    emitted: &RecordNode,
    original: &Control,
    edited: &Control,
) -> RecordNode {
    if original == edited {
        return source.clone();
    }
    match (original, edited) {
        (Control::Table(original), Control::Table(edited)) => {
            merge_table_control(source, emitted, original, edited)
        }
        _ => emitted.clone(),
    }
}

fn merge_table_control(
    source: &RecordNode,
    emitted: &RecordNode,
    original: &Table,
    edited: &Table,
) -> RecordNode {
    let mut original_structure = original.clone();
    let mut edited_structure = edited.clone();
    clear_table_paragraphs(&mut original_structure);
    clear_table_paragraphs(&mut edited_structure);
    let original_paragraphs = table_paragraphs(original);
    let edited_paragraphs = table_paragraphs(edited);
    if original_structure != edited_structure
        || original_paragraphs.len() != edited_paragraphs.len()
    {
        return emitted.clone();
    }

    let source_indices = source
        .children
        .iter()
        .enumerate()
        .filter(|(_, node)| node.tag == tag::PARA_HEADER)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let emitted_paragraphs_nodes = emitted
        .children
        .iter()
        .filter(|node| node.tag == tag::PARA_HEADER)
        .collect::<Vec<_>>();
    if source_indices.len() != original_paragraphs.len()
        || emitted_paragraphs_nodes.len() != edited_paragraphs.len()
    {
        return emitted.clone();
    }

    let mut merged = source.clone();
    for index in 0..original_paragraphs.len() {
        if original_paragraphs[index] != edited_paragraphs[index] {
            let child_index = source_indices[index];
            merged.children[child_index] = merge_paragraph_node(
                &source.children[child_index],
                emitted_paragraphs_nodes[index],
                original_paragraphs[index],
                edited_paragraphs[index],
            );
        }
    }
    merged
}

fn clear_table_paragraphs(table: &mut Table) {
    if let Some(caption) = &mut table.caption {
        caption.paragraphs.clear();
    }
    for cell in &mut table.cells {
        cell.paragraphs.clear();
    }
}

fn table_paragraphs(table: &Table) -> Vec<&Paragraph> {
    let mut paragraphs = table
        .caption
        .iter()
        .flat_map(|caption| caption.paragraphs.iter())
        .collect::<Vec<_>>();
    paragraphs.extend(table.cells.iter().flat_map(|cell| cell.paragraphs.iter()));
    paragraphs
}

fn materialize_document(
    doc: &Document,
    opts: &WriteOptions,
    compressed: bool,
    source_preserving: bool,
) -> Result<HwpStreamMaterialization> {
    let mut report = WriteReport::new();
    let input_bin_names = doc
        .bin_streams
        .iter()
        .map(|stream| normalized_bin_name(&stream.name))
        .collect::<Vec<_>>();

    // hwpx 출신 문서 정규화: hwp5 레코드(SHAPE_COMPONENT)가 없는 그림은
    // 쓸 수 없으므로 컨트롤과 확장 문자를 동기 제거한다
    //
    // synthesize = 문단 불변식·줄 배치를 다시 세워야 하는 경로(hwpx/md 출신 또는
    // 편집된 hwp5). synth_pictures = hwpx/md 출신 그림을 hwp5 도형으로 합성해야
    // 하는 경로(출처 기준 — 편집된 hwp5 그림은 이미 도형 레코드를 가지므로 제외).
    // 단 편집으로 새로 삽입된 그림(extras 빈 Picture)은 출처와 무관하게 합성해야
    // strip_unwritable_pictures에 드롭되지 않는다.
    let synth_pictures = doc.meta.source_format != "hwp5" || has_synthesizable_picture(doc);
    // hwpx-출신 구조화 도형/글상자(gso_shapes)는 hwp5 gso 레코드로 역합성해야
    // strip에 드롭되지 않는다(그림 합성과 동일 게이트 논리).
    let synth_gso = doc.meta.source_format != "hwp5" || has_synthesizable_gso(doc);
    let synthesize = synth_pictures || synth_gso || opts.edited;
    let normalized;
    let doc = if needs_normalize(doc) || synthesize {
        let mut d = doc.clone();
        // hwpx/md 출신 이미지에 hwp5 도형 레코드(SHAPE_COMPONENT + 그림)를 먼저
        // 합성한다 — 합성되면 extras가 채워져 아래 strip이 드롭하지 않는다.
        if synth_pictures {
            synthesize_pictures(&mut d, &mut report);
        }
        if synth_gso {
            degrade_hwpx_gso(&mut d, &mut report);
        }
        for section in &mut d.sections {
            for para in &mut section.paragraphs {
                strip_unwritable_pictures(para, &mut report);
            }
        }
        // 합성 5.1.x 문서: 줄 배치(PARA_LINE_SEG)를 합성한다. 5.1.x는 줄 배치
        // 캐시가 없으면 한글이 본문을 0높이로 그려 '검은 바'/'빈 내용', 심하면
        // '손상'으로 판정한다(실기 13차 확정 — bit31 클리어 시 전부 손상,
        // work_rt 5.0.2.4는 재계산하나 5.1.x 정품은 본문 문단 lineseg 100% 보유).
        if synthesize {
            // 줄 배치(PARA_LINE_SEG)는 hwp-cli가 hwp-render의 폰트 셰이핑으로
            // 정확히 계산해 IR(para.line_segs)에 미리 채운다(hwp5는 hwp-render를
            // 역참조할 수 없으므로). 여기서는 채워진 줄 배치를 방출만 한다.
            // nchars bit31 = '리스트의 마지막 문단' 표식: 각 리스트(섹션/표 셀/
            // 글상자)의 마지막 문단만 세팅한다(정품 다문단 5.1.1.0 실측).
            for section in &mut d.sections {
                set_last_para_flag(&mut section.paragraphs);
            }
            // hwpx 경유 변환은 ParaShape.attr1(한글 줄나눔·줄격자 비트)·
            // line_spacing_old·border_fill_id 와 구역 첫 문단 break_type=0x03 을
            // 왕복에서 잃는다(hwpx writer/reader가 직렬화/복원하지 않음). 합성
            // 경로에서만 정상 표본 기준값을 보정 주입한다 — hwp5 무수정 왕복
            // (synthesize=false)은 거치지 않으므로 바이트 동일 게이트 무영향.
            if !source_preserving {
                ensure_para_shape_defaults(&mut d.header);
            }
            for section in &mut d.sections {
                if let Some(p) = section.paragraphs.first_mut() {
                    p.header.break_type |= 0x03;
                }
            }
        }
        normalized = d;
        &normalized
    } else {
        doc
    };

    // 레코드 스트림 구성
    let doc_info_nodes = emit_doc_info(doc, &mut report);
    let doc_info = RecordNode::serialize_forest(&doc_info_nodes);
    // PARA_HEADER 꼬리 게이트: 5.0.3.2 이상은 '변경추적 병합 문단여부' UINT16이
    // 필수다(스펙 표 58, 전체 길이 24B). 합성 문단(tail 비어 있음)에만 적용해
    // 24B를 맞춘다. 22B만 쓰면 한글이 버전-레이아웃 불일치로 '손상/변조' 경고
    // (sample_m6 5.1.0.1·halla 5.1.1.0 실증). pre-5.0.3.2(work_report 5.0.2.4)는
    // 22B가 정답이므로 게이트 false.
    let add_tracking_tail = parse_version(&doc.meta.source_version).to_u32() >= 0x05_00_03_02;
    // 문단 고유 ID 카운터 — 합성 문단(instance_id=0)에 non-zero 유니크 값 부여.
    // 한글은 instance_id=0을 비정상으로 보고 '손상/변조' 판정(표본은 전부 non-zero).
    // hwp5 원본 왕복은 원본 instance_id(0 포함)를 보존해야 바이트 동일하므로 제외.
    let mut inst_counter = 0x1000_0000u32;
    let sections: Vec<Vec<u8>> = doc
        .sections
        .iter()
        .map(|s| {
            let mut roots = emit_section(
                s,
                synthesize,
                opts.preserve_linesegs,
                add_tracking_tail,
                &mut report,
            );
            if synthesize {
                assign_instance_ids(&mut roots, &mut inst_counter);
            }
            RecordNode::serialize_forest(&roots)
        })
        .collect();

    // FileHeader
    let header = FileHeader {
        version: parse_version(&doc.meta.source_version),
        attributes: 0x1, // 압축
        license: 0,
        // 비암호 문서라도 현대 한글(2010+)은 EncryptVersion=4(글 7.0+)를
        // 무조건 쓴다. fixtures/hwp5 표본 6개 전부 attr1 암호화 bit(=bit1)는
        // 0인데 encver=4. 0을 쓰면 한글이 '손상/변조'로 거부한다(실기 게이트).
        encrypt_version: 4,
        kogl_country: 0,
        reserved: [0u8; FILE_HEADER_SIZE - 49],
    };

    // 미리보기 텍스트 (UTF-16LE, 약 1000자)
    let mut preview = doc.plain_text();
    preview.truncate(
        preview
            .char_indices()
            .nth(1000)
            .map_or(preview.len(), |(i, _)| i),
    );
    let prv_text: Vec<u8> = preview.encode_utf16().flat_map(u16::to_le_bytes).collect();

    let mut storages = BTreeSet::from(["/BodyText".to_string()]);
    let mut streams = BTreeMap::new();
    streams.insert("/FileHeader".to_string(), header.serialize());
    streams.insert(
        "/DocInfo".to_string(),
        encode_compressed_stream(doc_info, compressed),
    );
    for (i, body) in sections.iter().enumerate() {
        streams.insert(
            format!("/BodyText/Section{i}"),
            encode_compressed_stream(body.clone(), compressed),
        );
    }
    // BIN_DATA 테이블이 참조하는 스트림만 동봉 (hwp5 명명 규칙)
    let referenced: Vec<String> = doc
        .header
        .bin_data
        .iter()
        .filter_map(|item| {
            let id = item.storage_id?;
            let ext = item.extension.as_deref().unwrap_or("");
            Some(format!("BIN{id:04X}.{ext}"))
        })
        .collect();
    let mut bin_written = 0usize;
    let mut bin_stream_paths = BTreeMap::new();
    if !referenced.is_empty() && !doc.bin_streams.is_empty() {
        storages.insert("/BinData".to_string());
        for (index, bin) in doc.bin_streams.iter().enumerate() {
            let base = bin.name.rsplit('/').next().unwrap_or(&bin.name);
            if referenced.iter().any(|r| r.eq_ignore_ascii_case(base)) {
                let stream_path = format!("/BinData/{base}");
                streams.insert(
                    stream_path.clone(),
                    encode_compressed_stream(bin.data.clone(), compressed),
                );
                bin_stream_paths.insert(normalized_bin_name(base), stream_path.clone());
                if let Some(input_name) = input_bin_names.get(index) {
                    bin_stream_paths.insert(input_name.clone(), stream_path);
                }
                bin_written += 1;
            }
        }
    }
    if bin_written < doc.bin_streams.len() {
        report.warning(format!(
            "BinData {}개 중 {}개만 동봉 (hwp5 BIN_DATA 테이블이 참조하는 항목만)",
            doc.bin_streams.len(),
            bin_written
        ));
    }
    storages.insert("/DocOptions".to_string());
    streams.insert("/DocOptions/_LinkDoc".to_string(), vec![0u8; 524]);
    storages.insert("/Scripts".to_string());
    streams.insert(
        "/Scripts/JScriptVersion".to_string(),
        vec![
            0x63, 0x64, 0x80, 0x00, 0x00, 0xF7, 0xDF, 0x88, 0xA9, 0x08, 0x00, 0x00, 0x00,
        ],
    );
    streams.insert(
        "/Scripts/DefaultJScript".to_string(),
        vec![
            0x63, 0x60, 0x40, 0x05, 0xFF, 0x81, 0x00, 0x00, 0x6E, 0xBB, 0x6E, 0xD1, 0x14, 0x00,
            0x00, 0x00,
        ],
    );
    streams.insert(
        "/\u{5}HwpSummaryInformation".to_string(),
        hwp_summary_information(&doc.metadata),
    );
    streams.insert("/PrvText".to_string(), prv_text);
    if let Some(img) = &opts.prv_image {
        streams.insert("/PrvImage".to_string(), img.clone());
    }
    for (path, bytes) in doc
        .hwp5_xml_template
        .iter()
        .chain(doc.hwp5_doc_history.iter())
    {
        if let Some((parent, _)) = path.rsplit_once('/')
            && !parent.is_empty()
        {
            storages.insert(parent.to_string());
        }
        streams.insert(path.clone(), bytes.clone());
    }

    Ok(HwpStreamMaterialization {
        storages,
        streams,
        bin_stream_paths,
        report,
    })
}

fn write_materialized_document(
    path: &Path,
    materialization: &HwpStreamMaterialization,
) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    let mut cfb = cfb::CompoundFile::create_with_version(cfb::Version::V3, file)?;
    for storage in &materialization.storages {
        cfb.create_storage_all(storage)?;
    }
    for (stream_path, bytes) in &materialization.streams {
        if let Some((parent, _)) = stream_path.rsplit_once('/')
            && !parent.is_empty()
        {
            cfb.create_storage_all(parent)?;
        }
        cfb.create_new_stream(stream_path)?.write_all(bytes)?;
    }

    const SECONDS_FROM_CFB_TO_UNIX_EPOCH: u64 = 11_644_473_600;
    let cfb_epoch = std::time::UNIX_EPOCH
        .checked_sub(std::time::Duration::from_secs(
            SECONDS_FROM_CFB_TO_UNIX_EPOCH,
        ))
        .expect("CFB epoch는 지원되는 SystemTime 범위");
    let entry_paths = cfb
        .walk()
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();
    for entry_path in entry_paths {
        cfb.set_created_time(&entry_path, cfb_epoch)?;
        cfb.set_modified_time(&entry_path, cfb_epoch)?;
    }
    cfb.flush()?;
    Ok(())
}

fn encode_compressed_stream(bytes: Vec<u8>, compressed: bool) -> Vec<u8> {
    if compressed { compress(&bytes) } else { bytes }
}

fn validate_source_rewrite_contract(original: &Document, edited: &Document) -> Result<()> {
    if original.meta.source_format != "hwp5" || edited.meta.source_format != "hwp5" {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "source-preserving rewrite requires HWP5 IR".to_string(),
        ));
    }
    if original.meta != edited.meta {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "source format and version cannot be changed by a native rewrite".to_string(),
        ));
    }
    if original.hwpx_settings_xml != edited.hwpx_settings_xml
        || original.hwpx_version_xml != edited.hwpx_version_xml
        || original.hwpx_preview_image != edited.hwpx_preview_image
        || original.hwp5_xml_template != edited.hwp5_xml_template
        || original.hwp5_doc_history != edited.hwp5_doc_history
    {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "opaque ancillary payloads are not editable through the HWP IR writer".to_string(),
        ));
    }
    ensure_unique_bin_names(original)?;
    ensure_unique_bin_names(edited)?;
    Ok(())
}

fn ensure_unique_bin_names(document: &Document) -> Result<()> {
    let mut names = BTreeSet::new();
    for stream in &document.bin_streams {
        let name = normalized_bin_name(&stream.name);
        if !names.insert(name) {
            return Err(crate::error::Hwp5Error::SourceRewrite(
                "duplicate case-insensitive BinData names cannot be rewritten safely".to_string(),
            ));
        }
    }
    Ok(())
}

impl HwpMutationPlan {
    fn derive(original: &Document, edited: &Document) -> Self {
        let changed_sections = edited
            .sections
            .iter()
            .enumerate()
            .filter(|(index, section)| original.sections.get(*index) != Some(*section))
            .map(|(index, _)| index)
            .collect();
        let removed_sections = (edited.sections.len()..original.sections.len()).collect();

        let original_bins = document_bins(original);
        let edited_bins = document_bins(edited);
        let changed_bins: BTreeSet<String> = edited_bins
            .iter()
            .filter(|(name, stream)| original_bins.get(*name) != Some(stream))
            .map(|(name, _)| name.clone())
            .collect();
        let removed_bins: BTreeSet<String> = original_bins
            .keys()
            .filter(|name| !edited_bins.contains_key(*name))
            .cloned()
            .collect();
        let bin_relationships_changed = original_bins.keys().ne(edited_bins.keys());
        let header_changed = original.header != edited.header
            || original.sections.len() != edited.sections.len()
            || bin_relationships_changed;

        Self {
            metadata_changed: original.metadata != edited.metadata,
            header_changed,
            changed_sections,
            removed_sections,
            changed_bins,
            removed_bins,
        }
    }
}

fn document_bins(document: &Document) -> BTreeMap<String, &hwp_model::BinStream> {
    document
        .bin_streams
        .iter()
        .map(|stream| (normalized_bin_name(&stream.name), stream))
        .collect()
}

fn normalized_bin_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase()
}

fn patch_source_container(
    path: &Path,
    plan: &HwpMutationPlan,
    source_bin_paths: &BTreeMap<String, String>,
    materialization: &HwpStreamMaterialization,
) -> Result<()> {
    let mut cfb = cfb::open_rw(path)?;

    if plan.metadata_changed {
        replace_cfb_stream(
            &mut cfb,
            "/\u{5}HwpSummaryInformation",
            required_materialized_stream(materialization, "/\u{5}HwpSummaryInformation")?,
        )?;
    }
    if plan.header_changed {
        replace_cfb_stream(
            &mut cfb,
            "/DocInfo",
            required_materialized_stream(materialization, "/DocInfo")?,
        )?;
    }
    for index in &plan.changed_sections {
        let stream_path = format!("/BodyText/Section{index}");
        replace_cfb_stream(
            &mut cfb,
            &stream_path,
            required_materialized_stream(materialization, &stream_path)?,
        )?;
    }
    for index in &plan.removed_sections {
        let stream_path = format!("/BodyText/Section{index}");
        if cfb.is_stream(&stream_path) {
            cfb.remove_stream(&stream_path)?;
        }
    }

    for normalized_name in &plan.changed_bins {
        let stream_path = materialization
            .bin_stream_paths
            .get(normalized_name)
            .ok_or_else(|| {
                crate::error::Hwp5Error::SourceRewrite(
                    "a changed BinData item is not representable in the materialized document"
                        .to_string(),
                )
            })?;
        let bytes = required_materialized_stream(materialization, stream_path)?;
        replace_cfb_stream(&mut cfb, stream_path, bytes)?;
    }
    for normalized_name in &plan.removed_bins {
        if let Some(stream_path) = source_bin_paths.get(normalized_name)
            && cfb.is_stream(stream_path)
        {
            cfb.remove_stream(stream_path)?;
        }
    }

    if !plan.changed_sections.is_empty() || !plan.removed_sections.is_empty() {
        replace_cfb_stream(
            &mut cfb,
            "/PrvText",
            required_materialized_stream(materialization, "/PrvText")?,
        )?;
        if let Some(preview) = materialization.streams.get("/PrvImage") {
            replace_cfb_stream(&mut cfb, "/PrvImage", preview)?;
        }
    }
    cfb.flush()?;
    Ok(())
}

fn required_materialized_stream<'a>(
    materialization: &'a HwpStreamMaterialization,
    path: &str,
) -> Result<&'a [u8]> {
    materialization
        .streams
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| crate::error::Hwp5Error::StreamNotFound(path.to_string()))
}

fn replace_cfb_stream(
    cfb: &mut cfb::CompoundFile<std::fs::File>,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    if let Some((parent, _)) = path.rsplit_once('/')
        && !parent.is_empty()
    {
        cfb.create_storage_all(parent)?;
    }
    cfb.create_stream(path)?.write_all(bytes)?;
    Ok(())
}

/// PARA_HEADER instance_id(offset 18~22)가 0이면 유니크 non-zero 값을 부여.
/// 레코드 트리를 재귀 순회 — 표 셀/글상자 안 문단도 포함.
fn assign_instance_ids(roots: &mut [RecordNode], counter: &mut u32) {
    for node in roots {
        if node.tag == tag::PARA_HEADER && node.data.len() >= 22 {
            let inst =
                u32::from_le_bytes([node.data[18], node.data[19], node.data[20], node.data[21]]);
            if inst == 0 {
                *counter = counter.wrapping_add(1);
                node.data[18..22].copy_from_slice(&counter.to_le_bytes());
            }
        }
        assign_instance_ids(&mut node.children, counter);
    }
}

/// 리스트(문단 목록)의 마지막 문단에만 nchars bit31(chars_flags 0x80)을 세팅한다.
/// 정품 다문단(5.1.1.0): 섹션·표 셀·글상자의 마지막 문단만 세팅, 나머지 0.
/// 표 셀·글상자 안 문단 리스트도 재귀 처리한다.
fn set_last_para_flag(paras: &mut [Paragraph]) {
    let n = paras.len();
    for (i, p) in paras.iter_mut().enumerate() {
        if i + 1 == n {
            p.header.chars_flags |= 0x80;
        } else {
            p.header.chars_flags &= !0x80;
        }
        for ctrl in &mut p.controls {
            match ctrl {
                Control::Table(t) => {
                    for cell in &mut t.cells {
                        set_last_para_flag(&mut cell.paragraphs);
                    }
                }
                Control::Generic(g) => {
                    for list in &mut g.paragraph_lists {
                        set_last_para_flag(&mut list.paragraphs);
                    }
                }
                _ => {}
            }
        }
    }
}

/// 합성(md/hwpx 출신) 문서의 ParaShape에 누락 기준값만 보정한다.
/// (attr1 의 줄나눔·줄격자 비트는 hwpx reader가 실제 값으로 채우므로 강제하지
/// 않는다 — 강제하면 BREAK_WORD 문단까지 KEEP_WORD가 돼 줄바꿈이 느슨해지고
/// 페이지가 밀린다. markdown은 from_markdown이 attr1을 직접 설정.)
fn ensure_para_shape_defaults(header: &mut hwp_model::DocHeader) {
    for ps in &mut header.para_shapes {
        if ps.line_spacing_old == 0 {
            ps.line_spacing_old = 160;
        }
        if ps.border_fill_id == 0 {
            ps.border_fill_id = 2;
        }
    }
}

/// hwp5로 쓸 수 없는 그림(SHAPE_COMPONENT 레코드 부재)이 있는지.
fn needs_normalize(doc: &Document) -> bool {
    fn para_has(para: &Paragraph) -> bool {
        para.controls.iter().any(|c| match c {
            Control::Picture(p) => p.extras.is_empty(),
            Control::Table(t) => t.cells.iter().flat_map(|c| &c.paragraphs).any(para_has),
            // hwp5 출신(raw_children 보존)은 원본 트리를 그대로 방출하므로
            // 정규화 불필요. raw_children가 없는데 data도 없는 컨트롤만
            // (hwpx/md 출신 합성 불가) 드롭 대상.
            Control::Generic(g) => {
                (g.data.is_empty() && g.ctrl_id != *b"cold" && g.raw_children.is_empty())
                    || g.paragraph_lists
                        .iter()
                        .flat_map(|l| &l.paragraphs)
                        .any(para_has)
            }
            _ => false,
        })
    }
    doc.sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .any(para_has)
}

// ── hwpx/md 출신 이미지의 hwp5 도형 레코드 합성 ───────────────────────────
//
// hwpx의 `<hp:pic>`은 IR Picture(extras 비어 있음)로 읽힌다. hwp5는 그림을
// gso CTRL_HEADER → SHAPE_COMPONENT → SHAPE_COMPONENT_PICTURE 트리 + BIN_DATA
// 항목 + BinData 스트림으로 저장한다. 정품 work_report.hwp의 이미지 레코드를
// 템플릿으로 쓰고 크기·모서리·BinItem ID만 패치한다(단위 변환행렬은 크기
// 무관). 합성 후 extras가 채워져 strip이 드롭하지 않는다.

/// 머리말/꼬리말(head/foot) LIST_HEADER 34B 템플릿 (정품 work_report.hwp foot).
/// paraCount@0(u16)만 실제 문단 수로 패치. hwpx 출신 머리말은 LIST_HEADER가
/// 비어 있어 이 템플릿으로 채워야 한글이 머리말(과 내부 로고)을 그린다.
const HEADER_LIST_HEADER_TEMPLATE: &str =
    "01000000400000003ebc0000130b0000000000000000000000000000000000000000";
/// SHAPE_COMPONENT 196B. width@20/28, height@24/32.
const SHAPE_COMPONENT_TEMPLATE: &str = "636970246369702400000000000000000000010038220000ec04000038220000ec0400000000082400001c110000760200000100000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000";
fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("템플릿 hex"))
        .collect()
}

/// 임베드 이미지 바이트에서 픽셀 크기(가로,세로)를 읽는다. PNG/JPEG/GIF/BMP.
/// 그림의 자르기(clip)·picture_effect에 들어가는 "원본 자연 크기" 계산용.
fn image_pixel_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") {
        // IHDR: width@16, height@20 (big-endian)
        let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if data.len() >= 10 && data.starts_with(b"GIF") {
        // 논리 화면 width@6, height@8 (little-endian u16)
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }
    if data.len() >= 26 && data.starts_with(b"BM") {
        // BITMAPINFOHEADER: width@18, height@22 (little-endian i32)
        let w = i32::from_le_bytes(data[18..22].try_into().ok()?);
        let h = i32::from_le_bytes(data[22..26].try_into().ok()?);
        return Some((w.unsigned_abs(), h.unsigned_abs()));
    }
    if data.len() >= 4 && data[0] == 0xFF && data[1] == 0xD8 {
        // JPEG: SOFn 마커(C0~CF, 단 C4/C8/CC 제외)에서 height/width(big-endian)
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC
            {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            if seg_len < 2 {
                break;
            }
            i += 2 + seg_len;
        }
        return None;
    }
    None
}

/// 픽셀 크기 → HWPUNIT 자연 크기(96 DPI 기준, px × 7200/96 = px × 75).
fn natural_size_hwpunit(px: (u32, u32)) -> (i32, i32) {
    let conv = |v: u32| (v as i64 * 7200 / 96) as i32;
    (conv(px.0), conv(px.1))
}

/// 정품 템플릿을 크기·BinItem ID로 패치해 그림 extras(SHAPE_COMPONENT +
/// 그 자식 SHAPE_COMPONENT_PICTURE)를 만든다.
///
/// - `width`/`height`: 표시 크기(HWPUNIT). gso 박스·도형 최종 크기.
/// - `natural`: 원본 이미지 자연 크기(픽셀×7200/96). 그림 자르기(clip)
///   사각형 = (0,0,natural) — **전체 이미지를 표시**한다. 표시 크기를 자르기로
///   쓰면 원본의 좌상단 일부만 잘려 보여(예: 8196/150000 ≈ 5%) 한글에서
///   그림이 거의 안 보인다(정품 한글은 항상 자연 크기를 쓴다).
/// - `bin_id`: BIN_DATA 항목 ID. `pic_inst`: 그림 개체 유니크 instance_id.
fn build_picture_extras(
    width: i32,
    height: i32,
    natural: (i32, i32),
    bin_id: u16,
    pic_inst: u32,
) -> Vec<OpaqueRecord> {
    let w = width.to_le_bytes();
    let h = height.to_le_bytes();
    let (nw, nh) = natural;
    let mut sc = hex_to_bytes(SHAPE_COMPONENT_TEMPLATE);
    sc[20..24].copy_from_slice(&w); // 폭(최종)
    sc[24..28].copy_from_slice(&h); // 높이(최종)
    sc[28..32].copy_from_slice(&w); // 폭(초기) — 단위행렬이라 최종과 동일
    sc[32..36].copy_from_slice(&h); // 높이(초기)

    // SHAPE_COMPONENT_PICTURE 91B (정품 5.1.x 레이아웃):
    // 테두리(12) + 표시 사각형 4꼭지점(32) + 자르기(16: 0,0,natural) +
    // 안쪽여백(8) + 그림(5: 밝기,명암,효과,BinItem ID) + 테두리투명(1) +
    // instance_id(4) + picture_effect(13: flags4=0 + natural_w4 + natural_h4 + 1)
    let mut pic = Vec::with_capacity(91);
    pic.extend_from_slice(&[0u8; 12]); // 테두리
    for &(x, y) in &[(0, 0), (width, 0), (width, height), (0, height)] {
        pic.extend_from_slice(&i32::to_le_bytes(x));
        pic.extend_from_slice(&i32::to_le_bytes(y));
    }
    pic.extend_from_slice(&0i32.to_le_bytes()); // 자르기 좌
    pic.extend_from_slice(&0i32.to_le_bytes()); // 자르기 상
    pic.extend_from_slice(&nw.to_le_bytes()); // 자르기 우 = 자연 폭
    pic.extend_from_slice(&nh.to_le_bytes()); // 자르기 하 = 자연 높이
    pic.extend_from_slice(&[0u8; 8]); // 안쪽 여백
    pic.push(0); // 밝기
    pic.push(0); // 명암
    pic.push(0); // 효과
    pic.extend_from_slice(&bin_id.to_le_bytes()); // BinItem ID
    pic.push(0); // 테두리 투명도
    pic.extend_from_slice(&pic_inst.to_le_bytes()); // instance_id(유니크)
    pic.extend_from_slice(&0u32.to_le_bytes()); // picture_effect flags=0(효과 없음)
    pic.extend_from_slice(&nw.to_le_bytes()); // picture_effect: 자연 폭
    pic.extend_from_slice(&nh.to_le_bytes()); // picture_effect: 자연 높이
    pic.push(0); // picture_effect reserved

    vec![OpaqueRecord {
        tag: tag::SHAPE_COMPONENT,
        data: sc,
        children: vec![OpaqueRecord {
            tag: tag::SHAPE_COMPONENT_PICTURE,
            data: pic,
            children: Vec::new(),
        }],
    }]
}

/// Applies IR crop, brightness, contrast, and rotation to synthesized picture
/// extras. This runs only for newly materialized extras, such as HWPX-origin
/// pictures; original HWP5 extras remain on the lossless raw round-trip path.
/// [`is_materialized_generated_picture`] applies the same patch so post-write
/// verification compares the exact materialized representation.
fn patch_picture_extras_fx(
    extras: &mut [OpaqueRecord],
    crop: Option<[f32; 4]>,
    brightness: i8,
    contrast: i8,
    rotation: Option<f32>,
) {
    let default = crop.is_none() && brightness == 0 && contrast == 0 && rotation.is_none();
    if default {
        return;
    }
    let Some(sc) = extras.first_mut() else { return };
    if sc.tag != tag::SHAPE_COMPONENT {
        return;
    }
    // Replace the synthesized 196-byte template's rotation matrix at byte 148.
    if let Some(deg) = rotation
        && sc.data.len() >= 196
    {
        let t = (deg as f64).to_radians();
        let (sn, cs) = (t.sin(), t.cos());
        // [a b c; d e f], clockwise-positive in y-down coordinates. This is
        // the inverse convention used by gso_rotation_deg.
        for (o, v) in [(148, cs), (156, -sn), (172, sn), (180, cs)] {
            sc.data[o..o + 8].copy_from_slice(&v.to_le_bytes());
        }
    }
    let Some(p) = sc.children.first_mut() else {
        return;
    };
    if p.tag != tag::SHAPE_COMPONENT_PICTURE || p.data.len() < 73 {
        return;
    }
    if let Some([l, t, r, b]) = crop {
        for (o, v) in [(44, l), (48, t), (52, r), (56, b)] {
            p.data[o..o + 4].copy_from_slice(&(v as i32).to_le_bytes());
        }
    }
    p.data[68] = brightness.clamp(-100, 100) as u8;
    p.data[69] = contrast.clamp(-100, 100) as u8;
}

/// Returns true only for the exact Picture scaffolding synthesized by this
/// writer from an IR picture whose `common_data`/`extras` were empty.
///
/// The predicate is used by post-write semantic verification. It recomputes
/// natural dimensions from the referenced media bytes and checks every common
/// header and shape byte except the writer-assigned instance/storage IDs. A
/// foreign or edited payload therefore remains part of the semantic digest.
pub fn is_materialized_generated_picture(picture: &hwp_model::Picture, media: &[u8]) -> bool {
    let common = &picture.common_data;
    if common.len() < 42 {
        return false;
    }
    let description_units = picture
        .description
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::encode_utf16)
        .map(Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    let Ok(description_len) = u16::try_from(description_units.len()) else {
        return false;
    };
    if common.len() != 42 + description_units.len() * 2 {
        return false;
    }
    let attr: u32 = if picture.treat_as_char {
        0x042a_6001
    } else {
        (0x040a_6000 & !(1 << 13)) | (2 << 3) | (3 << 8) | (5 << 21)
    };
    let instance_id = u32::from_le_bytes(common[32..36].try_into().expect("checked length"));
    if !(0x3000_0000..0x3010_0000).contains(&instance_id) {
        return false;
    }
    let mut expected_common = Vec::with_capacity(common.len());
    expected_common.extend_from_slice(&attr.to_le_bytes());
    expected_common.extend_from_slice(&picture.vert_offset.to_le_bytes());
    expected_common.extend_from_slice(&picture.horz_offset.to_le_bytes());
    expected_common.extend_from_slice(&picture.width.0.to_le_bytes());
    expected_common.extend_from_slice(&picture.height.0.to_le_bytes());
    expected_common.extend_from_slice(&picture.z_order.to_le_bytes());
    expected_common.extend_from_slice(&[0u8; 8]);
    expected_common.extend_from_slice(&instance_id.to_le_bytes());
    expected_common.extend_from_slice(&0u32.to_le_bytes());
    expected_common.extend_from_slice(&description_len.to_le_bytes());
    for unit in description_units {
        expected_common.extend_from_slice(&unit.to_le_bytes());
    }
    if common != &expected_common {
        return false;
    }
    let hwp_model::BinRef::Id(bin_id) = picture.bin_ref else {
        return false;
    };
    let natural = image_pixel_size(media)
        .map(natural_size_hwpunit)
        .unwrap_or((picture.width.0, picture.height.0));
    let mut expected = build_picture_extras(
        picture.width.0,
        picture.height.0,
        natural,
        bin_id.0,
        instance_id ^ 0x0010_0000,
    );
    patch_picture_extras_fx(
        &mut expected,
        picture.crop,
        picture.brightness,
        picture.contrast,
        picture.rotation,
    );
    picture.extras == expected
}

// ── hwpx-출신 도형/글상자 안전 저하 ──────────────────────────────────────
//
// hwpx reader가 만든 구조화 도형(GenericControl.gso_shapes)은 hwp5 SHAPE_COMPONENT
// 레코드가 없다. ㉒에서 정품 템플릿을 패치해 역합성했으나 한글 실기에서 손상 판정
// (라인 252B 템플릿을 사각형에 사용 → 정품 239B와 테두리/채움 섹션이 13B 어긋남 +
// attr/desc 불일치). 한글 수용을 자가검증할 수 없어 재-합성은 손상 재발 위험 →
// **안전 저하**로 전환: 글상자(텍스트 보유)는 그 문단을 본문으로 hoist(텍스트 보존),
// 순수 장식(텍스트 없음)은 손대지 않아 strip_unwritable_pictures가 드롭(유효 보장).
// kind별 정품 템플릿 기반 정합 역합성은 사용자 반복 실기가 필요한 후속 과제.

/// hwpx-출신 글상자의 문단을 본문으로 hoist해 텍스트를 보존한다(도형 래퍼는 생략).
/// 순수 장식 도형은 건드리지 않아 strip이 드롭한다. 본문·표 셀·중첩 리스트 재귀.
fn degrade_hwpx_gso(doc: &mut Document, warnings: &mut WriteReport) {
    for section in &mut doc.sections {
        degrade_hwpx_gso_list(&mut section.paragraphs, warnings);
    }
}

fn degrade_hwpx_gso_list(paras: &mut Vec<Paragraph>, warnings: &mut WriteReport) {
    // 중첩 컨테이너(표 셀·모든 Generic 문단 리스트)를 먼저 재귀 처리한다.
    for p in paras.iter_mut() {
        for c in &mut p.controls {
            match c {
                Control::Table(t) => {
                    for cell in &mut t.cells {
                        degrade_hwpx_gso_list(&mut cell.paragraphs, warnings);
                    }
                }
                Control::Generic(g) => {
                    for l in &mut g.paragraph_lists {
                        degrade_hwpx_gso_list(&mut l.paragraphs, warnings);
                    }
                }
                _ => {}
            }
        }
    }
    // 이 리스트에서 글상자(hwpx 출신 gso + 텍스트)를 host 문단 뒤 본문으로 펼친다.
    let mut out: Vec<Paragraph> = Vec::with_capacity(paras.len());
    for mut p in std::mem::take(paras) {
        let mut hoisted: Vec<Paragraph> = Vec::new();
        let mut removed: Vec<u32> = Vec::new();
        for (i, c) in p.controls.iter().enumerate() {
            if let Control::Generic(g) = c
                && g.ctrl_id != *b"gso "
                && !g.gso_shapes.is_empty()
                && g.raw_children.is_empty()
                && g.data.is_empty()
                && !g.paragraph_lists.is_empty()
            {
                for l in &g.paragraph_lists {
                    hoisted.extend(l.paragraphs.iter().cloned());
                }
                removed.push(i as u32);
                warnings.warning(
                    "hwpx 출신 글상자를 본문 텍스트로 저하(도형 래퍼 생략 — 한글 수용)".to_string(),
                );
            }
        }
        if !removed.is_empty() {
            let mut kept = Vec::with_capacity(p.controls.len());
            for (i, c) in std::mem::take(&mut p.controls).into_iter().enumerate() {
                if !removed.contains(&(i as u32)) {
                    kept.push(c);
                }
            }
            p.controls = kept;
            // 제거된 컨트롤의 ExtCtrl 문자 삭제 + 남은 ctrl_index 재매핑(strip과 동일).
            p.chars.retain(|ch| match ch {
                HwpChar::ExtCtrl {
                    ctrl_index: Some(i),
                    ..
                } => !removed.contains(i),
                _ => true,
            });
            for ch in &mut p.chars {
                if let HwpChar::ExtCtrl {
                    ctrl_index: Some(i),
                    ..
                } = ch
                {
                    let shift = removed.iter().filter(|r| **r < *i).count() as u32;
                    *i -= shift;
                }
            }
        }
        out.push(p);
        out.append(&mut hoisted);
    }
    *paras = out;
}

/// hwpx-출신 구조화 도형(gso 아닌 ctrl_id + gso_shapes 보유)이 있는지 재귀 검사.
fn has_synthesizable_gso(doc: &Document) -> bool {
    fn in_para(p: &Paragraph) -> bool {
        p.controls.iter().any(|c| match c {
            Control::Generic(g) => {
                (g.ctrl_id != *b"gso " && !g.gso_shapes.is_empty())
                    || g.paragraph_lists
                        .iter()
                        .any(|l| l.paragraphs.iter().any(in_para))
            }
            Control::Table(t) => t
                .cells
                .iter()
                .any(|cell| cell.paragraphs.iter().any(in_para)),
            _ => false,
        })
    }
    doc.sections
        .iter()
        .any(|s| s.paragraphs.iter().any(in_para))
}

/// 편집으로 삽입된 그림(도형 레코드 미보유 = extras 빈 Picture)이 있는지 재귀 검사.
/// 있으면 출처가 hwp5여도 그림 합성을 켜야 한다(안 켜면 strip_unwritable_pictures가 드롭).
fn has_synthesizable_picture(doc: &Document) -> bool {
    fn in_para(p: &Paragraph) -> bool {
        p.controls.iter().any(|c| match c {
            Control::Picture(pic) => pic.extras.is_empty(),
            Control::Table(t) => t
                .cells
                .iter()
                .any(|cell| cell.paragraphs.iter().any(in_para)),
            Control::Generic(g) => g
                .paragraph_lists
                .iter()
                .any(|l| l.paragraphs.iter().any(in_para)),
            _ => false,
        })
    }
    doc.sections.iter().flat_map(|s| &s.paragraphs).any(in_para)
}

/// 합성 문서의 hwpx 출신 이미지에 도형 레코드 + BIN_DATA 항목을 채운다.
fn synthesize_pictures(doc: &mut Document, warnings: &mut WriteReport) {
    let bin_names: Vec<String> = doc.bin_streams.iter().map(|b| b.name.clone()).collect();
    if bin_names.is_empty() {
        return;
    }
    // 각 이미지의 원본 픽셀 크기(자르기·picture_effect의 자연 크기 산출용).
    let bin_dims: Vec<Option<(u32, u32)>> = doc
        .bin_streams
        .iter()
        .map(|b| image_pixel_size(&b.data))
        .collect();
    let mut next_id: u16 = doc
        .header
        .bin_data
        .iter()
        .filter_map(|b| b.storage_id)
        .max()
        .unwrap_or(0)
        + 1;
    // bin_stream별 부여된 storage_id (여러 그림이 같은 이미지를 공유하면 재사용).
    let mut assigned: Vec<Option<u16>> = vec![None; bin_names.len()];
    let mut new_items: Vec<hwp_model::BinDataItem> = Vec::new();
    // (bin_stream 인덱스, 새 이름)
    let mut renames: Vec<(usize, String)> = Vec::new();
    // gso 개체마다 유니크 instance_id (같은 값이면 한글이 같은 개체로 보고 무시 가능).
    let mut inst: u32 = 0x3000_0000;

    // 머리말/꼬리말 LIST_HEADER를 구역 PageDef 치수로 채운다(hwpx 출신, 빈 것만).
    // textWidth=본문 폭(용지−좌우 여백), textHeight=머리말/꼬리말 여백. 과거에는
    // 다른 문서(work_report)에서 차용한 템플릿(textHeight=2835, list_attr=0x40)을 써
    // 머리말 영역 높이가 어긋났다. hwp5 출신은 header_data가 차 있어 건드리지 않는다.
    for section in &mut doc.sections {
        if let Some(pg) = section_page_def(section) {
            let text_width = pg.width.0 - pg.margin_left.0 - pg.margin_right.0;
            for para in &mut section.paragraphs {
                fill_head_foot_list_header(
                    para,
                    text_width,
                    pg.margin_header.0,
                    pg.margin_footer.0,
                );
            }
        }
    }

    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            synth_pictures_para(
                para,
                &bin_names,
                &bin_dims,
                &mut assigned,
                &mut next_id,
                &mut inst,
                &mut new_items,
                &mut renames,
                warnings,
            );
        }
    }
    for item in new_items {
        doc.header.bin_data.push(item);
    }
    for (idx, new_name) in renames {
        doc.bin_streams[idx].name = new_name;
    }
}

/// 구역의 PageDef를 찾는다 (SectionDef 컨트롤).
fn section_page_def(section: &Section) -> Option<hwp_model::PageDef> {
    section
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            Control::SectionDef(sd) => sd.page,
            _ => None,
        })
}

/// 머리말/꼬리말(head/foot) GenericControl의 빈 LIST_HEADER를 구역 치수로 채운다.
/// textWidth=본문 폭, textHeight=머리말/꼬리말 여백. hwp5 출신(header_data가 이미 참)은
/// 건드리지 않는다.
fn fill_head_foot_list_header(para: &mut Paragraph, text_width: i32, head_h: i32, foot_h: i32) {
    for control in &mut para.controls {
        let Control::Generic(g) = control else {
            continue;
        };
        let is_head = g.ctrl_id == *b"head";
        let is_foot = g.ctrl_id == *b"foot";
        if !(is_head || is_foot) || !g.raw_children.is_empty() {
            continue;
        }
        let text_height = if is_head { head_h } else { foot_h };
        for list in &mut g.paragraph_lists {
            if !list.header_data.is_empty() {
                continue;
            }
            let mut lh = hex_to_bytes(HEADER_LIST_HEADER_TEMPLATE);
            let npara = list.paragraphs.len().max(1) as u16;
            lh[0..2].copy_from_slice(&npara.to_le_bytes());
            lh[4..8].copy_from_slice(&0u32.to_le_bytes()); // list_attr=0 (정품 머리말)
            lh[8..12].copy_from_slice(&text_width.to_le_bytes());
            lh[12..16].copy_from_slice(&text_height.to_le_bytes());
            list.header_data = lh;
        }
    }
}

/// 한 문단(및 표 셀·글상자 재귀)의 빈 extras 그림에 도형 레코드를 합성한다.
#[allow(clippy::too_many_arguments)]
fn synth_pictures_para(
    para: &mut Paragraph,
    bin_names: &[String],
    bin_dims: &[Option<(u32, u32)>],
    assigned: &mut [Option<u16>],
    next_id: &mut u16,
    inst: &mut u32,
    new_items: &mut Vec<hwp_model::BinDataItem>,
    renames: &mut Vec<(usize, String)>,
    warnings: &mut WriteReport,
) {
    for control in &mut para.controls {
        match control {
            Control::Picture(p) if p.extras.is_empty() => {
                let item_ref = match &p.bin_ref {
                    hwp_model::BinRef::ItemRef(s) => s.clone(),
                    hwp_model::BinRef::Id(_) => String::new(),
                };
                // ItemRef가 파일명에 들어 있는 스트림, 없으면 첫 스트림. 같은
                // 이미지를 여러 그림이 참조할 수 있으므로 배타 사용하지 않는다.
                let pick = bin_names
                    .iter()
                    .position(|n| !item_ref.is_empty() && n.contains(&item_ref))
                    .or(if bin_names.is_empty() { None } else { Some(0) });
                let Some(idx) = pick else {
                    warnings.loss(
                        PreservationCode::BinaryAssetRemoved,
                        PreservationResourceKind::BinaryAsset,
                        PreservationDisposition::Removed,
                        1,
                    );
                    continue;
                };
                // 같은 스트림에 이미 storage_id가 있으면 재사용(공유 이미지).
                let sid = match assigned[idx] {
                    Some(s) => s,
                    None => {
                        let s = *next_id;
                        *next_id += 1;
                        assigned[idx] = Some(s);
                        let ext = bin_names[idx]
                            .rsplit('.')
                            .next()
                            .filter(|e| !e.contains('/'))
                            .unwrap_or("png")
                            .to_ascii_lowercase();
                        new_items.push(hwp_model::BinDataItem {
                            attr: 1, // 임베딩
                            storage_id: Some(s),
                            extension: Some(ext.clone()),
                            ..Default::default()
                        });
                        renames.push((idx, format!("BinData/BIN{s:04X}.{ext}")));
                        s
                    }
                };
                // gso 개체 공통 속성(40B)을 배치에서 합성한다. 정품 한라대 .hwp
                // 실측 attr: 인라인(글자처럼)=0x042a6001, 떠 있는(floating)=
                // 0x040a6000. instance_id는 개체마다 유니크 부여. (고정 템플릿을 쓰면
                // 떠 있는 로고가 인라인으로 배치돼 한글에서 안 보이거나 오배치된다.)
                //
                // 부유는 문단 기준(vertRelTo/horzRelTo=PARA)으로 둔다: 0x040a6000은
                // relTo=PAPER(용지 좌상단 기준)이라, insert_seal이 앵커 문단 안에서
                // 계산한 오프셋을 쓰면 도장이 용지 좌상단으로 튄다(D2 미겹침 결함).
                // PARA 비트(vertRelTo=2<<3, horzRelTo=3<<8)를 세워 hwpx(D1) PARA
                // 배치와 오프셋 의미를 일치시킨다.
                //
                // 어울림(bits21-23)은 글 앞(IN_FRONT_OF_TEXT=5)으로: 표본 0x040a6000의
                // wrap=0(SQUARE)은 본문을 밀어내 겹침이 불가(§4.3.9.1, D2 "(인) 우측
                // 밀림" 실기 결함). 값 체계는 정품 annual attr 0x046a4000→wrap=3
                // (TOP_AND_BOTTOM)으로 교차 확인. bit13(본문 영역 제한)도 해제 —
                // 스펙상 제한 시 겹침 허용(bit14)이 무시된다. hwpx 부유 그림 방출
                // (IN_FRONT_OF_TEXT 고정)과 대칭.
                let attr: u32 = if p.treat_as_char {
                    0x042a_6001
                } else {
                    (0x040a_6000 & !(1 << 13)) | (2 << 3) | (3 << 8) | (5 << 21) // = 0x04aa4310
                };
                // 오프셋은 글자처럼 취급(treat_as_char)이어도 보존한다. 정품 본문 로고는
                // treatAsChar=1인데도 vertOffset=68401을 유지하므로, 0으로 만들면 좌상단으로
                // 이동해 본문과 겹친다.
                let voff: i32 = p.vert_offset;
                let hoff: i32 = p.horz_offset;
                let inst_id = *inst;
                *inst += 1;
                let mut common = Vec::with_capacity(40);
                common.extend_from_slice(&attr.to_le_bytes());
                common.extend_from_slice(&voff.to_le_bytes());
                common.extend_from_slice(&hoff.to_le_bytes());
                common.extend_from_slice(&p.width.0.to_le_bytes());
                common.extend_from_slice(&p.height.0.to_le_bytes());
                common.extend_from_slice(&p.z_order.to_le_bytes());
                common.extend_from_slice(&0u32.to_le_bytes()); // 바깥 여백(왼/위)
                common.extend_from_slice(&0u32.to_le_bytes()); // 바깥 여백(오른/아래)
                common.extend_from_slice(&inst_id.to_le_bytes());
                common.extend_from_slice(&0u32.to_le_bytes()); // 쪽 나눔 방지(split)
                // 개체 설명(description) BSTR — CommonControl ≥5.0.0.5 필수 필드.
                // 빈 설명도 길이(u16=0)는 반드시 있어야 한다(정품 한글은 항상 기록).
                append_gso_description(&mut common, p.description.as_deref(), warnings);
                p.common_data = common;
                // 원본 이미지 자연 크기(픽셀×7200/96). 못 읽으면 표시 크기로
                // 대체(구버전 동작) — 자르기가 일부만 보일 수 있으나 차선.
                let natural = bin_dims
                    .get(idx)
                    .copied()
                    .flatten()
                    .map(natural_size_hwpunit)
                    .unwrap_or((p.width.0, p.height.0));
                // 그림 개체 instance_id(SCPIC)는 gso CTRL의 instance_id와 달라야
                // 한다 — 유니크 비트를 토글해 파생(역시 유니크 유지).
                let pic_inst = inst_id ^ 0x0010_0000;
                p.extras = build_picture_extras(p.width.0, p.height.0, natural, sid, pic_inst);
                patch_picture_extras_fx(
                    &mut p.extras,
                    p.crop,
                    p.brightness,
                    p.contrast,
                    p.rotation,
                );
                p.bin_ref = hwp_model::BinRef::Id(hwp_model::BinDataId(sid));
            }
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for cp in &mut cell.paragraphs {
                        synth_pictures_para(
                            cp, bin_names, bin_dims, assigned, next_id, inst, new_items, renames,
                            warnings,
                        );
                    }
                }
            }
            Control::Generic(g) => {
                // hwpx 출신 머리말/꼬리말(head/foot)은 LIST_HEADER가 비어 strip이
                // 통째로 드롭한다(내부 그림째). 정품 LIST_HEADER 템플릿을 채워 보존한다
                // (paraCount는 실제 문단 수로 패치). data(적용쪽+id 8B)는 hwpx reader가
                // 채우지만, 만일 비어 있으면 최소 4B로 대체해 드롭을 막는다.
                if (g.ctrl_id == *b"head" || g.ctrl_id == *b"foot") && g.raw_children.is_empty() {
                    if g.data.is_empty() {
                        g.data = vec![0, 0, 0, 0];
                    }
                    for list in &mut g.paragraph_lists {
                        if list.header_data.is_empty() {
                            let mut lh = hex_to_bytes(HEADER_LIST_HEADER_TEMPLATE);
                            let npara = list.paragraphs.len().max(1) as u16;
                            lh[0..2].copy_from_slice(&npara.to_le_bytes());
                            list.header_data = lh;
                        }
                    }
                }
                // 합성 각주/미주(md 출신): LIST_HEADER 헤더가 없으면 검증된 텍스트 리스트
                // 헤더 템플릿(paraCount 패치)으로 채운다. 비면 emit_control이 빈 LIST_HEADER를
                // 써 문단 리스트를 잃는다. (각주 전용 리스트 헤더 필드는 실기 확인 대상 — 보고.)
                if (g.ctrl_id == *b"fn  " || g.ctrl_id == *b"en  ") && g.raw_children.is_empty() {
                    for list in &mut g.paragraph_lists {
                        if list.header_data.is_empty() {
                            let mut lh = hex_to_bytes(HEADER_LIST_HEADER_TEMPLATE);
                            let npara = list.paragraphs.len().max(1) as u16;
                            lh[0..2].copy_from_slice(&npara.to_le_bytes());
                            list.header_data = lh;
                        }
                    }
                }
                for list in &mut g.paragraph_lists {
                    for lp in &mut list.paragraphs {
                        synth_pictures_para(
                            lp, bin_names, bin_dims, assigned, next_id, inst, new_items, renames,
                            warnings,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn append_gso_description(
    common: &mut Vec<u8>,
    description: Option<&str>,
    warnings: &mut WriteReport,
) {
    let Some(description) = description.filter(|value| !value.is_empty()) else {
        common.extend_from_slice(&0u16.to_le_bytes());
        return;
    };
    let encoded = description.encode_utf16().collect::<Vec<_>>();
    let Ok(len) = u16::try_from(encoded.len()) else {
        warnings.loss(
            PreservationCode::ControlMetadataUnrepresentable,
            PreservationResourceKind::Metadata,
            PreservationDisposition::Unrepresentable,
            1,
        );
        common.extend_from_slice(&0u16.to_le_bytes());
        return;
    };
    common.extend_from_slice(&len.to_le_bytes());
    for unit in encoded {
        common.extend_from_slice(&unit.to_le_bytes());
    }
}

/// hwp5 레코드가 없는 그림 컨트롤을 확장 문자와 동기 제거하고
/// 남은 ExtCtrl의 ctrl_index를 재조정한다 (중첩 구조 재귀).
fn strip_unwritable_pictures(para: &mut Paragraph, warnings: &mut WriteReport) {
    let mut removed: Vec<u32> = Vec::new();
    let mut kept = Vec::with_capacity(para.controls.len());
    for (i, mut control) in std::mem::take(&mut para.controls).into_iter().enumerate() {
        match &mut control {
            Control::Picture(p) if p.extras.is_empty() => {
                warnings.loss(
                    PreservationCode::PictureControlRemoved,
                    PreservationResourceKind::Control,
                    PreservationDisposition::Removed,
                    1,
                );
                removed.push(i as u32);
                continue;
            }
            // hwp5 페이로드를 합성할 수 없는 컨트롤(hwpx/md 출신 머리말/자동번호
            // 등)만 생략. raw_children가 있으면 hwp5 원본이므로 보존.
            // 예외: 각주/미주(fn/en)는 CTRL_HEADER가 ctrl_id뿐(빈 data가 정상)이고
            // paragraph_lists(본문)로부터 LIST_HEADER+문단을 합성할 수 있으므로 보존한다.
            Control::Generic(g)
                if g.data.is_empty()
                    && g.ctrl_id != *b"cold"
                    && g.raw_children.is_empty()
                    && !((g.ctrl_id == *b"fn  " || g.ctrl_id == *b"en  ")
                        && !g.paragraph_lists.is_empty()) =>
            {
                warnings.loss(
                    PreservationCode::OpaqueControlUnrepresentable,
                    PreservationResourceKind::Control,
                    PreservationDisposition::Unrepresentable,
                    1,
                );
                removed.push(i as u32);
                continue;
            }
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for cp in &mut cell.paragraphs {
                        strip_unwritable_pictures(cp, warnings);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for lp in &mut list.paragraphs {
                        strip_unwritable_pictures(lp, warnings);
                    }
                }
            }
            _ => {}
        }
        kept.push(control);
    }
    para.controls = kept;
    if removed.is_empty() {
        return;
    }
    para.chars.retain(|ch| match ch {
        HwpChar::ExtCtrl {
            ctrl_index: Some(i),
            ..
        } => !removed.contains(i),
        _ => true,
    });
    for ch in &mut para.chars {
        if let HwpChar::ExtCtrl {
            ctrl_index: Some(i),
            ..
        } = ch
        {
            let shift = removed.iter().filter(|r| **r < *i).count() as u32;
            *i -= shift;
        }
    }
}

/// 한글 빈 문서 표본의 구역 정의 페이로드 (43B — 값 재현, 임베드 아님).
const DEFAULT_SECD_DATA: [u8; 43] = [
    0x00, 0x00, 0x00, 0x00, 0x6E, 0x04, 0x00, 0x00, 0x00, 0x00, 0x40, 0x1F, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// 한글 빈 문서 표본의 단 정의 페이로드 (12B).
const DEFAULT_COLD_DATA: [u8; 12] = [
    0x04, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// secd 자식: 각주 모양(FOOTNOTE_SHAPE, 28B) — hello_world 표본 실측.
const DEFAULT_FOOTNOTE_SHAPE: [u8; 28] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00, 0x01, 0x00, 0xff, 0xff, 0xff, 0xff,
    0x52, 0x03, 0x37, 0x02, 0x1b, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
];

/// secd 자식: 미주 모양(FOOTNOTE_SHAPE 태그 공유, 28B) — hello_world 표본 실측.
const DEFAULT_ENDNOTE_SHAPE: [u8; 28] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00, 0x01, 0x00, 0xf8, 0x2f, 0xe0, 0x00,
    0x52, 0x03, 0x37, 0x02, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
];

/// secd 자식: 쪽 테두리/배경(PAGE_BORDER_FILL, 14B) BOTH/EVEN/ODD 3종.
/// 레이아웃: properties(u32) + 여백 4×HWPUNIT16(left/right/top/bottom=1417) +
/// borderFillID(u16)=1(합성 DocInfo에 항상 존재).
/// properties 첫 u32 = 1(flags=1). 정품 한글 저장 파일(가나다.hwp 5.1.1.0) 및
/// work_report.hwp(5.0.2.4) 모두 3종 전부 0x00000001. hello_world 표본의
/// BOTH/EVEN 값(0x0978f9c1·0x271b0b81)은 미초기화로 보이는 이상값이라 채택하지
/// 않는다 — pyhwp가 그 flags를 0x0978f9c1 등으로 파싱(잘못된 속성).
const DEFAULT_PAGE_BORDER_FILLS: [[u8; 14]; 3] = [
    [
        0x01, 0x00, 0x00, 0x00, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x01, 0x00,
    ],
    [
        0x01, 0x00, 0x00, 0x00, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x01, 0x00,
    ],
    [
        0x01, 0x00, 0x00, 0x00, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x01, 0x00,
    ],
];

/// 합성 `SectionDef`가 HWP5 writer에서 materialize된 정확한 기본 payload/자식인지 확인한다.
///
/// semantic 재개방 검증에서 writer가 넣은 기본값만 원래의 빈 합성 표현과 동등하게
/// 정규화하기 위한 predicate다. 사용자 payload나 opaque child가 한 바이트라도 다르면
/// false이므로 활성 문서 의미를 숨기지 않는다.
pub fn is_materialized_default_section_def(def: &SectionDef) -> bool {
    if def.data != DEFAULT_SECD_DATA
        || def.extras.len() != 5
        || def.footnote_shape_raw.as_deref() != Some(DEFAULT_FOOTNOTE_SHAPE.as_slice())
        || def.endnote_shape_raw.as_deref() != Some(DEFAULT_ENDNOTE_SHAPE.as_slice())
        || def.page_border_fills_raw
            != DEFAULT_PAGE_BORDER_FILLS
                .iter()
                .map(|bytes| bytes.to_vec())
                .collect::<Vec<_>>()
    {
        return false;
    }
    let expected = [
        (tag::FOOTNOTE_SHAPE, DEFAULT_FOOTNOTE_SHAPE.as_slice()),
        (tag::FOOTNOTE_SHAPE, DEFAULT_ENDNOTE_SHAPE.as_slice()),
        (
            tag::PAGE_BORDER_FILL,
            DEFAULT_PAGE_BORDER_FILLS[0].as_slice(),
        ),
        (
            tag::PAGE_BORDER_FILL,
            DEFAULT_PAGE_BORDER_FILLS[1].as_slice(),
        ),
        (
            tag::PAGE_BORDER_FILL,
            DEFAULT_PAGE_BORDER_FILLS[2].as_slice(),
        ),
    ];
    def.extras
        .iter()
        .zip(expected)
        .all(|(record, (tag, data))| {
            record.tag == tag && record.data == data && record.children.is_empty()
        })
}

/// 합성 단 정의가 HWP5 writer에서 materialize된 정확한 기본 payload인지 확인한다.
pub fn is_materialized_default_column_def(generic: &hwp_model::GenericControl) -> bool {
    generic.ctrl_id == *b"cold"
        && generic.data == DEFAULT_COLD_DATA
        && generic.column_def.as_ref().is_some_and(|column| {
            column.count == 1
                && column.kind == 0
                && column.direction == 0
                && column.same_width
                && column.gap == 0
                && column.widths.is_empty()
                && column.divider.is_none()
        })
}

/// 기본 번호 정의(NUMBERING) 페이로드 (226B — 5.1.0.1 표본 hello_world와 바이트 동일).
/// 문단 머리 7수준(^1.~^7) + 시작번호 7개 + 5.1.x 확장 3수준. PARA_SHAPE가
/// numbering_id=0 을 참조하므로 테이블이 비면 dangling reference가 되어 한글이
/// '손상/변조'로 거부한다 — 합성/hwpx 출신 안전망의 기본값.
const DEFAULT_NUMBERING_DATA: [u8; 226] = [
    0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x03, 0x00, 0x5e, 0x00,
    0x31, 0x00, 0x2e, 0x00, 0x0c, 0x01, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff,
    0x03, 0x00, 0x5e, 0x00, 0x32, 0x00, 0x2e, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00,
    0xff, 0xff, 0xff, 0xff, 0x03, 0x00, 0x5e, 0x00, 0x33, 0x00, 0x29, 0x00, 0x0c, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x03, 0x00, 0x5e, 0x00, 0x34, 0x00, 0x29, 0x00,
    0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x04, 0x00, 0x28, 0x00,
    0x5e, 0x00, 0x35, 0x00, 0x29, 0x00, 0x0c, 0x01, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0xff, 0xff,
    0xff, 0xff, 0x04, 0x00, 0x28, 0x00, 0x5e, 0x00, 0x36, 0x00, 0x29, 0x00, 0x2c, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x02, 0x00, 0x5e, 0x00, 0x37, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00,
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00,
];

/// 합성 DocInfo에 writer가 주입한 기본 TAB_DEF 3종인지 확인한다.
pub fn is_materialized_default_tab_defs(raw: &[RawEntry], modeled: &[hwp_model::TabDef]) -> bool {
    let expected_raw = [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [1, 0, 0, 0, 0, 0, 0, 0],
        [2, 0, 0, 0, 0, 0, 0, 0],
    ];
    let expected_attrs = [0, 1, 2];
    raw.len() == expected_raw.len()
        && modeled.len() == expected_attrs.len()
        && raw
            .iter()
            .zip(expected_raw)
            .all(|(entry, data)| entry.data == data && entry.children.is_empty())
        && modeled
            .iter()
            .zip(expected_attrs)
            .all(|(tab, attr)| tab.attr == attr && tab.items.is_empty())
}

/// 합성 DocInfo에 writer가 주입한 기본 NUMBERING과 그 의미 파싱 결과인지 확인한다.
pub fn is_materialized_default_numberings(
    raw: &[RawEntry],
    modeled: &[Vec<hwp_model::NumLevel>],
) -> bool {
    const TEMPLATES: [&str; 7] = ["^1.", "^2.", "^3)", "^4)", "(^5)", "(^6)", "^7"];
    !raw.is_empty()
        && raw.len() == modeled.len()
        && raw
            .iter()
            .all(|entry| entry.data == DEFAULT_NUMBERING_DATA && entry.children.is_empty())
        && modeled.iter().all(|levels| {
            levels.len() == TEMPLATES.len()
                && levels.iter().zip(TEMPLATES).all(|(level, template)| {
                    level.start == 1
                        && level.fmt == hwp_model::NumFmt::Digit
                        && level.template == template
                })
        })
}

/// 합성 DocInfo에 writer가 `bullet_chars`에서 만든 raw BULLET 레코드인지 확인한다.
pub fn is_materialized_generated_bullets(raw: &[RawEntry], chars: &[char]) -> bool {
    !raw.is_empty()
        && raw.len() == chars.len()
        && raw
            .iter()
            .zip(chars)
            .all(|(entry, ch)| entry.data == make_bullet_data(*ch) && entry.children.is_empty())
}

/// 합성 문서에 writer가 추가한 정확한 COMPATIBLE_DOCUMENT 서브트리인지 확인한다.
pub fn is_materialized_compatible_document(record: &OpaqueRecord) -> bool {
    record.tag == tag::COMPATIBLE_DOCUMENT
        && record.data == [0; 4]
        && record.children.len() == 2
        && record.children[0].tag == tag::LAYOUT_COMPATIBILITY
        && record.children[0].data == [0; 20]
        && record.children[0].children.is_empty()
        && record.children[1].tag == tag::TRACKCHANGE
        && record.children[1].data.len() == 1032
        && record.children[1].data.first() == Some(&0x38)
        && record.children[1].data[1..].iter().all(|byte| *byte == 0)
        && record.children[1].children.is_empty()
}

/// 빈 `BorderFill::tail`을 writer가 채운 정확한 버전별 payload인지 확인한다.
pub fn is_materialized_generated_border_fill_tail(fill: &hwp_model::BorderFill) -> bool {
    let expected: &[u8] = if fill.fill_type & 0x1 != 0 {
        &[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0]
    } else {
        &[0, 0, 0, 0]
    };
    fill.tail == expected
}

/// 빈 `CharShape::tail`을 writer가 채운 정확한 6바이트 payload인지 확인한다.
pub fn is_materialized_generated_char_shape_tail(shape: &CharShape) -> bool {
    shape.border_fill_id >= 2
        && shape.tail.len() == 6
        && shape.tail[..2] == shape.border_fill_id.to_le_bytes()
        && shape.tail[2..].iter().all(|byte| *byte == 0)
}

/// 빈 `ParaShape::tail`을 writer가 채운 정확한 5.1.x 16바이트 payload인지 확인한다.
pub fn is_materialized_generated_para_shape_tail(shape: &ParaShape) -> bool {
    shape.tail.len() == 16
        && shape.tail[..8].iter().all(|byte| *byte == 0)
        && shape.tail[8..12] == shape.line_spacing.to_le_bytes()
        && shape.tail[12..].iter().all(|byte| *byte == 0)
}

/// 빈 `Style::tail`을 writer가 채운 정확한 잠금 기본값인지 확인한다.
pub fn is_materialized_generated_style_tail(style: &Style) -> bool {
    style.tail == [0, 0]
}

/// HWPTAG_BULLET 레코드 데이터 — **정품 실측 25B**(제주한라대 사업계획서 BULLET 전수 대조).
/// 레이아웃: 문단 머리 정보(8B, attr 0x08=자동 내어쓰기·본문거리 50%) + **번호 글자모양
/// id 4B=0xFFFFFFFF(없음)** + 글머리표 문자 WCHAR(오프셋 12) + 후행 11B(0).
///
/// 스펙 md 표42의 "20B / 글머리 문자 오프셋 8"은 오답이다 — 정품엔 머리 정보와 문자 사이에
/// 글자모양 id 4B가 있어 문자가 오프셋 12에 온다(NUMBERING의 `ff ff ff ff` 필드와 동형).
/// 이 4B를 빠뜨리면 한글이 오프셋 12를 문자로 읽어 0을 만나 **글머리 마커가 미표시**된다
/// (1차 H2 실기 결함). doc_info.rs BULLET 파서도 오프셋 12로 함께 교정.
fn make_bullet_data(ch: char) -> Vec<u8> {
    let mut v = Vec::with_capacity(25);
    v.extend_from_slice(&[0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00]); // 문단 머리 정보 8B
    v.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // 번호 글자모양 id = 없음
    let code = u16::try_from(u32::from(ch)).unwrap_or(0x2022); // 글머리표 문자(오프셋 12)
    v.extend_from_slice(&code.to_le_bytes());
    v.extend_from_slice(&[0u8; 11]); // 이미지 여부/정보·체크 문자·후행 (정품 전수 0)
    v
}

/// `\x05HwpSummaryInformation` (OLE 속성 집합) 작성. 메타데이터(제목/주제/지은이/키워드/
/// 설명/마지막 저장자/작성·수정 일시)를 채우되, 없으면 기존과 동일한 기본값(빈 문자열/
/// FILETIME 0)으로 표본 구조를 유지한다 — 스트림 바이트 배치는 재설계하지 않는다.
/// 헤더 상수는 한글 저장 표본 실측값 — FMTID 9FA2B660-1061-11D4-B4C6-006097C09D8C.
/// PID: 0x02=제목 0x03=주제 0x04=지은이 0x14=날짜 문자열(작성일시 KST, VT_LPWSTR)
/// 0x05=키워드 0x06=설명 0x08=마지막저장자 0x0C=작성일시 0x0D=수정일시(VT_FILETIME)
/// (MS-OLEPS PIDSI). `summary.rs`의 파서와 대칭(단 0x14는 create_time에서 파생하므로
/// 읽기는 무시하고 쓰기 때 재생성한다).
pub(crate) fn hwp_summary_information(meta: &Metadata) -> Vec<u8> {
    const HWP_FMTID: [u8; 16] = [
        0x60, 0xB6, 0xA2, 0x9F, 0x61, 0x10, 0xD4, 0x11, 0xB4, 0xC6, 0x00, 0x60, 0x97, 0xC0, 0x9D,
        0x8C,
    ];
    let title = meta.title.as_deref().unwrap_or("");
    let subject = meta.subject.as_deref().unwrap_or("");
    let author = meta.author.as_deref().unwrap_or("");
    let keywords = meta.keywords.as_deref().unwrap_or("");
    let description = meta.description.as_deref().unwrap_or("");
    let last_saved_by = meta.last_saved_by.as_deref().unwrap_or("");
    // FILETIME은 raw u64 그대로. None이면 0 → 기존 기본값과 바이트 동일(게이트 불변).
    let create_time = meta.create_time.unwrap_or(0);
    let modify_time = meta.modify_time.unwrap_or(0);
    // PID 0x14 = 한국어 KST 날짜 문자열. 정품 표본 40종 전수 관찰 결과 이 문자열은
    // **작성일시(0x0C, create_time)**의 KST 표현이다(수정일시가 아님 — 모든 표본에서
    // 0x14의 연도가 create_time과 일치, modify_time과는 불일치). 한글 문서정보의
    // '날짜'가 이 문자열에서 온다. create_time이 없으면 modify_time으로 대체하고,
    // 둘 다 없으면 빈 문자열(기존 구조/바이트 유지 — 게이트 불변).
    let date_str = meta
        .create_time
        .or(meta.modify_time)
        .and_then(hwp_model::filetime_to_korean_kst)
        .unwrap_or_default();
    // 표본(한글 빈 문서)과 동일한 PID 순서/타입 — 메타데이터만 채움
    enum Val<'a> {
        Str(&'a str),
        FileTime(u64),
        I4,
        Dictionary,
    }
    let props: [(u32, Val); 14] = [
        (0x02, Val::Str(title)),         // 제목
        (0x03, Val::Str(subject)),       // 주제
        (0x04, Val::Str(author)),        // 지은이
        (0x14, Val::Str(&date_str)),     // 날짜 문자열(작성일시 KST)
        (0x05, Val::Str(keywords)),      // 키워드
        (0x06, Val::Str(description)),   // 설명
        (0x08, Val::Str(last_saved_by)), // 마지막 저장자
        (0x09, Val::Str("hwp-cli")),     // 프로그램
        (0x0C, Val::FileTime(create_time)),
        (0x0D, Val::FileTime(modify_time)),
        (0x0B, Val::FileTime(0)),
        (0x0E, Val::I4),
        (0x15, Val::I4),
        // PID 0 = PID_DICTIONARY (MS-OLEPS 2.17): VT_NULL이 아니라 사전 구조여야 한다.
        // 한글 표본(hello_world)은 항목 1개 사전(id=0 → 빈 이름). VT_NULL을 쓰면
        // pyhwp 등이 count=1 사전으로 읽고 항목을 기대하다 EOF(절단)로 거부한다.
        (0x00, Val::Dictionary),
    ];

    // 섹션 본문: ID/오프셋 표 + 값들
    let mut values = ByteWriter::new();
    let table_size = 8 + props.len() * 8;
    let mut offsets = Vec::with_capacity(props.len());
    for (_, val) in &props {
        offsets.push(table_size + values.len());
        match val {
            Val::Str(text) => {
                values.write_u32(31); // VT_LPWSTR
                let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
                values.write_u32(units.len() as u32);
                for u in &units {
                    values.write_u16(*u);
                }
                while !values.len().is_multiple_of(4) {
                    values.write_u8(0);
                }
            }
            Val::FileTime(ft) => {
                values.write_u32(64); // VT_FILETIME
                // dwLowDateTime, dwHighDateTime 순서(리틀엔디언). 0이면 기존 기본값과 동일.
                values.write_u32((*ft & 0xFFFF_FFFF) as u32);
                values.write_u32((*ft >> 32) as u32);
            }
            Val::I4 => {
                values.write_u32(3); // VT_I4
                values.write_u32(0);
            }
            Val::Dictionary => {
                // MS-OLEPS Dictionary: 항목 수 + DictionaryEntry(id + 이름 길이 + 이름).
                // 표본 hello_world와 바이트 동일(13B, 패딩 없음 — 마지막 속성):
                // 항목 1개, id=0, 이름 1바이트(널 종단자).
                values.write_u32(1); // 사전 항목 수
                values.write_u32(0); // DictionaryEntry.id
                values.write_u32(1); // 이름 길이(바이트)
                values.write_u8(0); // 이름: 빈 문자열(널 종단자)
            }
        }
    }
    let values = values.into_bytes();
    let section_size = table_size + values.len();

    let mut w = ByteWriter::new();
    w.write_u16(0xFFFE); // byte order
    w.write_u16(0); // format
    w.write_u32(0x0002_0400); // OS version (표본값)
    w.write_bytes(&HWP_FMTID); // CLSID
    w.write_u32(1); // 섹션 수
    w.write_bytes(&HWP_FMTID); // FMTID
    w.write_u32(48); // 섹션 오프셋
    w.write_u32(section_size as u32);
    w.write_u32(props.len() as u32);
    for ((pid, _), off) in props.iter().zip(&offsets) {
        w.write_u32(*pid);
        w.write_u32(*off as u32);
    }
    w.write_bytes(&values);
    w.into_bytes()
}

fn parse_version(s: &str) -> HwpVersion {
    let mut parts = s.split('.').filter_map(|p| p.parse::<u8>().ok());
    let (a, b, c, d) = (parts.next(), parts.next(), parts.next(), parts.next());
    match (a, b, c, d) {
        (Some(major), Some(minor), Some(build), Some(revision)) => HwpVersion {
            major,
            minor,
            build,
            revision,
        },
        _ => HwpVersion {
            major: 5,
            minor: 1,
            build: 0,
            revision: 1,
        },
    }
}

fn opaque_to_node(o: &OpaqueRecord) -> RecordNode {
    RecordNode {
        tag: o.tag,
        data: o.data.clone(),
        children: o.children.iter().map(opaque_to_node).collect(),
    }
}

fn hwp_string(w: &mut ByteWriter, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    w.write_u16(units.len() as u16);
    for u in units {
        w.write_u16(u);
    }
}

// ─────────────────────────── DocInfo ───────────────────────────

fn emit_doc_info(doc: &Document, _warnings: &mut WriteReport) -> Vec<RecordNode> {
    let h = &doc.header;
    let mut roots = Vec::new();

    // 안전망: 합성(md)·hwpx 출신 문서는 tab_defs/numberings를 비운 채 온다.
    // 그러나 모든 PARA_SHAPE는 tab_def_id=0·numbering_id=0 을 참조하므로
    // 테이블이 비면 ID 0 이 가리킬 레코드가 없어 dangling reference가 되고
    // 한글이 '손상/변조'로 거부한다(halla.hwp 실증: hwpx 출신은 from_markdown
    // 을 거치지 않아 default_header 수정만으로는 커버 불가). 정상 표본
    // (hello_world 5.1.0.1)은 예외 없이 TAB_DEF≥1·NUMBERING=1 을 가진다 —
    // 그 바이트를 그대로 기본값으로 주입한다(값 복제). emit 루프와 ID_MAPPINGS
    // 카운트가 동일 Vec를 참조하므로 둘은 항상 정합한다.
    let tab_defs_owned: Vec<RawEntry> = if h.tab_defs.is_empty() {
        vec![
            RawEntry {
                data: vec![0, 0, 0, 0, 0, 0, 0, 0],
                children: Vec::new(),
            },
            RawEntry {
                data: vec![1, 0, 0, 0, 0, 0, 0, 0],
                children: Vec::new(),
            },
            RawEntry {
                data: vec![2, 0, 0, 0, 0, 0, 0, 0],
                children: Vec::new(),
            },
        ]
    } else {
        h.tab_defs.clone()
    };
    let numberings_owned: Vec<RawEntry> = if h.numberings.is_empty() {
        // 합성 문서(md 출신): 참조되는 번호 정의 수(numbering_levels)만큼 기본 번호를
        // 만든다. 최소 1개 — 모든 PARA_SHAPE가 numbering_id=0을 참조하므로 dangling 방지.
        let count = h.numbering_levels.len().max(1);
        (0..count)
            .map(|_| RawEntry {
                data: DEFAULT_NUMBERING_DATA.to_vec(),
                children: Vec::new(),
            })
            .collect()
    } else {
        h.numberings.clone()
    };
    // 글머리표: 합성 문서는 bullet_chars만큼 BULLET 레코드를 만든다(head_type=3 참조처).
    // 비면 글머리 목록 문단의 numbering_id가 dangling → 한글 '손상' 판정.
    let bullets_owned: Vec<RawEntry> = if h.bullets.is_empty() {
        h.bullet_chars
            .iter()
            .map(|&ch| RawEntry {
                data: make_bullet_data(ch),
                children: Vec::new(),
            })
            .collect()
    } else {
        h.bullets.clone()
    };

    // DOCUMENT_PROPERTIES — 구역 수는 실제 섹션 수에서 유도
    let mut w = ByteWriter::new();
    w.write_u16(doc.sections.len().max(1) as u16);
    // 시작 번호(쪽/각주/미주/그림/표/수식)는 최소 1. 합성 문서는 전부 0인데
    // 한글은 쪽 번호 0을 비정상으로 보고 '손상/변조' 판정 (표본 전부 1).
    for n in h.properties.start_numbers {
        w.write_u16(n.max(1));
    }
    w.write_u32(h.properties.caret.0);
    w.write_u32(h.properties.caret.1);
    w.write_u32(h.properties.caret.2);
    roots.push(RecordNode {
        tag: tag::DOCUMENT_PROPERTIES,
        data: w.into_bytes(),
        children: Vec::new(),
    });

    // ID_MAPPINGS — 카운트는 테이블 길이에서 유도, 원본의 추가 꼬리는 보존
    let mut counts: Vec<u32> = Vec::with_capacity(18);
    counts.push(h.bin_data.len() as u32);
    for slot in 0..LANG_COUNT {
        counts.push(h.fonts[slot].len() as u32);
    }
    counts.push(h.border_fills.len() as u32);
    counts.push(h.char_shapes.len() as u32);
    counts.push(tab_defs_owned.len() as u32);
    counts.push(numberings_owned.len() as u32);
    counts.push(bullets_owned.len() as u32);
    counts.push(h.para_shapes.len() as u32);
    counts.push(h.styles.len() as u32);
    if h.id_mappings_counts.len() > counts.len() {
        counts.extend_from_slice(&h.id_mappings_counts[counts.len()..]);
    }
    // ID_MAPPINGS 카운트 개수는 선언 버전과 일치해야 한다(스펙 표 16, 표 15
    // "doc version 에 따라 가변적"). 인덱스 15=메모모양(5.0.2.1+),
    // 16·17=변경추적(5.0.3.2+). 파생 기본은 15개(인덱스 0~14, 스타일까지).
    //   - 원본을 왕복할 때는 h.id_mappings_counts 길이를 그대로 보존한다
    //     (실제 한컴 저장본은 이미 버전에 맞는 길이라서).
    //   - 합성 문서(원본 카운트 없음)는 선언 버전으로 목표 길이를 정한다.
    // 무조건 18 패딩을 하면 5.0.2.x 문서(16개)를 18개로 부풀려 버전-레이아웃이
    // 어긋나고 한글이 '손상'으로 거부한다(work_report 5.0.2.4 실증).
    let ver = parse_version(&doc.meta.source_version);
    let version_target = if ver.to_u32() >= 0x05_00_03_02 {
        18 // 5.0.3.2 이상: 메모 + 변경추적 ×2
    } else if ver.to_u32() >= 0x05_00_02_01 {
        16 // 5.0.2.1 이상: 메모 모양
    } else {
        15 // 그 이전: 스타일까지
    };
    let target = h
        .id_mappings_counts
        .len()
        .max(version_target)
        .max(counts.len());
    while counts.len() < target {
        counts.push(0);
    }
    let mut w = ByteWriter::new();
    for c in &counts {
        w.write_u32(*c);
    }

    let mut children = Vec::new();
    for item in &h.bin_data {
        children.push(emit_bin_data(item));
    }
    for slot in 0..LANG_COUNT {
        for f in &h.fonts[slot] {
            children.push(emit_face_name(f));
        }
    }
    for bf in &h.border_fills {
        children.push(emit_border_fill(bf));
    }
    for cs in &h.char_shapes {
        children.push(emit_char_shape(cs));
    }
    for t in &tab_defs_owned {
        children.push(RecordNode {
            tag: tag::TAB_DEF,
            data: t.data.clone(),
            children: t.children.iter().map(opaque_to_node).collect(),
        });
    }
    for n in &numberings_owned {
        children.push(RecordNode {
            tag: tag::NUMBERING,
            data: n.data.clone(),
            children: n.children.iter().map(opaque_to_node).collect(),
        });
    }
    for b in &bullets_owned {
        children.push(RecordNode {
            tag: tag::BULLET,
            data: b.data.clone(),
            children: b.children.iter().map(opaque_to_node).collect(),
        });
    }
    for ps in &h.para_shapes {
        children.push(emit_para_shape(ps));
    }
    for st in &h.styles {
        children.push(emit_style(st));
    }
    children.extend(h.id_extras.iter().map(opaque_to_node));

    roots.push(RecordNode {
        tag: tag::ID_MAPPINGS,
        data: w.into_bytes(),
        children,
    });

    // 5.1.x 합성 문서는 COMPATIBLE_DOCUMENT 서브트리가 필수. 정품(가나다.hwp
    // 5.1.1.0, hello_world 5.1.0.1)은 모두 보유하나 구버전(work_report 5.0.2.4)은
    // 면제. 누락 시 한글이 '손상/변조'로 거부. hwp5 원본 왕복은 h.extras로
    // 보존되므로(아래 extend), 합성 경로이고 원본에 없을 때만 추가.
    let synth = doc.meta.source_format != "hwp5";
    let has_compat = h.extras.iter().any(|r| r.tag == tag::COMPATIBLE_DOCUMENT);
    if synth && !has_compat {
        let mut trackchange = vec![0u8; 1032];
        trackchange[0] = 0x38; // 표본(가나다/hello_world) 실측: 선두 0x38, 나머지 0
        roots.push(RecordNode {
            tag: tag::COMPATIBLE_DOCUMENT,
            data: vec![0u8; 4], // 대상 프로그램 0
            children: vec![
                RecordNode {
                    tag: tag::LAYOUT_COMPATIBILITY,
                    data: vec![0u8; 20],
                    children: Vec::new(),
                },
                RecordNode {
                    tag: tag::TRACKCHANGE,
                    data: trackchange,
                    children: Vec::new(),
                },
            ],
        });
    }

    roots.extend(h.extras.iter().map(opaque_to_node));
    roots
}

fn emit_bin_data(item: &hwp_model::BinDataItem) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_u16(item.attr);
    if item.kind() == 0 {
        hwp_string(&mut w, item.link_abs.as_deref().unwrap_or(""));
        hwp_string(&mut w, item.link_rel.as_deref().unwrap_or(""));
    } else {
        w.write_u16(item.storage_id.unwrap_or(0));
        if item.kind() == 1 {
            hwp_string(&mut w, item.extension.as_deref().unwrap_or(""));
        }
    }
    w.write_bytes(&item.tail);
    RecordNode {
        tag: tag::BIN_DATA,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_face_name(f: &FaceName) -> RecordNode {
    let mut attr = f.attr;
    if f.alt_name.is_some() {
        attr |= 0x80;
    }
    if f.panose.is_some() {
        attr |= 0x40;
    }
    if f.default_name.is_some() {
        attr |= 0x20;
    }
    let mut w = ByteWriter::new();
    w.write_u8(attr);
    hwp_string(&mut w, &f.name);
    if let Some(alt) = &f.alt_name {
        w.write_u8(f.alt_kind.unwrap_or(0));
        hwp_string(&mut w, alt);
    }
    if let Some(p) = &f.panose {
        w.write_bytes(p);
    }
    if let Some(d) = &f.default_name {
        hwp_string(&mut w, d);
    }
    w.write_bytes(&f.tail);
    RecordNode {
        tag: tag::FACE_NAME,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn write_border_line(w: &mut ByteWriter, line: &BorderLine) {
    w.write_u8(line.line_type);
    w.write_u8(line.width);
    w.write_u32(line.color);
}

fn emit_border_fill(bf: &hwp_model::BorderFill) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_u16(bf.attr);
    for side in &bf.sides {
        write_border_line(&mut w, side);
    }
    write_border_line(&mut w, &bf.diagonal);
    w.write_u32(bf.fill_type);
    if bf.fill_type & 0x1 != 0 {
        w.write_u32(bf.bg_color.unwrap_or(0xFFFF_FFFF));
    }
    if bf.tail.is_empty() {
        // hwpx/md 출신: hwp5 채우기 블록 완성 (표본 40B/53B 레이아웃 역산)
        if bf.fill_type & 0x1 != 0 {
            w.write_u32(0); // 무늬 색
            w.write_u32(0xFFFF_FFFF); // 무늬 종류 (-1 = 없음)
        }
        w.write_u32(0); // 추가 채우기 속성 크기
        if bf.fill_type & 0x1 != 0 {
            w.write_u8(0); // 투명도
        }
    } else {
        // hwp5 왕복: tail이 무늬색 이후 전부를 담고 있다
        w.write_bytes(&bf.tail);
    }
    RecordNode {
        tag: tag::BORDER_FILL,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_char_shape(cs: &CharShape) -> RecordNode {
    let mut w = ByteWriter::new();
    for id in cs.face_ids {
        w.write_u16(id);
    }
    for v in cs.ratios {
        w.write_u8(v);
    }
    for v in cs.spacings {
        w.write_u8(v as u8);
    }
    for v in cs.rel_sizes {
        w.write_u8(v);
    }
    for v in cs.offsets {
        w.write_u8(v as u8);
    }
    w.write_i32(cs.base_size);
    // 취소선: IR의 `strike` 플래그가 켜지면 CHAR_SHAPE 속성 bit18(취소선 여부=1)을 세운다.
    // 스펙 §4.2.7 표35의 취소선 여부(bit18~20) + 정품 코퍼스 실측(취소선 쓰는 문단이 값 1).
    // **쓰기에만** 적용한다(우리가 취소선임을 아는 md 출신). 읽기는 bit18을 신뢰하지 않는다
    // (변경추적 삭제표시 등에서 bit18이 세워진 정품이 있어 평문에 가짜 취소선을 그림 — DIFFSPEC,
    // doc_info.rs가 strike=false 고정). ⚠ bit 조합의 한글 수용은 H2 실기로 최종 확정 대상.
    let attr = if cs.strike {
        cs.attr | (1 << 18)
    } else {
        cs.attr
    };
    w.write_u32(attr);
    w.write_u8(cs.shadow_gap.0 as u8);
    w.write_u8(cs.shadow_gap.1 as u8);
    w.write_u32(cs.text_color);
    w.write_u32(cs.underline_color);
    w.write_u32(cs.shade_color);
    w.write_u32(cs.shadow_color);
    if cs.tail.is_empty() {
        // hwpx/md 출신: 5.1.x 규격 충전 (테두리채움 ID 5.0.2.1+, 취소선 색 5.0.3.0+)
        w.write_u16(cs.border_fill_id.max(2));
        w.write_u32(0); // 취소선 색
    } else {
        // hwp5 왕복: border_fill_id는 tail 선두에서 추출만 했으므로 그대로 담겨 있다
        w.write_bytes(&cs.tail);
    }
    RecordNode {
        tag: tag::CHAR_SHAPE,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_para_shape(ps: &ParaShape) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_u32(ps.attr1);
    w.write_i32(ps.margin_left);
    w.write_i32(ps.margin_right);
    w.write_i32(ps.indent);
    w.write_i32(ps.spacing_top);
    w.write_i32(ps.spacing_bottom);
    w.write_i32(ps.line_spacing_old);
    w.write_u16(ps.tab_def_id);
    // IR의 번호/글머리 참조는 0-기반이다. on-disk는 1-기반이므로 head_type 2/3에 한해
    // +1 복원한다(parse_para_shape의 -1 정규화의 역 — 왕복 바이트 동일). 개요·머리없음은 그대로.
    let numbering_id_raw = if matches!(ps.head_type(), 2 | 3) {
        ps.numbering_id.saturating_add(1)
    } else {
        ps.numbering_id
    };
    w.write_u16(numbering_id_raw);
    w.write_u16(ps.border_fill_id);
    for v in ps.border_offsets {
        w.write_u16(v as u16);
    }
    if ps.tail.is_empty() {
        // hwpx/md 출신: 5.1.0.1 규격 충전 (속성2/속성3/줄간격 + 후행 4B).
        // 정상 5.1.0.1 표본(hello_world)의 PARA_SHAPE는 58B이며 tail 16B =
        // 속성2(4)+속성3(4)+줄간격(4)+후행(4). 후행 4B를 누락하면(54B) 한글이
        // 무결성 위반으로 '손상/변조' 경고를 띄운다. 합성 문서는 항상
        // 5.1.0.1로 선언되므로(parse_version 기본값) 58B가 정답.
        w.write_u32(0);
        w.write_u32(0);
        w.write_u32(if ps.line_spacing > 0 {
            ps.line_spacing as u32
        } else {
            160
        });
        w.write_u32(0); // 후행 4B — 5.1.0.1 표본 hello_world와 동일 (00 00 00 00)
    } else {
        w.write_bytes(&ps.tail);
    }
    RecordNode {
        tag: tag::PARA_SHAPE,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_style(st: &Style) -> RecordNode {
    let mut w = ByteWriter::new();
    hwp_string(&mut w, &st.name);
    hwp_string(&mut w, &st.english_name);
    w.write_u8(st.attr);
    w.write_u8(st.next_style);
    w.write_u16(st.lang_id as u16);
    w.write_u16(st.para_shape.0);
    w.write_u16(st.char_shape.0);
    if st.tail.is_empty() {
        w.write_u16(0); // 잠금 등 후행 2바이트 (표본 실측)
    } else {
        w.write_bytes(&st.tail);
    }
    RecordNode {
        tag: tag::STYLE,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

// ─────────────────────────── BodyText ───────────────────────────

fn emit_section(
    section: &Section,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> Vec<RecordNode> {
    let mut roots: Vec<RecordNode> = section
        .paragraphs
        .iter()
        .map(|p| {
            emit_paragraph(
                p,
                synthesize,
                preserve_linesegs,
                add_tracking_tail,
                warnings,
            )
        })
        .collect();
    roots.extend(section.extras.iter().map(opaque_to_node));
    roots
}

fn emit_paragraph(
    para: &Paragraph,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> RecordNode {
    // 합성 경로(md/hwpx 출신)는 한글 문단 불변식을 직렬화 시 보정한다. 모든
    // 합성 문단(본문·표 셀·글상자)이 이 단일 지점을 거치므로 누락이 없다.
    // from_markdown뿐 아니라 hwpx→hwp 변환 경로도 함께 커버하기 위해 IR/변환기가
    // 아니라 writer에서 일원화한다.
    // (hwp5 무수정 왕복은 synthesize=false — 원본 보존, 바이트 동일 게이트.)
    let patched;
    let para = if synthesize {
        let mut p = para.clone();
        // 1. 모든 문단은 문단끝 문자(0x0d=13)로 닫혀야 한다 (정품 188문단 전수).
        //    누락 시 한글이 '손상/변조'로 거부.
        if p.chars.last() != Some(&HwpChar::CharCtrl(13)) {
            p.chars.push(HwpChar::CharCtrl(13));
        }
        // 2. nchars 최상위 비트(0x80000000)는 '리스트의 마지막 문단' 표식이다.
        //    정품 다문단(5.1.1.0) 실측: 섹션의 마지막 문단만 세팅, 나머지 0.
        //    모든 문단에 세팅하면 한글이 첫 문단을 마지막으로 보고 뒤 문단을
        //    무시한다(다문단 '둘째부터 안 보임' 원인). chars_flags는 IR 단계의
        //    set_last_para_flag가 리스트별로 설정하므로 여기서 건드리지 않는다.
        // 3. PARA_CHAR_SHAPE 연속 동일 id run 병합 (중복 run은 손상 판정).
        p.char_shape_runs.dedup_by(|(_, b), (_, a)| a == b);
        patched = p;
        &patched
    } else {
        para
    };
    // PARA_HEADER
    let mut w = ByteWriter::new();
    // 빈 문단도 한글은 글자 수를 1로 기록한다(암묵적 문단끝, PARA_TEXT는
    // 생략). 표본 실측: 빈 셀 문단 60개 전부 nchars 하위=1. 0을 쓰면
    // 한글이 손상으로 판정.
    let char_count = if para.chars.is_empty() {
        1
    } else {
        para.wchar_len()
    };
    let nchars = char_count | (u32::from(para.header.chars_flags) << 24);
    w.write_u32(nchars);
    // ctrl_mask는 확장/인라인 컨트롤만 표시한다. 문자형 컨트롤(문단끝 13,
    // 줄나눔 10, 하이픈 등)은 포함하지 않는다 — 표본 실측: 텍스트만 있고
    // 문단끝(13)으로 닫히는 문단의 원본 ctrl_mask=0. CharCtrl까지 비트를
    // 켜면 한글이 'ctrl_mask에 있다는 컨트롤이 실제로 없다'고 손상 판정.
    let ctrl_mask = if para.header.ctrl_mask != 0 {
        para.header.ctrl_mask
    } else {
        para.chars
            .iter()
            .filter_map(|c| match c {
                HwpChar::InlineCtrl { code, .. } | HwpChar::ExtCtrl { code, .. } => {
                    Some(1u32 << code)
                }
                _ => None,
            })
            .fold(0, |a, b| a | b)
    };
    w.write_u32(ctrl_mask);
    w.write_u16(para.para_shape.0);
    w.write_u8(para.style.0 as u8);
    w.write_u8(para.header.break_type);
    w.write_u16(para.char_shape_runs.len() as u16);
    let range_tags = para
        .extras
        .iter()
        .filter(|e| e.tag == tag::PARA_RANGE_TAG)
        .count() as u16;
    w.write_u16(range_tags);
    // 줄 배치(PARA_LINE_SEG) 개수.
    // - preserve_linesegs(=hwp5 무수정 왕복): 원본 그대로 보존(바이트 동일 게이트).
    // - synthesize(=md/hwpx 합성): synthesize_linesegs가 채운 줄 배치를 방출.
    //   5.1.x는 줄 배치 캐시가 없으면 본문이 안 그려진다(13차 실기).
    let emit_lineseg = synthesize || preserve_linesegs;
    let seg_count = if emit_lineseg {
        para.line_segs.len()
    } else {
        0
    };
    w.write_u16(seg_count as u16);
    w.write_u32(para.header.instance_id);
    if para.header.tail.is_empty() {
        // 합성 문단: 선언 버전이 5.0.3.2 이상이면 '변경추적 병합 문단여부'
        // UINT16(=0)을 붙여 24B를 맞춘다(스펙 표 58). 정상 5.1.0.1 표본
        // hello_world의 PARA_HEADER 꼬리(00 00)와 동일. 누락 시(22B) 한글이
        // 버전-레이아웃 불일치로 '손상/변조' 경고.
        if add_tracking_tail {
            w.write_u16(0);
        }
    } else {
        // hwp5 왕복: 꼬리(버전별 추가 필드)를 그대로 보존한다.
        w.write_bytes(&para.header.tail);
    }

    let mut children = Vec::new();
    // 빈 문단(내용이 암묵적 문단끝뿐, nchars=1)은 PARA_TEXT 레코드를 만들지
    // 않는다 — 정품 실측: work_report·한라대 정품의 빈 문단/빈 셀은 전부
    // nchars=1 + PARA_TEXT 없음. 합성 경로는 모든 문단에 CharCtrl(13)을 붙이므로
    // 빈 문단이 chars=[0x0d](char_count=1)가 되는데, 이를 PARA_TEXT=[0x0d]로
    // 방출하면 한글이 '손상'으로 거부한다(빈 셀 표 전부 손상의 원인). 따라서
    // 실제 내용이 있는 문단(char_count>1)만 PARA_TEXT를 방출한다. 채워진 문단은
    // 끝 0x0d를 포함해 PARA_TEXT 길이=nchars로 정품과 동일하다.
    if char_count > 1 {
        children.push(RecordNode {
            tag: tag::PARA_TEXT,
            data: emit_para_text(&para.chars),
            children: Vec::new(),
        });
    }
    if !para.char_shape_runs.is_empty() {
        let mut cw = ByteWriter::new();
        for (pos, id) in &para.char_shape_runs {
            cw.write_u32(*pos);
            cw.write_u32(u32::from(id.0));
        }
        children.push(RecordNode {
            tag: tag::PARA_CHAR_SHAPE,
            data: cw.into_bytes(),
            children: Vec::new(),
        });
    }
    if emit_lineseg && !para.line_segs.is_empty() {
        let mut lw = ByteWriter::new();
        for seg in &para.line_segs {
            lw.write_u32(seg.text_start);
            lw.write_i32(seg.v_pos);
            lw.write_i32(seg.line_height);
            lw.write_i32(seg.text_height);
            lw.write_i32(seg.baseline_gap);
            lw.write_i32(seg.line_spacing);
            lw.write_i32(seg.col_start);
            lw.write_i32(seg.seg_width);
            lw.write_u32(seg.flags);
        }
        children.push(RecordNode {
            tag: tag::PARA_LINE_SEG,
            data: lw.into_bytes(),
            children: Vec::new(),
        });
    }
    let mut next_extra = 0;
    let mut next_control = 0;
    for kind in &para.header.hwp5_child_order {
        match kind {
            hwp_model::Hwp5ParagraphChild::Extra => {
                if let Some(extra) = para.extras.get(next_extra) {
                    children.push(opaque_to_node(extra));
                    next_extra += 1;
                }
            }
            hwp_model::Hwp5ParagraphChild::Control => {
                if let Some(control) = para.controls.get(next_control) {
                    children.push(emit_control(
                        control,
                        synthesize,
                        preserve_linesegs,
                        add_tracking_tail,
                        warnings,
                    ));
                    next_control += 1;
                }
            }
        }
    }
    children.extend(para.extras[next_extra..].iter().map(opaque_to_node));
    for control in &para.controls[next_control..] {
        children.push(emit_control(
            control,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ));
    }

    RecordNode {
        tag: tag::PARA_HEADER,
        data: w.into_bytes(),
        children,
    }
}

fn emit_para_text(chars: &[HwpChar]) -> Vec<u8> {
    let mut w = ByteWriter::new();
    for ch in chars {
        match ch {
            // 방어선: 탭이 Text로 잘못 적재된 경우(과거 오염 IR)에도 8 WCHAR 인라인
            // 컨트롤(코드 9)로 정규화한다. 코드 9를 1 WCHAR로 내보내면 한글이 인라인
            // 컨트롤 선두로 오인해 뒤 7 WCHAR를 잘못 삼킨다(§3.2.3 표 6). 정상 경로는
            // 탭을 InlineCtrl(9)로 이미 담으므로 여기 오지 않아 왕복 바이트동일 게이트에
            // 영향이 없다(정품 hwp5는 탭을 Text로 저장하지 않는다).
            HwpChar::Text('\t') => {
                w.write_u16(9);
                w.write_bytes(&[0u8; 12]);
                w.write_u16(9);
            }
            HwpChar::Text(c) => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    w.write_u16(*u);
                }
            }
            HwpChar::CharCtrl(code) => w.write_u16(*code),
            HwpChar::InlineCtrl { code, payload } | HwpChar::ExtCtrl { code, payload, .. } => {
                w.write_u16(*code);
                let mut p = payload.clone();
                p.resize(12, 0);
                w.write_bytes(&p);
                w.write_u16(*code);
            }
        }
    }
    w.into_bytes()
}

fn reversed(id: [u8; 4]) -> [u8; 4] {
    let mut r = id;
    r.reverse();
    r
}

fn emit_control(
    control: &Control,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> RecordNode {
    match control {
        Control::SectionDef(def) => emit_section_def(def),
        Control::Table(table) => emit_table(
            table,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ),
        Control::Picture(pic) => emit_picture(
            pic,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ),
        Control::Generic(g) => {
            let mut w = ByteWriter::new();
            w.write_bytes(&reversed(g.ctrl_id));
            if g.data.is_empty() && g.ctrl_id == *b"cold" {
                w.write_bytes(&DEFAULT_COLD_DATA);
            } else {
                w.write_bytes(&g.data);
            }
            // 원본 hwp5 자식 서브트리가 있으면 중첩 그대로 방출(무손실).
            // paragraph_lists/extras는 텍스트 추출 전용이므로 평탄화하지 않는다.
            if !g.raw_children.is_empty() {
                return RecordNode {
                    tag: tag::CTRL_HEADER,
                    data: w.into_bytes(),
                    children: g.raw_children.iter().map(opaque_to_node).collect(),
                };
            }
            let mut children = match &g.caption {
                Some(caption) => emit_caption_records(
                    caption,
                    synthesize,
                    preserve_linesegs,
                    add_tracking_tail,
                    warnings,
                ),
                None => Vec::new(),
            };
            for list in &g.paragraph_lists {
                children.push(RecordNode {
                    tag: tag::LIST_HEADER,
                    data: list.header_data.clone(),
                    children: Vec::new(),
                });
                for p in &list.paragraphs {
                    children.push(emit_paragraph(
                        p,
                        synthesize,
                        preserve_linesegs,
                        add_tracking_tail,
                        warnings,
                    ));
                }
            }
            if !g.extras.is_empty() && !g.paragraph_lists.is_empty() {
                warnings.warning(format!(
                    "{:?} 컨트롤 내부 구조가 평탄화되어 저장됨 — 한글에서 확인 필요",
                    String::from_utf8_lossy(&g.ctrl_id)
                ));
            }
            children.extend(g.extras.iter().map(opaque_to_node));
            RecordNode {
                tag: tag::CTRL_HEADER,
                data: w.into_bytes(),
                children,
            }
        }
    }
}

fn emit_section_def(def: &SectionDef) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_bytes(b"dces");
    if def.data.is_empty() {
        w.write_bytes(&DEFAULT_SECD_DATA);
    } else {
        w.write_bytes(&def.data);
    }
    let mut children = Vec::new();
    if let Some(p) = &def.page {
        let mut pw = ByteWriter::new();
        pw.write_i32(p.width.0);
        pw.write_i32(p.height.0);
        pw.write_i32(p.margin_left.0);
        pw.write_i32(p.margin_right.0);
        pw.write_i32(p.margin_top.0);
        pw.write_i32(p.margin_bottom.0);
        pw.write_i32(p.margin_header.0);
        pw.write_i32(p.margin_footer.0);
        pw.write_i32(p.gutter.0);
        pw.write_u32(p.attr);
        children.push(RecordNode {
            tag: tag::PAGE_DEF,
            data: pw.into_bytes(),
            children: Vec::new(),
        });
    }
    if def.extras.is_empty() {
        // 합성 문서(md/hwpx 출신): secd 필수 자식을 5.1.0.1 표본(hello_world)
        // 바이트로 보충한다. 한글은 구역 정의 아래 각주/미주 모양과 쪽
        // 테두리 3종(BOTH/EVEN/ODD)을 기대하며, PAGE_DEF만 있으면 '손상/변조'
        // 판정. PAGE_BORDER_FILL 끝 2B=borderFillID=1(항상 존재).
        for shape in [&DEFAULT_FOOTNOTE_SHAPE, &DEFAULT_ENDNOTE_SHAPE] {
            children.push(RecordNode {
                tag: tag::FOOTNOTE_SHAPE,
                data: shape.to_vec(),
                children: Vec::new(),
            });
        }
        for bf in &DEFAULT_PAGE_BORDER_FILLS {
            children.push(RecordNode {
                tag: tag::PAGE_BORDER_FILL,
                data: bf.to_vec(),
                children: Vec::new(),
            });
        }
    } else {
        children.extend(def.extras.iter().map(opaque_to_node));
    }
    RecordNode {
        tag: tag::CTRL_HEADER,
        data: w.into_bytes(),
        children,
    }
}

fn emit_table(
    table: &Table,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    report: &mut WriteReport,
) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_bytes(b" lbt");
    if !table.common_data.is_empty() {
        // hwp5 출신: 원본 개체 공통 속성 그대로 보존.
        w.write_bytes(&table.common_data);
    } else if let Some(pl) = &table.placement {
        // hwpx 출신: 배치 정보(<hp:pos>/<hp:sz>/<hp:outMargin>/zOrder)에서 합성.
        // 과거에는 0x082A2210(글자처럼취급=꺼짐) 상수로 덮어써 인라인이어야 할 표가
        // 떠 있는 개체가 되고 본문 흐름에서 빠져 한글이 재배치(겹침/빈 페이지)했다.
        w.write_u32(pl.synth_attr());
        w.write_i32(pl.vert_offset);
        w.write_i32(pl.horz_offset);
        w.write_i32(pl.width); // <hp:sz> — 병합 셀 합산보다 정확
        w.write_i32(pl.height);
        w.write_i32(pl.z_order);
        for m in pl.out_margins {
            w.write_u16(m);
        }
        // instance id: 고유한 비-0 값(z순서 기반). 개체 링크용이라 렌더 영향은 작다.
        w.write_u32(0x5000_0000 | (pl.z_order as u32 & 0xffff));
        // @36 INT32 = 쪽나눔 방지 on(1)/off(0) (스펙 표 69). 정품 표본(color_fill.hwp
        // 표) 실측 = 0. 과거 이 자리에 hwpx holdAnchorAndSO를 잘못 기록했다 — 두 필드는
        // 의미가 다르다. holdAnchorAndSO는 hwp5 공통 속성에 대응 자리가 없어 hwp5 합성에서
        // 버린다(손실 아님: hwpx→hwpx 왕복은 <hp:pos> anchor 속성으로 pl.hold_anchor를 보존).
        w.write_i32(0); // 쪽나눔 방지 off
        // 개체 설명문 길이 WORD(len=0) — 스펙 표 69의 마지막 필수 필드(≥5.0.0.5).
        // 누락하면 45→44B가 되어 스펙 최소 46B(=" lbt" 4 + 42)에 미달, 한글이 레코드를
        // 짧게 읽어 거부할 수 있다. 그림 경로(emit_picture 합성)와 동일하게 방출한다.
        w.write_u16(0); // desc_len = 0
    } else {
        // md 출신: 셀 크기로 계산한 떠 있는 표본값(기존 동작 유지).
        let mut col_w = vec![0i64; table.cols.max(1) as usize];
        let mut row_h = vec![0i64; table.rows.max(1) as usize];
        for cell in &table.cells {
            let (c, r) = (cell.col as usize, cell.row as usize);
            if cell.col_span == 1 && c < col_w.len() {
                col_w[c] = col_w[c].max(i64::from(cell.width.0));
            }
            if cell.row_span == 1 && r < row_h.len() {
                row_h[r] = row_h[r].max(i64::from(cell.height.0));
            }
        }
        w.write_u32(0x082A_2210); // 속성 (표본값)
        w.write_u32(0); // 세로 오프셋
        w.write_u32(0); // 가로 오프셋
        w.write_i32(col_w.iter().sum::<i64>() as i32);
        w.write_i32(row_h.iter().sum::<i64>() as i32);
        w.write_u32(0); // z-order
        for _ in 0..4 {
            w.write_u16(283); // 바깥 여백 (표본값)
        }
        w.write_u32(0); // instance id
        w.write_i32(0); // 쪽 나눔 방지
        w.write_u16(0); // 개체 설명문 길이 WORD(len=0) — 스펙 표 69 필수, 46B 정합
    }

    let mut tw = ByteWriter::new();
    tw.write_u32(table.attr);
    tw.write_u16(table.rows);
    tw.write_u16(table.cols);
    tw.write_u16(table.cell_spacing);
    for m in table.inner_margins {
        tw.write_u16(m);
    }
    for c in &table.row_cell_counts {
        tw.write_u16(*c);
    }
    tw.write_u16(table.border_fill.0);
    if table.table_tail.is_empty() {
        tw.write_u16(0); // 영역 속성 크기 (5.0.1.0+)
    } else {
        tw.write_bytes(&table.table_tail);
    }

    // Table 71 caption records precede the TABLE record.
    let mut children = match &table.caption {
        Some(caption) => emit_caption_records(
            caption,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            report,
        ),
        None => Vec::new(),
    };
    children.push(RecordNode {
        tag: tag::TABLE,
        data: tw.into_bytes(),
        children: Vec::new(),
    });
    for cell in &table.cells {
        children.push(emit_cell_header(cell));
        for p in &cell.paragraphs {
            children.push(emit_paragraph(
                p,
                synthesize,
                preserve_linesegs,
                add_tracking_tail,
                report,
            ));
        }
    }
    children.extend(table.extras.iter().map(opaque_to_node));
    RecordNode {
        tag: tag::CTRL_HEADER,
        data: w.into_bytes(),
        children,
    }
}

/// Caption LIST_HEADER (tables 71-73, 22 bytes), inverse of
/// `body_text::parse_caption_header`.
fn emit_caption_header(caption: &Caption) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_i32(caption.paragraphs.len() as i32);
    // Reconstruct only text direction (table 65 bits 0-2); the IR does not
    // preserve line wrapping (bits 3-4) or vertical alignment (bits 5-6).
    // TODO(hancom-verify): measure upper listflag bits in a genuine caption.
    w.write_u32(match caption.direction {
        CaptionDirection::Horizontal => 0,
        CaptionDirection::Vertical => 1,
    });
    let side = match caption.side {
        CaptionSide::Left => 0,
        CaptionSide::Right => 1,
        CaptionSide::Top => 2,
        CaptionSide::Bottom => 3,
    };
    // Bit 2 extends caption width through margins (full-size), mapped to None.
    w.write_u32(side | (u32::from(caption.width.is_none()) << 2));
    w.write_i32(caption.width.unwrap_or(0).min(i32::MAX as u32) as i32);
    w.write_u16(caption.gap.clamp(0, i32::from(u16::MAX)) as u16);
    w.write_i32(caption.last_width.min(i32::MAX as u32) as i32);
    RecordNode {
        tag: tag::LIST_HEADER,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

/// Caption record sequence (LIST_HEADER plus paragraphs). It precedes the
/// object record such as TABLE, matching pyhwp `seen_table_body` behavior.
fn emit_caption_records(
    caption: &Caption,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> Vec<RecordNode> {
    let mut out = vec![emit_caption_header(caption)];
    for p in &caption.paragraphs {
        out.push(emit_paragraph(
            p,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ));
    }
    out
}

fn emit_cell_header(cell: &Cell) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_i32(cell.paragraphs.len() as i32);
    w.write_u32(cell.list_attr);
    w.write_u16(cell.col);
    w.write_u16(cell.row);
    w.write_u16(cell.col_span);
    w.write_u16(cell.row_span);
    w.write_i32(cell.width.0);
    w.write_i32(cell.height.0);
    for m in cell.margins {
        w.write_u16(m);
    }
    w.write_u16(cell.border_fill.0);
    if cell.header_tail.is_empty() {
        // 표본 실측 46B 레이아웃 충전: 텍스트 폭(셀 폭 반복) + 예약 8B
        w.write_i32(cell.width.0);
        w.write_bytes(&[0u8; 8]);
    } else {
        w.write_bytes(&cell.header_tail);
    }
    RecordNode {
        tag: tag::LIST_HEADER,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_picture(
    pic: &Picture,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_bytes(b" osg");
    if pic.common_data.is_empty() {
        // hwpx/md 출신: 개체 공통 속성 최소 구성 (글자처럼 취급)
        warnings.warning("그림 개체 공통 속성을 기본값으로 생성 — 한글에서 확인 필요");
        w.write_u32(u32::from(pic.treat_as_char)); // 속성
        w.write_u32(0); // 세로 오프셋
        w.write_u32(0); // 가로 오프셋
        w.write_i32(pic.width.0);
        w.write_i32(pic.height.0);
        w.write_u32(0); // z-order
        for _ in 0..4 {
            w.write_u16(0); // 바깥 여백
        }
        w.write_u32(0); // instance id
        w.write_i32(0); // 쪽 나눔 방지
        let mut description = Vec::new();
        append_gso_description(&mut description, pic.description.as_deref(), warnings);
        w.write_bytes(&description);
    } else {
        w.write_bytes(&pic.common_data);
    }
    // Table 71 caption follows common object properties and leads child records.
    let mut children = match &pic.caption {
        Some(caption) => emit_caption_records(
            caption,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ),
        None => Vec::new(),
    };
    children.extend(pic.extras.iter().map(opaque_to_node));
    RecordNode {
        tag: tag::CTRL_HEADER,
        data: w.into_bytes(),
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_rewrite_path(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "hwp5-source-rewrite-{label}-{}-{unique}.hwp",
            std::process::id(),
        ))
    }

    fn add_opaque_source_entries(path: &Path) {
        let mut cfb = cfb::open_rw(path).unwrap();
        cfb.create_stream("/MemoExtended")
            .unwrap()
            .write_all(b"opaque-memo-payload")
            .unwrap();
        cfb.create_storage_all("/OpaqueStorage").unwrap();
        cfb.create_stream("/OpaqueStorage/Hidden")
            .unwrap()
            .write_all(b"opaque-hidden-payload")
            .unwrap();
        cfb.flush().unwrap();
    }

    fn raw_streams(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        container
            .list_streams()
            .into_iter()
            .map(|stream| {
                let bytes = container.read_stream_raw(&stream.path).unwrap();
                (stream.path, bytes)
            })
            .collect()
    }

    fn find_control_node(nodes: &[RecordNode], reversed_id: &[u8; 4]) -> Option<RecordNode> {
        for node in nodes {
            if node.tag == tag::CTRL_HEADER && node.data.get(..4) == Some(reversed_id) {
                return Some(node.clone());
            }
            if let Some(found) = find_control_node(&node.children, reversed_id) {
                return Some(found);
            }
        }
        None
    }

    fn section_definition_node(path: &Path) -> RecordNode {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        let bytes = container.read_record_stream("/BodyText/Section0").unwrap();
        let roots = scan_stream(&bytes, ScanMode::Strict).unwrap().roots;
        find_control_node(&roots, b"dces").expect("section definition")
    }

    fn prepend_section_ctrl_data(path: &Path) {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        let compressed = container.file_header().is_compressed();
        let bytes = container.read_record_stream("/BodyText/Section0").unwrap();
        drop(container);
        let mut roots = scan_stream(&bytes, ScanMode::Strict).unwrap().roots;

        fn prepend(nodes: &mut [RecordNode]) -> bool {
            for node in nodes {
                if node.tag == tag::CTRL_HEADER && node.data.get(..4) == Some(b"dces") {
                    node.children.insert(
                        0,
                        RecordNode {
                            tag: tag::CTRL_DATA,
                            data: b"source-order-sentinel".to_vec(),
                            children: Vec::new(),
                        },
                    );
                    return true;
                }
                if prepend(&mut node.children) {
                    return true;
                }
            }
            false
        }
        assert!(prepend(&mut roots));
        let encoded = encode_compressed_stream(RecordNode::serialize_forest(&roots), compressed);
        let mut cfb = cfb::open_rw(path).unwrap();
        cfb.create_stream("/BodyText/Section0")
            .unwrap()
            .write_all(&encoded)
            .unwrap();
        cfb.flush().unwrap();
    }

    #[test]
    fn source_preserving_noop_is_an_exact_file_copy() {
        let source = source_rewrite_path("noop-source");
        let output = source_rewrite_path("noop-output");
        let document = hwp_convert::from_markdown("source preserving no-op");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        add_opaque_source_entries(&source);
        let original = crate::read_document(&source).unwrap().document;

        rewrite_document_with_report(
            &source,
            &original,
            &original,
            &output,
            &WriteOptions::default(),
        )
        .unwrap();

        assert_eq!(
            std::fs::read(&source).unwrap(),
            std::fs::read(&output).unwrap()
        );
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn source_preserving_rewrite_rejects_hardlink_output_alias() {
        let source = source_rewrite_path("alias-source");
        let output = source_rewrite_path("alias-output");
        let document = hwp_convert::from_markdown("source alias guard");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        std::fs::hard_link(&source, &output).unwrap();
        let original_bytes = std::fs::read(&source).unwrap();
        let original = crate::read_document(&source).unwrap().document;

        let error = rewrite_document_with_report(
            &source,
            &original,
            &original,
            &output,
            &WriteOptions::default(),
        )
        .unwrap_err();

        assert!(matches!(error, crate::error::Hwp5Error::SourceRewrite(_)));
        assert_eq!(std::fs::read(&source).unwrap(), original_bytes);
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(source).unwrap();
    }

    #[test]
    fn source_mutation_plan_updates_docinfo_when_section_count_changes() {
        let original = hwp_convert::from_markdown("first section");
        let mut added = original.clone();
        added.sections.push(hwp_model::Section::default());
        let add_plan = HwpMutationPlan::derive(&original, &added);
        assert!(add_plan.header_changed);
        assert_eq!(add_plan.changed_sections, BTreeSet::from([1]));
        assert!(add_plan.removed_sections.is_empty());

        let remove_plan = HwpMutationPlan::derive(&added, &original);
        assert!(remove_plan.header_changed);
        assert_eq!(remove_plan.removed_sections, BTreeSet::from([1]));
    }

    #[test]
    fn source_mutation_plan_does_not_rewrite_docinfo_for_bin_payload_only() {
        let original = hwp_convert::from_markdown("binary payload");
        let mut edited = original.clone();
        edited.bin_streams.push(hwp_model::BinStream {
            name: "BIN0001.png".to_string(),
            data: vec![1, 2, 3],
        });
        let with_bin = edited.clone();
        edited.bin_streams[0].data = vec![4, 5, 6];

        let plan = HwpMutationPlan::derive(&with_bin, &edited);
        assert!(!plan.header_changed);
        assert_eq!(
            plan.changed_bins,
            BTreeSet::from(["bin0001.png".to_string()])
        );
        assert!(plan.removed_bins.is_empty());
    }

    #[test]
    fn control_matching_preserves_shifted_unique_controls_without_guessing_duplicates() {
        fn generic(id: [u8; 4]) -> Control {
            Control::Generic(hwp_model::GenericControl {
                ctrl_id: id,
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
            })
        }

        let first = generic(*b"one ");
        let second = generic(*b"two ");
        assert_eq!(
            match_controls(&[first, second.clone()], std::slice::from_ref(&second)),
            vec![Some(1)]
        );
        assert_eq!(
            match_controls(&[second.clone(), second.clone()], &[second]),
            vec![None]
        );
    }

    #[test]
    fn source_preserving_metadata_edit_changes_only_summary_stream() {
        let source = source_rewrite_path("metadata-source");
        let output = source_rewrite_path("metadata-output");
        let document = hwp_convert::from_markdown("source preserving metadata");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        add_opaque_source_entries(&source);
        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        edited.metadata.title = Some("edited title".to_string());

        rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                preserve_linesegs: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();

        let source_streams = raw_streams(&source);
        let output_streams = raw_streams(&output);
        assert_eq!(
            source_streams.keys().collect::<Vec<_>>(),
            output_streams.keys().collect::<Vec<_>>()
        );
        for (path, source_bytes) in source_streams {
            if path == "/\u{5}HwpSummaryInformation" {
                assert_ne!(source_bytes, output_streams[&path]);
            } else {
                assert_eq!(
                    source_bytes, output_streams[&path],
                    "unexpected stream change: {path}"
                );
            }
        }
        assert_eq!(
            crate::read_document(&output)
                .unwrap()
                .document
                .metadata
                .title
                .as_deref(),
            Some("edited title")
        );
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn source_preserving_text_edit_keeps_opaque_streams_raw() {
        let source = source_rewrite_path("text-source");
        let output = source_rewrite_path("text-output");
        let document = hwp_convert::from_markdown("replace this text");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        add_opaque_source_entries(&source);
        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        assert_eq!(
            hwp_convert::replace_text(&mut edited, "replace", "updated", true),
            1
        );

        rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                preserve_linesegs: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();

        let source_streams = raw_streams(&source);
        let output_streams = raw_streams(&output);
        for path in [
            "/FileHeader",
            "/DocInfo",
            "/MemoExtended",
            "/OpaqueStorage/Hidden",
        ] {
            assert_eq!(
                source_streams[path], output_streams[path],
                "unexpected stream change: {path}"
            );
        }
        assert_ne!(
            source_streams["/BodyText/Section0"],
            output_streams["/BodyText/Section0"]
        );
        assert_ne!(source_streams["/PrvText"], output_streams["/PrvText"]);
        assert!(
            crate::read_document(&output)
                .unwrap()
                .document
                .plain_text()
                .contains("updated")
        );
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn source_preserving_text_edit_keeps_unchanged_typed_control_subtrees_raw() {
        let source = source_rewrite_path("typed-control-source");
        let output = source_rewrite_path("typed-control-output");
        let document = hwp_convert::from_markdown("replace this text");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        prepend_section_ctrl_data(&source);
        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        assert_eq!(
            hwp_convert::replace_text(&mut edited, "replace", "updated", true),
            1
        );

        rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                preserve_linesegs: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();

        let source_section = section_definition_node(&source);
        assert_eq!(source_section.children[0].tag, tag::CTRL_DATA);
        assert_eq!(source_section, section_definition_node(&output));
        assert!(
            crate::read_document(&output)
                .unwrap()
                .document
                .plain_text()
                .contains("updated")
        );
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn source_preserving_image_insert_materializes_only_new_relationships() {
        let source = source_rewrite_path("image-source");
        let output = source_rewrite_path("image-output");
        let asset = source_rewrite_path("image-asset").with_extension("gif");
        let gif = vec![
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x02, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c,
            0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3b,
        ];
        std::fs::write(&asset, gif).unwrap();
        let document = hwp_convert::from_markdown("image anchor");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        add_opaque_source_entries(&source);
        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        hwp_convert::insert_image(
            &mut edited,
            "anchor",
            &asset,
            hwp_convert::ImageSize::Mm(20.0, 10.0),
        )
        .unwrap();

        let report = rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                edited: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();
        assert!(report.preservation.is_lossless());

        let source_streams = raw_streams(&source);
        let output_streams = raw_streams(&output);
        for path in ["/FileHeader", "/MemoExtended", "/OpaqueStorage/Hidden"] {
            assert_eq!(
                source_streams[path], output_streams[path],
                "unexpected stream change: {path}"
            );
        }
        let reopened = crate::read_document(&output).unwrap().document;
        assert_eq!(reopened.bin_streams.len(), original.bin_streams.len() + 1);
        let picture = reopened.sections[0]
            .paragraphs
            .iter()
            .flat_map(|paragraph| &paragraph.controls)
            .find_map(|control| match control {
                Control::Picture(picture) => Some(picture),
                _ => None,
            })
            .expect("inserted picture");
        assert!(reopened.resolve_bin(&picture.bin_ref).is_some());

        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(asset).unwrap();
    }

    #[test]
    fn caption_hwpunit_values_saturate_instead_of_wrapping() {
        let caption = Caption {
            side: CaptionSide::Bottom,
            direction: CaptionDirection::Horizontal,
            gap: i32::MAX,
            width: Some(u32::MAX),
            last_width: u32::MAX,
            paragraphs: Vec::new(),
        };
        let record = emit_caption_header(&caption);
        assert_eq!(
            i32::from_le_bytes(record.data[12..16].try_into().unwrap()),
            i32::MAX
        );
        assert_eq!(
            u16::from_le_bytes(record.data[16..18].try_into().unwrap()),
            u16::MAX
        );
        assert_eq!(
            i32::from_le_bytes(record.data[18..22].try_into().unwrap()),
            i32::MAX
        );
    }

    #[test]
    fn synthesized_generic_gso_emits_its_caption_first() {
        let generic = hwp_model::GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: Some(Caption {
                side: CaptionSide::Bottom,
                direction: CaptionDirection::Horizontal,
                gap: 100,
                width: None,
                last_width: 200,
                paragraphs: vec![hwp_model::Paragraph::default()],
            }),
        };
        let node = emit_control(
            &Control::Generic(generic),
            true,
            false,
            false,
            &mut WriteReport::new(),
        );
        assert_eq!(node.children[0].tag, tag::LIST_HEADER);
        assert_eq!(node.children[1].tag, tag::PARA_HEADER);
    }

    /// hwpx 출신 이미지의 합성 도형 레코드가 정품 5.1.x 구조여야 한다.
    /// SHAPE_COMPONENT(196B, "$pic") → SHAPE_COMPONENT_PICTURE(91B): 자르기는
    /// 표시 크기가 아니라 원본 자연 크기여야 한다(부분 크롭 → 미표시 방지).
    #[test]
    fn 그림_extras_합성_구조() {
        // 표시 8760×1260, 자연 150000×61875.
        let extras = build_picture_extras(8760, 1260, (150000, 61875), 1, 0x3010_0000);
        assert_eq!(extras.len(), 1, "SHAPE_COMPONENT 1개");
        let sc = &extras[0];
        assert_eq!(sc.tag, tag::SHAPE_COMPONENT);
        assert_eq!(sc.data.len(), 196, "SHAPE_COMPONENT 196B");
        assert_eq!(&sc.data[0..4], b"cip$", "chid = $pic(역순)");
        assert_eq!(
            i32::from_le_bytes(sc.data[20..24].try_into().unwrap()),
            8760
        );
        assert_eq!(
            i32::from_le_bytes(sc.data[24..28].try_into().unwrap()),
            1260
        );

        assert_eq!(sc.children.len(), 1, "SHAPE_COMPONENT_PICTURE 자식");
        let pic = &sc.children[0];
        assert_eq!(pic.tag, tag::SHAPE_COMPONENT_PICTURE);
        assert_eq!(
            pic.data.len(),
            91,
            "PICTURE 91B (정품 5.1.x picture_effect)"
        );
        assert_eq!(
            u16::from_le_bytes(pic.data[71..73].try_into().unwrap()),
            1,
            "BinItem ID @71"
        );
        // 꼭지점2 = 표시 크기 (width, height)
        assert_eq!(
            i32::from_le_bytes(pic.data[28..32].try_into().unwrap()),
            8760
        );
        assert_eq!(
            i32::from_le_bytes(pic.data[32..36].try_into().unwrap()),
            1260
        );
        // 자르기 우/하 = 자연 크기 (표시 크기 아님!)
        assert_eq!(
            i32::from_le_bytes(pic.data[52..56].try_into().unwrap()),
            150000,
            "자르기 우 = 자연 폭"
        );
        assert_eq!(
            i32::from_le_bytes(pic.data[56..60].try_into().unwrap()),
            61875,
            "자르기 하 = 자연 높이"
        );
        // instance_id @74 유니크
        assert_eq!(
            u32::from_le_bytes(pic.data[74..78].try_into().unwrap()),
            0x3010_0000,
            "instance_id @74"
        );
        // picture_effect @78: flags=0, 자연 폭@82, 자연 높이@86
        assert_eq!(
            u32::from_le_bytes(pic.data[78..82].try_into().unwrap()),
            0,
            "picture_effect flags=0"
        );
        assert_eq!(
            i32::from_le_bytes(pic.data[82..86].try_into().unwrap()),
            150000
        );
        assert_eq!(
            i32::from_le_bytes(pic.data[86..90].try_into().unwrap()),
            61875
        );
    }

    #[test]
    fn gso_설명_utf16_u16_경계() {
        let mut report = WriteReport::new();
        let mut common = Vec::new();
        append_gso_description(&mut common, Some("😀"), &mut report);
        assert_eq!(u16::from_le_bytes(common[0..2].try_into().unwrap()), 2);
        assert_eq!(common.len(), 6);
        assert!(report.warnings.is_empty());

        let mut max_common = Vec::new();
        append_gso_description(&mut max_common, Some(&"가".repeat(65_535)), &mut report);
        assert_eq!(
            u16::from_le_bytes(max_common[0..2].try_into().unwrap()),
            u16::MAX
        );
        assert_eq!(max_common.len(), 2 + 65_535 * 2);

        let mut overflow = Vec::new();
        append_gso_description(&mut overflow, Some(&"가".repeat(65_536)), &mut report);
        assert_eq!(overflow, 0u16.to_le_bytes());
        assert_eq!(
            report.preservation.events.as_slice(),
            &[hwp_model::PreservationEvent::new(
                PreservationCode::ControlMetadataUnrepresentable,
                PreservationResourceKind::Metadata,
                PreservationDisposition::Unrepresentable,
                1,
            )]
        );
    }

    #[test]
    fn typed_write_report_keeps_loss_events_out_of_ordinary_warnings() {
        let mut doc = hwp_convert::from_markdown("typed report");
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"head",
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
            }));
        let output =
            std::env::temp_dir().join(format!("hwp5-typed-report-{}.hwp", std::process::id()));

        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .all(|warning| !warning.starts_with("DROP:"))
        );
        assert_eq!(report.preservation.events.len(), 1);
        let event = &report.preservation.events[0];
        assert_eq!(event.code, PreservationCode::OpaqueControlUnrepresentable);
        assert_eq!(event.resource, PreservationResourceKind::Control);
        assert_eq!(event.disposition, PreservationDisposition::Unrepresentable);
        assert_eq!(event.count, 1);

        let legacy = write_document(&doc, &output, &WriteOptions::default()).unwrap();
        assert!(
            legacy
                .iter()
                .any(|warning| { warning.starts_with("DROP: opaque_control_unrepresentable") })
        );
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn typed_write_report_accounts_for_missing_picture_payload() {
        let mut doc = hwp_convert::from_markdown("picture report");
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Picture(Picture {
                common_data: Vec::new(),
                width: hwp_model::HwpUnit(1000),
                height: hwp_model::HwpUnit(500),
                treat_as_char: true,
                z_order: 0,
                vert_offset: 0,
                horz_offset: 0,
                description: None,
                crop: None,
                flip: 0,
                rotation: None,
                brightness: 0,
                contrast: 0,
                effect_flags: 0,
                effects_raw: Vec::new(),
                caption: None,
                bin_ref: hwp_model::BinRef::ItemRef("missing-image".to_string()),
                extras: Vec::new(),
            }));
        let output = std::env::temp_dir().join(format!(
            "hwp5-typed-picture-report-{}.hwp",
            std::process::id()
        ));

        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .all(|warning| !warning.starts_with("DROP:"))
        );
        assert!(report.preservation.events.iter().any(|event| {
            event.code == PreservationCode::PictureControlRemoved
                && event.resource == PreservationResourceKind::Control
                && event.disposition == PreservationDisposition::Removed
                && event.count == 1
        }));
        std::fs::remove_file(output).unwrap();
    }

    fn synth_table(placement: Option<hwp_model::GsoPlacement>) -> Table {
        Table {
            common_data: Vec::new(),
            placement,
            attr: 0,
            rows: 1,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: hwp_model::BorderFillId(1),
            table_tail: Vec::new(),
            cells: Vec::new(),
            caption: None,
            extras: Vec::new(),
        }
    }

    /// 합성(hwpx·md 출신) 표의 개체 공통 속성은 스펙 표 69의 최소 46B
    /// (" lbt" 4 + 40 필드 + desc_len 2)로 방출해야 한다. 그림 경로(46B)와 정합.
    /// @40(=공통 4+@36) INT32 = 쪽나눔 방지 = 0(정품 표본 실측), @44 WORD = desc_len = 0.
    #[test]
    fn 합성_표_공통속성_46b_쪽나눔0() {
        // 1) hwpx 출신(placement) 경로.
        let pl = hwp_model::GsoPlacement {
            hold_anchor: true, // hwp5 합성에서 버려야 함(쪽나눔 자리로 새면 안 됨)
            width: 40000,
            height: 20000,
            z_order: 3,
            ..Default::default()
        };
        let node = emit_table(
            &synth_table(Some(pl)),
            true,
            false,
            false,
            &mut WriteReport::new(),
        );
        assert_eq!(&node.data[0..4], b" lbt", "표 ctrl ID");
        assert_eq!(
            node.data.len(),
            46,
            "placement 경로: ctrl ID 4 + 공통 42 = 46B (스펙 표 69 최소)"
        );
        assert_eq!(
            i32::from_le_bytes(node.data[40..44].try_into().unwrap()),
            0,
            "@40 쪽나눔 방지 = 0 (holdAnchorAndSO가 새면 안 됨)"
        );
        assert_eq!(
            u16::from_le_bytes(node.data[44..46].try_into().unwrap()),
            0,
            "@44 desc_len = 0"
        );

        // 2) md 출신 경로(placement=None, common_data 비어 있음).
        let node = emit_table(
            &synth_table(None),
            true,
            false,
            false,
            &mut WriteReport::new(),
        );
        assert_eq!(node.data.len(), 46, "md 경로: 46B (스펙 표 69 최소)");
        assert_eq!(
            i32::from_le_bytes(node.data[40..44].try_into().unwrap()),
            0,
            "md 경로 @40 쪽나눔 방지 = 0"
        );
        assert_eq!(
            u16::from_le_bytes(node.data[44..46].try_into().unwrap()),
            0,
            "md 경로 @44 desc_len = 0"
        );
    }
}
