//! Official-document lint engine (표기법·구조 규칙 검사).
//!
//! Pure, IO-free module colocated with `official.rs` (the official-document policy
//! home). Markdown input is parsed with the pulldown-cmark AST (D-01): fenced code
//! blocks, GFM tables and HTML blocks are skipped structurally via event depth, not
//! line heuristics. Both lint profiles run the same rule table in v1 (D-04); the
//! profile enum exists so calibration can diverge later without breaking the CLI.
//!
//! Findings carry rule id, severity, 1-based line and character-based column, and an
//! advisory Korean message only — never source-text excerpts (T-02.3-04).

use std::sync::LazyLock;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use regex::Regex;

/// A lint profile (D-03). Both variants run the same rule table in v1 (D-04).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LintProfile {
    /// Statutory 공문서 rules (default).
    #[default]
    Gongmun,
    /// 보고서 conventions (reserved for later calibration).
    Report,
}

impl LintProfile {
    /// Stable canonical CLI/report name (echoed into the JSON report per D-04).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gongmun => "gongmun",
            Self::Report => "report",
        }
    }
}

/// Finding severity (D-05). Only `Error` affects the `--strict` exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    /// Lowercase report spelling (D-06).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// One lint finding. `line` is 1-based; `col` is character-based (Korean is 3
/// bytes/char in UTF-8, so byte columns would drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule_id: &'static str,
    pub severity: Severity,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

/// Candidate date span: `YYYY. M. D.` with optional spaces and optional final
/// period. The canonical form is `YYYY. M. D.` (space after each period, mandatory
/// final period, no leading zeros) per the 2020 행정업무운영 편람 표기법
/// (skills/hwp/references/korean-official-format.md §6 날짜). A 4-digit year is
/// required so `제3.01.호` and `v2.05.1` can never match.
static DATE_CANDIDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})\. ?(\d{1,2})\. ?(\d{1,2})\.?").unwrap());

/// Advisory message — cites the 편람 표기법, never statutory-violation wording and
/// never a source-text excerpt (T-02.3-04).
const NOTATION_DATE_MESSAGE: &str = "날짜는 `2026. 6. 19.`처럼 표기합니다 (마침표 뒤 한 칸, 마지막 마침표 필수, 앞자리 0 제거) — 행정업무운영 편람 표기법";

/// Lint markdown text and return findings in ascending source order.
///
/// The profile is recorded by callers (report echo, D-04) but selects no rules in
/// v1 — both profiles share one rule table.
pub fn lint_markdown(markdown: &str, profile: LintProfile) -> Vec<Finding> {
    let _ = profile; // Same rule table for every profile in v1 (D-04).

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    // Parses strikethrough (`~~`) and footnotes (`[^N]`). Task lists (TASKLISTS) are excluded — no corresponding IR meaning.
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);

    // 1-based line index: byte offset of each line start, for binary search.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(markdown.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let locate = |offset: usize| -> (u32, u32) {
        let idx = line_starts.partition_point(|&start| start <= offset);
        let line_start = line_starts[idx - 1];
        let col = markdown[line_start..offset].chars().count() as u32 + 1;
        (idx as u32, col)
    };

    let mut findings = Vec::new();
    // Structural skip depth (D-01): Text inside code blocks, tables and HTML blocks
    // is never linted; InlineHtml events are skipped outright.
    let mut skip_depth: u32 = 0;
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::Table(_) | Tag::HtmlBlock) => {
                skip_depth += 1;
            }
            Event::End(TagEnd::CodeBlock | TagEnd::Table | TagEnd::HtmlBlock) => {
                skip_depth = skip_depth.saturating_sub(1);
            }
            Event::Text(text) if skip_depth == 0 => {
                lint_notation_date(&text, range.start, &locate, &mut findings);
            }
            _ => {}
        }
    }
    findings
}

