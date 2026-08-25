//! Skill template smoke gate (D-19 "usable today"): for every committed
//! `skills/hwp/templates/*.md` skeleton, drive the real binary through
//! `hwp new --template` → `hwp slots` → `hwp fill` → `hwp validate` and assert
//! success criterion 3 in full (GONG-03/TMPL-01, D-06):
//!
//! - every `{{slot}}` in the template source survives import intact and is
//!   reported by `hwp slots` (set equality — a slot split across runs would
//!   still import but would not fill; see Pitfall 1 / EDIT-01 gap),
//! - `hwp fill` succeeds and reports a non-zero replacement count — fill is
//!   fail-closed, so exit 0 already proves every `--set` scalar matched, and
//!   for part-spliced `{{본문}}` templates the JSON report's per-part count
//!   must be >= 1,
//! - `hwp validate` exits 0 on the filled document,
//! - `hwp new --list-templates` names all eight slugs, no more, no fewer,
//! - a roman numeral in the template source survives as U+2160, never an
//!   ASCII `I.` substitute,
//! - `minutes` carries all nine 공공기록물 관리에 관한 법률 시행령 제18조
//!   statutory elements,
//! - `hwp lint` reports zero findings on every `--template` output.
//!
//! CI-safe by construction: committed templates + temp dirs only, no
//! fixtures, no fonts, no network.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::{Control, Document, HwpChar, Paragraph};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn repo(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hwp-cli-skill-templates-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// One row of the smoke table: template slug, the preset the recipe uses
/// today, a scalar slot to fill (기관명 where present, else the template's
/// first slot), whether the template carries a `{{본문}}` part anchor, and the
/// slots the template's native 두문/결문 frames contribute on top of the
/// markdown source.
///
/// `frame_slots` mirrors `commands::skill::defaults_for` — an integration test
/// drives the binary from outside, so the table cannot read that constant and
/// states it instead. That is the point: a slot silently added to or dropped
/// from a frame default shows up here as a set mismatch.
struct Case {
    slug: &'static str,
    preset: &'static str,
    scalar_slot: &'static str,
    has_body_part: bool,
    frame_slots: &'static [&'static str],
}

/// 두문 + 결문 for the two templates that carry the full external-dispatch frame set.
const GIAN_FULL_SLOTS: &[&str] = &[
    "기관명",
    "수신",
    "경유",
    "발신명의",
    "기안자",
    "검토자",
    "결재자",
    "협조자",
    "시행번호",
    "시행일자",
    "접수번호",
    "접수일자",
    "주소",
    "홈페이지",
    "전화",
    "팩스",
    "이메일",
    "공개구분",
];

const CASES: &[Case] = &[
    Case {
        slug: "gian-internal",
        preset: "official",
        scalar_slot: "기관명",
        has_body_part: true,
        frame_slots: &[
            "기관명",
            "발신명의",
            "기안자",
            "기안자직위",
            "협조자",
            "시행번호",
            "시행일자",
        ],
    },
    Case {
        slug: "gian-external",
        preset: "official",
        scalar_slot: "기관명",
        has_body_part: true,
        frame_slots: GIAN_FULL_SLOTS,
    },
    Case {
        slug: "gongmun-basic",
        preset: "official",
        scalar_slot: "기관명",
        has_body_part: true,
        frame_slots: GIAN_FULL_SLOTS,
    },
    Case {
        slug: "report",
        preset: "report",
        scalar_slot: "제목",
        has_body_part: false,
        frame_slots: &[],
    },
    Case {
        slug: "plan",
        preset: "plan",
        scalar_slot: "사업명",
        has_body_part: false,
        frame_slots: &[],
    },
    Case {
        slug: "minutes",
        preset: "minutes",
        scalar_slot: "회의명",
        has_body_part: false,
        frame_slots: &[],
    },
    Case {
        slug: "notice",
        preset: "notice",
        scalar_slot: "기관명",
        has_body_part: true,
        frame_slots: &["기관명", "공고번호", "공고일자", "발신명의"],
    },
    Case {
        slug: "press",
        preset: "press",
        scalar_slot: "기관명",
        has_body_part: true,
        frame_slots: &[
            "기관명",
            "보도시점",
            "배포일",
            "담당부서",
            "담당자",
            "연락처",
        ],
    },
];

