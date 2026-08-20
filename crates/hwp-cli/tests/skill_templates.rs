//! Skill template smoke gate (D-19 "usable today"): for every committed
//! `skills/hwp/templates/*.md` skeleton, drive the real binary through
//! `hwp new --from` → `hwp slots` → `hwp fill` → `hwp validate` and assert
//!
//! - every `{{slot}}` in the template source survives import intact and is
//!   reported by `hwp slots` (set equality — a slot split across runs would
//!   still import but would not fill; see Pitfall 1 / EDIT-01 gap),
//! - `hwp fill` succeeds and reports a non-zero replacement count — fill is
//!   fail-closed, so exit 0 already proves every `--set` scalar matched, and
//!   for part-spliced `{{본문}}` templates the JSON report's per-part count
//!   must be >= 1,
//! - `hwp validate` exits 0 on the filled document.
//!
//! CI-safe by construction: committed templates + temp dirs only, no
//! fixtures, no fonts, no network.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

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
/// first slot), and whether the template carries a `{{본문}}` part anchor.
struct Case {
    slug: &'static str,
    preset: &'static str,
    scalar_slot: &'static str,
    has_body_part: bool,
}

const CASES: &[Case] = &[
    Case {
        slug: "gian-internal",
        preset: "gian",
        scalar_slot: "기관명",
        has_body_part: true,
    },
    Case {
        slug: "gian-external",
        preset: "gian",
        scalar_slot: "기관명",
        has_body_part: true,
    },
    Case {
        slug: "gongmun-basic",
        preset: "gian",
        scalar_slot: "기관명",
        has_body_part: true,
    },
    Case {
        slug: "report",
        preset: "report",
        scalar_slot: "제목",
        has_body_part: false,
    },
    Case {
        slug: "plan",
        preset: "report",
        scalar_slot: "사업명",
        has_body_part: false,
    },
    Case {
        slug: "minutes",
        preset: "report",
        scalar_slot: "회의명",
        has_body_part: false,
    },
    Case {
        slug: "notice",
        preset: "gian",
        scalar_slot: "기관명",
        has_body_part: true,
    },
    Case {
        slug: "press",
        preset: "gian",
        scalar_slot: "기관명",
        has_body_part: true,
    },
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
        let expected = source_slots(&source);
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

        // 1. new --from
        let created = dir.join(format!("{}.hwpx", case.slug));
        let output = hwp()
            .arg("new")
            .arg("--from")
            .arg(&template)
            .arg("--preset")
            .arg(case.preset)
            .arg("-o")
            .arg(&created)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: new --from 단계 실패\nstdout: {}\nstderr: {}",
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
    }

    let _ = std::fs::remove_dir_all(&dir);
}
