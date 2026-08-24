//! `hwp new --doc-head`/`--doc-foot` frame tracer (GONG-03 plan 01, D-01/D-02/D-03/D-04).
//!
//! Drives the real binary via `CARGO_BIN_EXE_hwp` against temp dirs only — no fixtures, no fonts,
//! no network, mirroring `tests/skill_templates.rs`'s harness style.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use hwp_model::{Control, Document, HwpChar, Paragraph};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "hwp-cli-frames-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn command_output(command: &mut Command, label: &str) -> Output {
    command
        .output()
        .unwrap_or_else(|error| panic!("{label}: {error}"))
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn reread(path: &Path) -> Document {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("hwp") => hwp5::read_document(path).unwrap().document,
        Some("hwpx") => hwpx::read_document(path).unwrap().document,
        other => panic!("unsupported test extension: {other:?}"),
    }
}

/// Text of a paragraph's own characters only (no recursion).
fn paragraph_text(paragraph: &Paragraph) -> String {
    paragraph
        .chars
        .iter()
        .filter_map(|c| match c {
            HwpChar::Text(ch) => Some(*ch),
            _ => None,
        })
        .collect()
}

/// Flattens paragraph text and table-cell text into one document-order sequence of non-empty
/// blocks (D-03: block order over paragraphs and table controls together, not paragraph order).
/// Recurses into `Control::Table` cells and `Control::Generic` paragraph lists.
fn collect_blocks(paragraphs: &[Paragraph], out: &mut Vec<String>) {
    for paragraph in paragraphs {
        let text = paragraph_text(paragraph);
        if !text.is_empty() {
            out.push(text);
        }
        for control in &paragraph.controls {
            match control {
                Control::Table(table) => {
                    for cell in &table.cells {
                        collect_blocks(&cell.paragraphs, out);
                    }
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        collect_blocks(&list.paragraphs, out);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The document's block text sequence in document order (D-03 helper, reused by later plans in
/// this phase for the full criterion-1 ordering assertion).
fn block_text_sequence(document: &Document) -> Vec<String> {
    let mut out = Vec::new();
    for section in &document.sections {
        collect_blocks(&section.paragraphs, &mut out);
    }
    out
}

/// Index of the first block whose text contains `needle`, panicking with the full sequence on a
/// miss (clearer failure than a bare `None`).
fn index_of(blocks: &[String], needle: &str) -> usize {
    blocks
        .iter()
        .position(|block| block.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not found in block sequence: {blocks:?}"))
}

#[test]
fn tracer_doc_head_becomes_paragraph_zero_and_reopens() {
    let dir = temp_dir("tracer");
    let body = dir.join("body.md");
    std::fs::write(&body, "본문 첫 줄입니다.\n").unwrap();
    let out = dir.join("out.hwpx");

    let output = command_output(
        hwp()
            .args(["new", "--preset", "official", "--from"])
            .arg(&body)
            .args(["--doc-head", "기관명=예시대학교"])
            .args(["--doc-head", "수신=총장"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-head",
    );
    assert_success(&output, "hwp new --doc-head");

    let document = reread(&out);
    let paragraph_zero = &document.sections[0].paragraphs[0];
    // The 두문 table is paragraph 0 (Pattern 5 / D-02).
    assert!(
        paragraph_zero
            .controls
            .iter()
            .any(|control| matches!(control, Control::Table(_))),
        "paragraph 0 must carry the 두문 table control"
    );
    // ...and it still received the section/page-margin controls (secd/cold), proving the
    // architectural bet this tracer exists to test.
    assert!(
        paragraph_zero
            .controls
            .iter()
            .any(|control| matches!(control, Control::SectionDef(_))),
        "paragraph 0 must still carry the section-def control after a 두문 table is prepended"
    );

    let blocks = block_text_sequence(&document);
    let agency = index_of(&blocks, "예시대학교");
    let recipient = index_of(&blocks, "수신  총장");
    let body_idx = index_of(&blocks, "본문 첫 줄입니다.");
    assert!(agency < recipient, "blocks: {blocks:?}");
    assert!(recipient < body_idx, "blocks: {blocks:?}");

    // The file reopens cleanly (hwp validate exit 0), confirming Hancom-shaped bytes.
    let validate = command_output(hwp().arg("validate").arg(&out), "hwp validate");
    assert_success(&validate, "hwp validate");
}

#[test]
fn doc_head_value_containing_equals_is_preserved() {
    let dir = temp_dir("equals-in-value");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--doc-head", "수신=총장=대리"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-head with = in value",
    );
    assert_success(&output, "hwp new --doc-head with = in value");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    assert!(
        blocks.iter().any(|block| block == "수신  총장=대리"),
        "blocks: {blocks:?}"
    );
}

#[test]
fn malformed_doc_head_spec_without_equals_fails_closed() {
    let dir = temp_dir("malformed");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--doc-head", "기관명"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-head (no =)",
    );
    assert!(!output.status.success(), "malformed spec must fail closed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("기관명"), "stderr: {stderr}");
    assert!(!out.exists(), "no file should be written on a hard error");
}

#[test]
fn unknown_doc_head_key_fails_closed() {
    let dir = temp_dir("unknown-key");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--doc-head", "없는키=x"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-head with unknown key",
    );
    assert!(!output.status.success(), "unknown key must fail closed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("없는키=x"), "stderr: {stderr}");
}

#[test]
fn no_frame_flag_leaves_paragraph_zero_untouched() {
    let dir = temp_dir("no-frame");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["-o"])
            .arg(&out),
        "hwp new (no frame flag)",
    );
    assert_success(&output, "hwp new (no frame flag)");

    let document = reread(&out);
    let paragraph_zero = &document.sections[0].paragraphs[0];
    assert!(
        !paragraph_zero
            .controls
            .iter()
            .any(|control| matches!(control, Control::Table(_))),
        "no frame flag must not add a table to paragraph 0"
    );
    let blocks = block_text_sequence(&document);
    assert!(
        !blocks.iter().any(|block| block.contains("예시대학교")),
        "blocks: {blocks:?}"
    );
}

/// The document's single section's last paragraph — where the 결문 table lands (D-03: 결문 is
/// the last block once `--doc-foot` is supplied).
fn last_paragraph(document: &Document) -> &Paragraph {
    document.sections[0]
        .paragraphs
        .last()
        .expect("document must have at least one paragraph")
}

fn table_of(paragraph: &Paragraph) -> &hwp_model::Table {
    paragraph
        .controls
        .iter()
        .find_map(|control| match control {
            Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("paragraph must carry a table control")
}

#[test]
fn doc_foot_last_block_is_table_with_centered_bold_발신명의() {
    let dir = temp_dir("doc-foot-typography");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--doc-foot", "발신명의=예시대학교총장"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-foot",
    );
    assert_success(&output, "hwp new --doc-foot");

    let document = reread(&out);
    let table = table_of(last_paragraph(&document));
    let first_cell_paragraph = &table.cells[0].paragraphs[0];
    assert_eq!(paragraph_text(first_cell_paragraph), "예시대학교총장");

    let para_shape = &document.header.para_shapes[first_cell_paragraph.para_shape.0 as usize];
    assert_eq!(para_shape.alignment(), 3, "발신명의 row must be centered");

    let (_, char_shape_id) = first_cell_paragraph.char_shape_runs[0];
    let char_shape = &document.header.char_shapes[char_shape_id.0 as usize];
    assert_eq!(char_shape.base_size, 2200, "발신명의 row must be 22pt");
    assert_eq!(
        char_shape.attr & (1 << 1),
        1 << 1,
        "발신명의 row must be bold"
    );

    let validate = command_output(hwp().arg("validate").arg(&out), "hwp validate");
    assert_success(&validate, "hwp validate");
}

#[test]
fn doc_foot_결재_and_협조_are_separate_placeholder_rows() {
    let dir = temp_dir("doc-foot-placeholders");
    let out = dir.join("out.hwpx");
    // No 기안자/검토자/결재자/협조자 supplied — the placeholder rows must still appear (D-04).
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--doc-foot", "발신명의=예시대학교총장"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-foot (placeholders only)",
    );
    assert_success(&output, "hwp new --doc-foot (placeholders only)");

    let document = reread(&out);
    let table = table_of(last_paragraph(&document));
    let cell_text: Vec<String> = table
        .cells
        .iter()
        .map(|cell| paragraph_text(&cell.paragraphs[0]))
        .collect();
    let approval_row = index_of(&cell_text, "결재");
    let cooperation_row = index_of(&cell_text, "협조");
    assert!(
        approval_row < cooperation_row,
        "결재 must precede 협조: {cell_text:?}"
    );
    // 협조 must never be folded into the 결재 row (D-04).
    assert_ne!(cell_text[approval_row], cell_text[cooperation_row]);
    assert!(
        !cell_text[approval_row].contains("협조"),
        "결재 row must not fold in 협조: {cell_text:?}"
    );
}

#[test]
fn doc_foot_끝_guard_between_body_and_결문() {
    let dir = temp_dir("kkeut-guard");
    let body = dir.join("body.md");
    std::fs::write(&body, "본문 마지막 줄\n").unwrap();
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official", "--from"])
            .arg(&body)
            .args(["--doc-foot", "발신명의=예시대학교총장"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-foot (끝. guard)",
    );
    assert_success(&output, "hwp new --doc-foot (끝. guard)");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    let body_idx = index_of(&blocks, "본문 마지막 줄");
    let kkeut_idx = index_of(&blocks, "끝.");
    let signoff_idx = index_of(&blocks, "예시대학교총장");
    assert!(
        body_idx < kkeut_idx && kkeut_idx < signoff_idx,
        "blocks: {blocks:?}"
    );
    assert_eq!(
        blocks.iter().filter(|b| b.as_str() == "끝.").count(),
        1,
        "exactly one 끝. paragraph: {blocks:?}"
    );
}

#[test]
fn doc_foot_끝_guard_is_idempotent_when_body_already_ends_with_끝() {
    let dir = temp_dir("kkeut-idempotent");
    let body = dir.join("body.md");
    std::fs::write(&body, "본문 마지막 줄 끝.\n").unwrap();
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official", "--from"])
            .arg(&body)
            .args(["--doc-foot", "발신명의=예시대학교총장"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-foot (already ends with 끝.)",
    );
    assert_success(&output, "hwp new --doc-foot (already ends with 끝.)");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    assert_eq!(
        blocks.iter().filter(|b| b.ends_with("끝.")).count(),
        1,
        "applying the 끝. rule twice must add nothing: {blocks:?}"
    );
}

#[test]
fn doc_foot_two_runs_produce_byte_identical_output() {
    let dir = temp_dir("byte-stable");
    let first = dir.join("first.hwpx");
    let second = dir.join("second.hwpx");
    for out in [&first, &second] {
        let output = command_output(
            hwp()
                .args(["new", "--preset", "official"])
                .args(["--doc-head", "기관명=예시대학교"])
                .args(["--doc-foot", "발신명의=예시대학교총장"])
                .args(["-o"])
                .arg(out),
            "hwp new --doc-head --doc-foot (byte stability)",
        );
        assert_success(&output, "hwp new --doc-head --doc-foot (byte stability)");
    }
    let first_bytes = std::fs::read(&first).unwrap();
    let second_bytes = std::fs::read(&second).unwrap();
    assert_eq!(
        first_bytes, second_bytes,
        "two runs of the same frame flags must produce byte-identical output"
    );
}

/// Success criterion 1, pinned whole: `hwp new` with a full `--doc-head`/`--doc-foot` field set
/// yields the block sequence 두문(기관명, 수신, 경유) → 본문 → `끝.` → 발신명의 → 결재 → 협조 →
/// 시행/접수 → 연락처 (D-03: block order over paragraphs and table controls together — every
/// marker's first index must be strictly increasing, not adjacent, and not paragraph-only, since
/// under D-02 the frames are table controls).
#[test]
fn criterion_1_full_block_order_두문_to_연락처() {
    let dir = temp_dir("criterion-1");
    let body = dir.join("body.md");
    std::fs::write(&body, "본문 마지막 줄\n").unwrap();
    let out = dir.join("out.hwpx");

    let output = command_output(
        hwp()
            .args(["new", "--preset", "official", "--from"])
            .arg(&body)
            .args(["--doc-head", "기관명=예시대학교"])
            .args(["--doc-head", "수신=총장"])
            .args(["--doc-head", "경유=총무과"])
            .args(["--doc-foot", "발신명의=예시대학교총장"])
            .args(["--doc-foot", "기안자=홍길동"])
            .args(["--doc-foot", "검토자=김철수"])
            .args(["--doc-foot", "결재자=박영희"])
            .args(["--doc-foot", "협조자=이몽룡"])
            .args(["--doc-foot", "시행번호=가나1234"])
            .args(["--doc-foot", "시행일자=2026.8.23."])
            .args(["--doc-foot", "접수번호=나다5678"])
            .args(["--doc-foot", "접수일자=2026.8.24."])
            .args(["--doc-foot", "주소=예시로 1"])
            .args(["--doc-foot", "홈페이지=example.go.kr"])
            .args(["--doc-foot", "전화=02-000-0000"])
            .args(["--doc-foot", "팩스=02-111-1111"])
            .args(["--doc-foot", "이메일=test@example.go.kr"])
            .args(["--doc-foot", "공개구분=공개"])
            .args(["-o"])
            .arg(&out),
        "hwp new (criterion 1, full doc-head/doc-foot)",
    );
    assert_success(&output, "hwp new (criterion 1, full doc-head/doc-foot)");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);

    let agency = index_of(&blocks, "예시대학교"); // 기관명 (두문)
    let recipient = index_of(&blocks, "수신  총장"); // 수신 (두문)
    let via = index_of(&blocks, "(경유)  총무과"); // 경유 (두문)
    let body_idx = index_of(&blocks, "본문 마지막 줄"); // 본문
    let kkeut = index_of(&blocks, "끝."); // 끝.
    let signoff = index_of(&blocks, "예시대학교총장"); // 발신명의
    let approval = index_of(&blocks, "결재  기안자 홍길동"); // 결재
    let cooperation = index_of(&blocks, "협조  이몽룡"); // 협조
    let dispatch = index_of(&blocks, "시행  가나1234"); // 시행/접수
    let contact = index_of(&blocks, "전화 02-000-0000"); // 연락처

    let sequence = [
        ("기관명", agency),
        ("수신", recipient),
        ("경유", via),
        ("본문", body_idx),
        ("끝.", kkeut),
        ("발신명의", signoff),
        ("결재", approval),
        ("협조", cooperation),
        ("시행/접수", dispatch),
        ("연락처", contact),
    ];
    for window in sequence.windows(2) {
        let (name_a, idx_a) = window[0];
        let (name_b, idx_b) = window[1];
        assert!(
            idx_a < idx_b,
            "expected {name_a} (block {idx_a}) before {name_b} (block {idx_b}); full sequence: {sequence:?}\nblocks: {blocks:?}"
        );
    }

    // Regression guard (Phase 2.3): `hwp lint` must stay silent on the framed document.
    let lint = command_output(hwp().arg("lint").arg(&out), "hwp lint (framed document)");
    assert_success(&lint, "hwp lint (framed document)");
    assert!(
        String::from_utf8_lossy(&lint.stdout).trim().is_empty(),
        "hwp lint must report zero findings on the framed document, got:\n{}",
        String::from_utf8_lossy(&lint.stdout)
    );

    let validate = command_output(hwp().arg("validate").arg(&out), "hwp validate");
    assert_success(&validate, "hwp validate");
}

#[test]
fn notice_head_produces_agency_and_wrapped_number() {
    let dir = temp_dir("notice-head");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "notice"])
            .args(["--notice-head", "기관명=예시대학교"])
            .args(["--notice-head", "공고번호=2025-282"])
            .args(["-o"])
            .arg(&out),
        "hwp new --notice-head",
    );
    assert_success(&output, "hwp new --notice-head");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    let agency = index_of(&blocks, "예시대학교 공고");
    let number = index_of(&blocks, "제2025-282호");
    assert!(agency < number, "blocks: {blocks:?}");

    let paragraph_zero = &document.sections[0].paragraphs[0];
    assert!(
        paragraph_zero
            .controls
            .iter()
            .any(|control| matches!(control, Control::Table(_))),
        "공고문 head must be a table control (D-02)"
    );

    let lint = command_output(hwp().arg("lint").arg(&out), "hwp lint (notice head)");
    assert_success(&lint, "hwp lint (notice head)");
    assert!(
        String::from_utf8_lossy(&lint.stdout).trim().is_empty(),
        "hwp lint must report zero findings, got:\n{}",
        String::from_utf8_lossy(&lint.stdout)
    );
    let validate = command_output(hwp().arg("validate").arg(&out), "hwp validate");
    assert_success(&validate, "hwp validate");
}

#[test]
fn notice_head_number_already_wrapped_is_not_double_wrapped() {
    let dir = temp_dir("notice-head-wrapped");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "notice"])
            .args(["--notice-head", "공고번호=제2025-282호"])
            .args(["-o"])
            .arg(&out),
        "hwp new --notice-head (already wrapped)",
    );
    assert_success(&output, "hwp new --notice-head (already wrapped)");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    assert!(
        blocks.iter().any(|block| block == "제2025-282호"),
        "blocks: {blocks:?}"
    );
    assert!(
        !blocks.iter().any(|block| block.contains("제제")),
        "number must not be double-wrapped: {blocks:?}"
    );
}