/// The nine statutory elements of `minutes` (공공기록물 관리에 관한 법률 시행령 제18조, D-19).
const MINUTES_STATUTORY_ELEMENTS: &[&str] = &[
    "회의 명칭",
    "개최기관",
    "일시·장소",
    "참석자·배석자 명단",
    "진행 순서",
    "상정 안건",
    "발언 요지",
    "결정 사항",
    "표결 내용",
];

/// `{{name}}` tokens in the template source, in first-seen order (set
/// semantics — duplicates across paragraphs fill together).
fn source_slots(markdown: &str) -> BTreeSet<String> {
    let mut slots = BTreeSet::new();
    let mut rest = markdown;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else { break };
        slots.insert(after[..close].to_owned());
        rest = &after[close + 2..];
    }
    slots
}

/// Slot names reported by `hwp slots` (one `name<TAB>count` line per slot).
fn reported_slots(stdout: &[u8]) -> BTreeSet<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split('\t').next().unwrap_or(line).to_owned())
        .collect()
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

/// Recurses into `Control::Table` cells and `Control::Generic` paragraph lists, appending every
/// paragraph's text (mirrors `tests/frames.rs`'s `collect_blocks`).
fn collect_text(paragraphs: &[Paragraph], out: &mut String) {
    for paragraph in paragraphs {
        out.push_str(&paragraph_text(paragraph));
        for control in &paragraph.controls {
            match control {
                Control::Table(table) => {
                    for cell in &table.cells {
                        collect_text(&cell.paragraphs, out);
                    }
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        collect_text(&list.paragraphs, out);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The whole document's text, paragraphs and table cells alike, concatenated in document order.
fn full_document_text(document: &Document) -> String {
    let mut out = String::new();
    for section in &document.sections {
        collect_text(&section.paragraphs, &mut out);
    }
    out
}

#[test]
fn skill_templates_are_usable_today() {
    let dir = test_dir("smoke");
    let body_md = dir.join("body.md");
    std::fs::write(
        &body_md,
        "1. 관련: 스모크 시험 근거(안)\n2. 위와 관련하여 본문 부분 이식을 시험합니다.\n",
    )
    .unwrap();

    for case in CASES {
        let template = repo(&format!("skills/hwp/templates/{}.md", case.slug));
        let source = std::fs::read_to_string(&template)
            .unwrap_or_else(|e| panic!("{}: 템플릿 원문 읽기 실패: {e}", case.slug));
        let mut expected = source_slots(&source);
        for slot in case.frame_slots {
            expected.insert((*slot).to_string());
        }
        assert!(
            expected.contains(case.scalar_slot),
            "{}: 스모크 표의 스칼라 슬롯 {}이(가) 템플릿에 없음",
            case.slug,
            case.scalar_slot
        );
        assert_eq!(
            expected.contains("본문"),
            case.has_body_part,
            "{}: 스모크 표의 has_body_part가 템플릿의 본문 슬롯 유무와 어긋남",
            case.slug
        );

        // 1. new --template (D-06: drives the flag this phase ships, not a disk path)
        let created = dir.join(format!("{}.hwpx", case.slug));
        let output = hwp()
            .arg("new")
            .arg("--template")
            .arg(case.slug)
            .arg("--preset")
            .arg(case.preset)
            .arg("-o")
            .arg(&created)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: new --template 단계 실패\nstdout: {}\nstderr: {}",
            case.slug,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        // 2. slots — every source slot must survive import inside a single run
        let output = hwp().arg("slots").arg(&created).output().unwrap();
        assert!(
            output.status.success(),
            "{}: slots 단계 실패\nstderr: {}",
            case.slug,
            String::from_utf8_lossy(&output.stderr)
        );
        let reported = reported_slots(&output.stdout);
        assert_eq!(
            expected, reported,
            "{}: slots 단계 슬롯 집합 불일치 — 템플릿 원문의 {{{{slot}}}}이 임포트에서 깨짐 \
             (런 분할/서식 의심)",
            case.slug
        );

        // 3. fill — non-zero replacements (Pitfall 1 warning sign check)
        let filled = dir.join(format!("{}-filled.hwpx", case.slug));
        let mut fill = hwp();
        fill.arg("fill")
            .arg(&created)
            .arg("-o")
            .arg(&filled)
            .arg("--set")
            .arg(format!("{}=예시값", case.scalar_slot))
            .arg("--json");
        if case.has_body_part {
            fill.arg("--set")
                .arg(format!("본문=@{}", body_md.display()));
        }
        let output = fill.output().unwrap();
        assert!(
            output.status.success(),
            "{}: fill 단계 실패 — 미치환 슬롯이 있으면 fill은 fail-closed로 거절함\nstderr: {}",
            case.slug,
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{}: fill --json 보고서 해석 실패: {e}", case.slug));
        let replaced = report["replaced"].as_u64().unwrap_or(0);
        assert!(
            replaced >= 1,
            "{}: fill 단계 치환 건수 0 — 슬롯이 런 안에 온전히 들어가지 않은 징후 (Pitfall 1)",
            case.slug
        );
        if case.has_body_part {
            let body_hits = report["counts"]["본문"].as_u64().unwrap_or(0);
            assert!(
                body_hits >= 1,
                "{}: fill 단계 본문 부분 이식 0건 — {{{{본문}}}} 앵커 문단이 깨짐",
                case.slug
            );
        }

        // 4. validate
        let output = hwp().arg("validate").arg(&filled).output().unwrap();
        assert!(
            output.status.success(),
            "{}: validate 단계 실패\nstdout: {}\nstderr: {}",
            case.slug,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        // 5. roman numerals survive as U+2160, never an ASCII "I." substitute.
        if source.contains('\u{2160}') {
            let full_text = full_document_text(&reread(&filled));
            assert!(
                full_text.contains('\u{2160}'),
                "{}: 로마 숫자 Ⅰ.(U+2160)이 생성 문서에서 사라짐",
                case.slug
            );
            assert!(
                !full_text.contains("I."),
                "{}: 로마 숫자 자리에 ASCII 'I.'가 남아있음 (U+2160이어야 함)",
                case.slug
            );
        }

        // 6. minutes carries all nine statutory elements (공공기록물 관리에 관한 법률 시행령
        // 제18조, D-19).
        if case.slug == "minutes" {
            let full_text = full_document_text(&reread(&filled));
            for element in MINUTES_STATUTORY_ELEMENTS {
                assert!(
                    full_text.contains(element),
                    "{}: 회의록 필수 요소 누락: {element}",
                    case.slug
                );
            }
        }

        // 7. hwp lint stays silent on every embedded template source that --template resolves
        // to (regression guard carried in from Phase 2.3, `tests/lint.rs::silent_on_embedded_templates`).
        let lint_output = hwp().arg("lint").arg(&template).output().unwrap();
        assert!(
            lint_output.status.success(),
            "{}: hwp lint 실행 실패\nstderr: {}",
            case.slug,
            String::from_utf8_lossy(&lint_output.stderr)
        );
        assert!(
            lint_output.stdout.is_empty(),
            "{}: hwp lint가 --template 소스에서 findings를 냄(0건이어야 함):\n{}",
            case.slug,
            String::from_utf8_lossy(&lint_output.stdout)
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_templates_names_all_eight_slugs() {
    let output = hwp().args(["new", "--list-templates"]).output().unwrap();
    assert!(
        output.status.success(),
        "hwp new --list-templates 실패\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        8,
        "--list-templates must name exactly eight templates:\n{stdout}"
    );
    let listed_slugs: BTreeSet<&str> = lines
        .iter()
        .map(|line| line.split('\t').next().unwrap_or(line))
        .collect();
    let expected_slugs: BTreeSet<&str> = CASES.iter().map(|case| case.slug).collect();
    assert_eq!(
        listed_slugs, expected_slugs,
        "--list-templates slugs must match the smoke table's eight slugs"
    );
}

/// A template carries its own native 두문/결문 frames, whose values default to its `{{slot}}`
/// tokens, and a frame flag overrides one key rather than adding a second row. Phase 2.4
/// verification found the opposite on both counts: `--template` emitted zero table controls and
/// refused every frame flag.
#[test]
fn template_frames_are_native_and_flags_override_them() {
    let dir = test_dir("template-frames");

    // 1. The bare template already produces the two frame tables, with slots left fillable.
    let plain = dir.join("plain.hwpx");
    let run = hwp()
        .args(["new", "--template", "gian-external"])
        .arg("-o")
        .arg(&plain)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "bare --template must succeed\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        table_count(&reread(&plain)),
        2,
        "a bare --template must carry the 두문 and 결문 tables, not loose paragraphs"
    );
    assert!(
        full_document_text(&reread(&plain)).contains("{{기관명}}"),
        "the frame default must stay a fillable slot"
    );

    // 2. A frame flag replaces that one key. Two tables still, and no leftover slot for the
    //    key the caller supplied — that leftover is exactly what a duplicated row would look like.
    let overridden = dir.join("overridden.hwpx");
    let run = hwp()
        .args(["new", "--template", "gian-external"])
        .args(["--doc-head", "기관명=테스트대학교"])
        .args(["--doc-foot", "발신명의=테스트대학교총장"])
        .arg("-o")
        .arg(&overridden)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "--template with frame flags must succeed\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let document = reread(&overridden);
    assert_eq!(
        table_count(&document),
        2,
        "an overriding flag must not add a third frame table"
    );
    let text = full_document_text(&document);
    for supplied in ["테스트대학교", "테스트대학교총장"] {
        assert!(
            text.contains(supplied),
            "flag value {supplied:?} missing:\n{text}"
        );
    }
    for replaced in ["{{기관명}}", "{{발신명의}}"] {
        assert!(
            !text.contains(replaced),
            "slot {replaced:?} survived alongside the flag value — the row is duplicated:\n{text}"
        );
    }
    assert!(
        text.contains("{{수신}}"),
        "a key the caller did not supply must keep its slot default"
    );

    // 3. The template also implies its profile, so no frame/preset mismatch warning is emitted.
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        !stderr.contains("--preset"),
        "a template names its own profile; frames must not warn about a missing --preset:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Table controls anywhere in the document, frames included.
fn table_count(document: &Document) -> usize {
    document
        .sections
        .iter()
        .flat_map(|section| section.paragraphs.iter())
        .flat_map(|paragraph| paragraph.controls.iter())
        .filter(|control| matches!(control, Control::Table(_)))
        .count()
}

/// Documentation regression guard (Phase 2.4 verification gap 2): the two claims the guides once
/// made are both false against the shipped binary, and a doc-only edit is exactly the kind of
/// change no other test notices. Assert the retracted wording stays gone from the embedded copies
/// — those are what `hwp skill` hands a caller.
#[test]
fn embedded_guides_do_not_restate_retracted_claims() {
    for rel in ["official-documents.md", "official-documents.ko.md"] {
        let text = std::fs::read_to_string(repo(&format!("skills/hwp/{rel}"))).unwrap();
        for retracted in [
            // The template path needs the frame flags; it does not refuse them.
            "any frame flag is refused",
            "프레임 플래그와 함께 쓸 수 없",
            // `hwp fill` fails closed on an unmatched slot.
            "Slots a template does not contain are ignored",
            "템플릿에 없는 슬롯은 무시",
        ] {
            assert!(
                !text.contains(retracted),
                "{rel} restates a claim verification falsified: {retracted:?}"
            );
        }
    }
}

/// No two templates may generate the same document. Moving the 두문/결문 fields into the frame
/// builder collapsed `gongmun-basic` onto `gian-external` once, because their bodies are identical
/// and the only thing separating them — `(수신처참조)` on the recipient line — lived in the text
/// that was replaced. Byte equality is the sharpest form of that bug and the cheapest to check.
#[test]
fn every_template_generates_a_distinct_document() {
    let dir = test_dir("template-distinct");
    let mut seen: std::collections::BTreeMap<Vec<u8>, &str> = std::collections::BTreeMap::new();
    for case in CASES {
        let created = dir.join(format!("{}.hwpx", case.slug));
        let run = hwp()
            .args(["new", "--template", case.slug])
            .arg("-o")
            .arg(&created)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}: new --template failed\nstderr: {}",
            case.slug,
            String::from_utf8_lossy(&run.stderr)
        );
        let bytes = std::fs::read(&created).unwrap();
        if let Some(other) = seen.insert(bytes, case.slug) {
            panic!(
                "{} and {} generate byte-identical documents — one template's distinguishing \
                 content was lost",
                other, case.slug
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