/// `notation-date` (warning): flag any date-like span that deviates from the
/// canonical `YYYY. M. D.` form — missing spaces (`2020.7.8`), a missing final
/// period (`2026. 8. 20`), or leading zeros (`1985.09.06.`).
fn lint_notation_date(
    text: &str,
    base: usize,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    for caps in DATE_CANDIDATE.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        // Guard: a digit on either side means a longer numeric run (e.g. the
        // `12026. 8. 20.` prefix or the `2020.7.8.1` 4-part version), not a date.
        let before = text[..whole.start()].bytes().next_back();
        let after = text[whole.end()..].bytes().next();
        if before.is_some_and(|b| b.is_ascii_digit()) || after.is_some_and(|b| b.is_ascii_digit()) {
            continue;
        }
        let year: u32 = caps[1].parse().unwrap();
        let month: u32 = caps[2].parse().unwrap();
        let day: u32 = caps[3].parse().unwrap();
        let canonical = format!("{year}. {month}. {day}.");
        if whole.as_str() == canonical {
            continue;
        }
        let (line, col) = locate(base + whole.start());
        findings.push(Finding {
            rule_id: "notation-date",
            severity: Severity::Warning,
            line,
            col,
            message: NOTATION_DATE_MESSAGE.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint(md: &str) -> Vec<Finding> {
        lint_markdown(md, LintProfile::Gongmun)
    }

    #[test]
    fn notation_date_fires_on_each_wrong_form() {
        for (md, why) in [
            ("시행: 2020.7.8\n", "missing spaces and final period"),
            ("시행: 2026. 8. 20\n", "missing final period"),
            ("시행: 1985.09.06.\n", "leading zeros and missing spaces"),
            ("시행: 2020. 7. 8\n", "missing final period only"),
        ] {
            let findings = lint(md);
            assert_eq!(findings.len(), 1, "{why}: {md:?} -> {findings:?}");
            assert_eq!(findings[0].rule_id, "notation-date");
            assert_eq!(findings[0].severity, Severity::Warning);
        }
    }

    #[test]
    fn notation_date_silent_on_correct_forms() {
        for md in [
            "시행: 2020. 7. 8.\n",
            "시행일: 2026. 6. 19.\n",
            "2023. 11. 11.(토) 개최\n",
            "기간: 2026. 4. 23.∼2026. 6. 15.\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_date_never_matches_non_date_numeric_runs() {
        for md in [
            "제3.01.호 참조\n",  // no 4-digit year
            "버전 v2.05.1\n",    // no 4-digit year
            "버전 2020.7.8.1\n", // 4-part version: digit after the span
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn structural_skips_code_table_and_html_blocks() {
        let md = "```\n2020.7.8\n```\n\n| 날짜 |\n|------|\n| 2020.7.8 |\n\n<div>\n2020.7.8\n</div>\n\n시행: 2020.7.8\n";
        let findings = lint(md);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 13);
    }

    #[test]
    fn line_and_col_are_1_based_and_character_based() {
        // Korean text before the date: byte cols would drift (3 bytes/char).
        let md = "본문입니다.\n\n시행: 2020.7.8\n";
        let findings = lint(md);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 3);
        // "시행: " is 4 characters, so the date starts at column 5.
        assert_eq!(findings[0].col, 5);
    }

    #[test]
    fn report_profile_runs_the_same_rule_table_v1() {
        let md = "시행: 2020.7.8\n";
        assert_eq!(
            lint_markdown(md, LintProfile::Gongmun),
            lint_markdown(md, LintProfile::Report)
        );
    }

    // ---- Task 1: notation families + ai-style-marks + bare-URL mask ----

    fn only_rule(md: &str, rule_id: &str) -> Vec<Finding> {
        let findings = lint(md);
        for f in &findings {
            assert_eq!(f.rule_id, rule_id, "unexpected rule in {md:?}: {findings:?}");
        }
        findings
    }

    #[test]
    fn notation_time_fires_on_meridiem_and_single_digit_hour() {
        let findings = only_rule("일시: 오후 3시 20분\n", "notation-time");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].severity, Severity::Warning);
        let findings = only_rule("회의는 8:09에 시작합니다\n", "notation-time");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_time_silent_on_canonical_24h_forms() {
        for md in ["일시: 15:20\n", "일시: 08:09\n", "15:20∼17:00\n"] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_money_fires_on_abstract_thousand_and_missing_reading() {
        let findings = only_rule("비용: 345천원\n", "notation-money");
        assert_eq!(findings.len(), 1, "{findings:?}");
        let findings = only_rule("금액은 금113,560원이다\n", "notation-money");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_money_silent_on_canonical_amounts() {
        for md in [
            "금113,560원(금일십일만삼천오백육십원)\n",
            "참석 50여 명\n",
            "34만 5천 원\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_attach_colon_fires_on_line_initial_colon() {
        let findings = only_rule("붙임: 계획서 1부.  끝.\n", "notation-attach-colon");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 1);
        assert_eq!(findings[0].col, 1);
    }

    #[test]
    fn notation_attach_colon_silent_in_prose() {
        for md in [
            "자세한 내용은 붙임 파일을 참고하시기 바랍니다.\n",
            "붙임  계획서 1부.  끝.\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_attach_number_fires_on_single_number_and_missing_period() {
        // One attachment must not carry the `1.` number (inline form).
        let findings = only_rule("붙임  1. 계획서 1부.  끝.\n", "notation-attach-number");
        assert_eq!(findings.len(), 1, "{findings:?}");
        // One attachment in list form: the single-item ordered list is the
        // typed `1.` number.
        let findings = only_rule("붙임\n\n1. 계획서 1부.\n", "notation-attach-number");
        assert_eq!(findings.len(), 1, "{findings:?}");
        // Quantity missing the final period.
        let findings = only_rule("붙임  계획서 1부\n", "notation-attach-number");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_attach_number_silent_on_ladder_and_canonical_single() {
        for md in [
            // Two or more attachments keep the 1. 2. ladder.
            "붙임  1. 계획서 1부.\n      2. 서류 1부.  끝.\n",
            "붙임\n\n1. 계획서 1부.\n2. 서류 1부.\n",
            // One attachment, unnumbered, quantity closed by a period.
            "붙임  계획서 1부.  끝.\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_end_dot_fires_when_gongmun_marker_present() {
        let findings = only_rule("붙임  계획서 1부.  끝\n", "notation-end-dot");
        assert_eq!(findings.len(), 1, "{findings:?}");
        let findings = only_rule("수신  교육부장관\n\n협조하여 주시기 바랍니다.  끝\n", "notation-end-dot");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_end_dot_silent_without_markers() {
        for md in [
            "메모: 오늘 회의 끝\n",
            "붙임  계획서 1부.  끝.\n",
            "회의가 끝나는 대로 공유\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_punctuation_fires_on_keyboard_tilde_and_tight_colon() {
        let findings = only_rule("기간: 2026. 4. 23.~2026. 6. 15.\n", "notation-punctuation");
        assert_eq!(findings.len(), 1, "{findings:?}");
        let findings = only_rule("원장:김갑동\n", "notation-punctuation");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_punctuation_silent_on_canonical_forms() {
        for md in ["기간: 2026. 4. 23.∼2026. 6. 15.\n", "원장: 김갑동\n"] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn ai_style_marks_fires_on_decorative_leading_symbols() {
        for md in ["■ 사업 개요\n", "▶ 추진 방향\n", "※ 참고 사항\n"] {
            let findings = only_rule(md, "ai-style-marks");
            assert_eq!(findings.len(), 1, "{md:?} -> {findings:?}");
            assert_eq!(findings[0].severity, Severity::Warning);
        }
    }

    #[test]
    fn ai_style_marks_silent_on_sanctioned_marks() {
        for md in [
            "□ 회의 명칭\n\n○ 안건\n",
            "★ 발의자 홍길동\n",
            "- 목록 항목\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn fp_guard_url_bare() {
        // A bare URL is plain Text (pulldown-cmark 0.13.4 has no GFM
        // autolinking); the bounded mask keeps every rule silent on it.
        let md = "참고: https://example.go.kr/2026.8.20/notice 입니다\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        let md = "https://example.go.kr/2026.8.20/notice\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
    }

    #[test]
    fn fp_guard_url_inline() {
        // Inline-link destinations and `<…>` autolinks are structurally safe.
        for md in [
            "[공고문](https://example.go.kr/2026.8.20/notice)\n",
            "<https://example.go.kr/2026.8.20/notice>\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }
}