#[test]
fn notice_foot_produces_date_then_sender_centered_bold() {
    let dir = temp_dir("notice-foot");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "notice"])
            .args(["--notice-foot", "공고일자=2026. 8. 23."])
            .args(["--notice-foot", "발신명의=예시대학교총장"])
            .args(["-o"])
            .arg(&out),
        "hwp new --notice-foot",
    );
    assert_success(&output, "hwp new --notice-foot");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    let date_idx = index_of(&blocks, "2026. 8. 23.");
    let sender_idx = index_of(&blocks, "예시대학교총장");
    assert!(date_idx < sender_idx, "blocks: {blocks:?}");

    let table = table_of(last_paragraph(&document));
    let sender_cell = table
        .cells
        .iter()
        .find(|cell| paragraph_text(&cell.paragraphs[0]) == "예시대학교총장")
        .expect("sender row must exist");
    let para_shape = &document.header.para_shapes[sender_cell.paragraphs[0].para_shape.0 as usize];
    assert_eq!(para_shape.alignment(), 3, "발신명의 must be centered");
    let (_, char_shape_id) = sender_cell.paragraphs[0].char_shape_runs[0];
    let char_shape = &document.header.char_shapes[char_shape_id.0 as usize];
    assert_eq!(char_shape.base_size, 2200, "발신명의 must be 22pt");
    assert_eq!(char_shape.attr & (1 << 1), 1 << 1, "발신명의 must be bold");
}

#[test]
fn notice_foot_with_no_공고일자_still_writes_document() {
    let dir = temp_dir("notice-foot-no-date");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "notice"])
            .args(["--notice-foot", "발신명의=예시대학교총장"])
            .args(["-o"])
            .arg(&out),
        "hwp new --notice-foot (no 공고일자)",
    );
    assert_success(&output, "hwp new --notice-foot (no 공고일자)");
    assert!(out.exists());

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    assert!(
        blocks.iter().any(|block| block == "예시대학교총장"),
        "blocks: {blocks:?}"
    );
}

#[test]
fn press_head_produces_title_agency_time_and_contact_rows() {
    let dir = temp_dir("press-head");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "press"])
            .args(["--press-head", "기관명=예시대학교"])
            .args(["--press-head", "보도시점=즉시"])
            .args(["--press-head", "배포일=2026.8.23."])
            .args(["--press-head", "담당부서=홍보실"])
            .args(["--press-head", "담당자=홍길동"])
            .args(["--press-head", "연락처=02-000-0000"])
            .args(["-o"])
            .arg(&out),
        "hwp new --press-head",
    );
    assert_success(&output, "hwp new --press-head");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    let title = index_of(&blocks, "보도자료");
    let agency = index_of(&blocks, "예시대학교");
    let time_line = index_of(&blocks, "보도시점  즉시    배포일  2026.8.23.");
    let contact_line = index_of(&blocks, "담당  홍보실 홍길동 (02-000-0000)");
    assert!(
        title < agency && agency < time_line && time_line < contact_line,
        "blocks: {blocks:?}"
    );

    let lint = command_output(hwp().arg("lint").arg(&out), "hwp lint (press head)");
    assert_success(&lint, "hwp lint (press head)");
    assert!(
        String::from_utf8_lossy(&lint.stdout).trim().is_empty(),
        "hwp lint must report zero findings, got:\n{}",
        String::from_utf8_lossy(&lint.stdout)
    );
    let validate = command_output(hwp().arg("validate").arg(&out), "hwp validate");
    assert_success(&validate, "hwp validate");
}

#[test]
fn unknown_notice_and_press_keys_fail_closed() {
    for (flag, spec) in [
        ("--notice-head", "없는키=x"),
        ("--notice-foot", "없는키=x"),
        ("--press-head", "없는키=x"),
    ] {
        let dir = temp_dir("unknown-key-notice-press");
        let out = dir.join("out.hwpx");
        let output = command_output(
            hwp()
                .args(["new", "--preset", "notice"])
                .args([flag, spec])
                .args(["-o"])
                .arg(&out),
            &format!("hwp new {flag} with unknown key"),
        );
        assert!(
            !output.status.success(),
            "{flag} unknown key must fail closed"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("없는키=x"), "stderr: {stderr}");
        assert!(!out.exists());
    }
}

#[test]
fn compatibility_warning_fires_on_mismatched_preset_and_document_still_writes() {
    let dir = temp_dir("compat-warning-mismatch");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--notice-head", "기관명=예시대학교"])
            .args(["-o"])
            .arg(&out),
        "hwp new --notice-head against --preset official",
    );
    assert_success(&output, "hwp new --notice-head against --preset official");
    assert!(
        out.exists(),
        "mismatched combination must still write the document"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--notice-head"), "stderr: {stderr}");
    assert!(stderr.contains("official"), "stderr: {stderr}");
}

#[test]
fn compatibility_warning_silent_on_matched_preset() {
    let dir = temp_dir("compat-warning-matched");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "notice"])
            .args(["--notice-head", "기관명=예시대학교"])
            .args(["-o"])
            .arg(&out),
        "hwp new --notice-head against --preset notice",
    );
    assert_success(&output, "hwp new --notice-head against --preset notice");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("--notice-head"),
        "matched combination must not warn: stderr: {stderr}"
    );
}

#[test]
fn compatibility_warning_does_not_trip_the_strict_html_contract_gate() {
    let dir = temp_dir("compat-warning-strict");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official", "--strict"])
            .args(["--notice-head", "기관명=예시대학교"])
            .args(["-o"])
            .arg(&out),
        "hwp new --strict with mismatched frame/preset",
    );
    assert_success(
        &output,
        "--strict must exit 0 on a mismatched frame/preset combination (Pitfall 7)",
    );
    assert!(out.exists());
}

#[test]
fn all_five_frame_flags_appear_in_help() {
    let output = command_output(hwp().args(["new", "--help"]), "hwp new --help");
    assert_success(&output, "hwp new --help");
    let help = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--doc-head",
        "--doc-foot",
        "--notice-head",
        "--notice-foot",
        "--press-head",
    ] {
        assert!(help.contains(flag), "help missing {flag}:\n{help}");
    }
}
