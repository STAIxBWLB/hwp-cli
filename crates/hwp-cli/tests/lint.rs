//! `hwp lint` integration suite — the ROADMAP SC1 acceptance harness (plan
//! 02.3-04). Every SC1 clause gets a named test: the full 10-rule table fires
//! on an all-violations fixture, the 8 embedded templates stay silent, exit
//! codes follow D-05, human output follows D-07, `--json` validates against the
//! published hwp-lint-report-v1 schema (D-06), stdin works (D-08), and binary
//! .hwpx input flows through the same engine with the D-02 note (D-01/D-02).
//!
//! CI-safe by construction: text fixtures written to per-test tmp dirs, .hwpx
//! inputs generated via `hwp new --from` — no committed fixtures, no fonts, no
//! network.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn repo(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hwp-cli-lint-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// The full rule table (ROADMAP SC1 + D-05).
const ALL_RULES: [&str; 10] = [
    "notation-date",
    "notation-time",
    "notation-money",
    "notation-attach-colon",
    "notation-attach-number",
    "notation-end-dot",
    "notation-punctuation",
    "ai-style-marks",
    "struct-item-mark",
    "struct-roman-heading",
];

/// One §6-derived violation per rule ID, plus the sanctioned counter-examples
/// that must stay silent (adopted research Open Question 6: inline `const
/// &str`, no committed data file). The ASCII roman numeral is a paragraph, not
/// a `#` heading — a heading acquires the profile's auto-number prefix
/// (`**1. …**`) on the markdown→IR→markdown round-trip, which would mask the
/// numeral from `struct-roman-heading` on binary input.
const ALL_VIOLATIONS: &str = r"I. 사업 개요

시행: 2020.7.8

회의는 오후 3시 20분에 열립니다.

예산: 345천원

기간: 4. 23.~6. 15.

원장:김갑동

■ 주요 지시 사항

가. 항목 내용

**가. 항목**

□ 개최 결과
○ 참석 현황

# Ⅰ. 개요

제3.01.호에 따라 v2.05.1 기준을 적용합니다.

참고: https://example.com/2020.7.8/notice 및 [공지](https://example.com/1985.09.06/x)입니다.

| 항목 | 내용 |
| --- | --- |
| 날짜 | 2020.7.8 |
| 시각 | 오후 3시 20분 |

```
2020.7.8
오후 3시 20분
```

붙임: 계획서

붙임  1. 계획서 1부

끝
";

/// A warnings-only input (a single notation-date violation, no 공문서 marker,
/// no error-severity rule).
const WARNINGS_ONLY: &str = "날짜: 2020.7.8\n";

/// Parses D-07 human output lines `{line}: {rule_id} — {message}`.
fn findings(stdout: &str) -> Vec<(u32, String)> {
    stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (line_no, rest) = line
                .split_once(": ")
                .unwrap_or_else(|| panic!("line-number prefix missing: {line}"));
            let (rule, _message) = rest
                .split_once(" — ")
                .unwrap_or_else(|| panic!("em-dash rule separator missing: {line}"));
            (
                line_no.parse().expect("numeric line prefix"),
                rule.to_owned(),
            )
        })
        .collect()
}

fn rule_ids(stdout: &str) -> BTreeSet<String> {
    findings(stdout).into_iter().map(|(_, rule)| rule).collect()
}

fn all_rules() -> BTreeSet<String> {
    ALL_RULES.iter().map(|s| (*s).to_owned()).collect()
}

/// `hwp lint <file>`; asserts the run itself succeeded.
fn lint_ok(path: &Path) -> Output {
    let out = hwp().arg("lint").arg(path).output().expect("hwp lint");
    assert!(
        out.status.success(),
        "hwp lint {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Writes `content` to `name` inside `dir` and returns the path.
fn write_md(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// Generates a .hwpx from markdown via `hwp new --from` (the in-test document
/// factory — no committed binary fixtures).
fn new_hwpx(md: &Path, out: &Path) {
    let made = hwp()
        .args(["new", "--from"])
        .arg(md)
        .arg("-o")
        .arg(out)
        .output()
        .expect("hwp new");
    assert!(
        made.status.success(),
        "hwp new --from {}: {}",
        md.display(),
        String::from_utf8_lossy(&made.stderr)
    );
}

/// The published hwp-lint-report-v1 schema as a compiled validator (D-06).
fn lint_report_validator() -> jsonschema::Validator {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/lint-report-v1.schema.json")).unwrap();
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap()
}

#[test]
fn fires_every_rule() {
    let dir = test_dir("fires-every-rule");
    let md = write_md(&dir, "all-violations.md", ALL_VIOLATIONS);

    let out = lint_ok(&md);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        rule_ids(&stdout),
        all_rules(),
        "every rule in the table fires on the all-violations fixture:\n{stdout}"
    );

    // Per-rule counts pin the sanctioned counter-examples silent: the URLs
    // (inline + bare), 제3.01.호, v2.05.1, the `| --- |` table and the fenced
    // code block add nothing; the `□ `/`○ ` ladder and the U+2160 heading add
    // nothing; the bold-wrapped `**가. 항목**` yields exactly one finding.
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for (_, rule) in findings(&stdout) {
        *counts.entry(rule).or_default() += 1;
    }
    let count = |rule: &str| counts.get(rule).copied().unwrap_or(0);
    assert_eq!(count("notation-date"), 1, "URL/ho-citation/version masks");
    assert_eq!(count("notation-time"), 1, "table/code blocks stay silent");
    assert_eq!(count("notation-money"), 1);
    assert_eq!(
        count("notation-punctuation"),
        2,
        "tilde range + tight colon"
    );
    assert_eq!(count("ai-style-marks"), 1, "□ /○ ladder stays silent");
    assert_eq!(
        count("struct-item-mark"),
        2,
        "typed mark + exactly one bold-wrapped mark"
    );
    assert_eq!(
        count("struct-roman-heading"),
        1,
        "U+2160 heading stays silent"
    );
    assert_eq!(count("notation-attach-colon"), 1);
    assert_eq!(
        count("notation-attach-number"),
        2,
        "typed `1.` + `1부` quantity"
    );
    assert_eq!(count("notation-end-dot"), 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn silent_on_embedded_templates() {
    let templates_dir = repo("skills/hwp/templates");
    let mut templates: Vec<PathBuf> = std::fs::read_dir(&templates_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    templates.sort();
    assert_eq!(
        templates.len(),
        8,
        "the embedded template corpus is exactly 8 files: {templates:?}"
    );
    for template in templates {
        let out = lint_ok(&template);
        assert!(
            out.stdout.is_empty(),
            "{}: zero findings expected, got:\n{}",
            template.display(),
            String::from_utf8_lossy(&out.stdout)
        );
    }
}

#[test]
fn exit_code_policy() {
    let dir = test_dir("exit-code-policy");
    let all = write_md(&dir, "all-violations.md", ALL_VIOLATIONS);
    let warnings = write_md(&dir, "warnings-only.md", WARNINGS_ONLY);

    // D-05: findings are advisory — the default run exits 0 even with
    // error-severity findings present.
    let default = hwp().arg("lint").arg(&all).output().unwrap();
    assert!(
        default.status.success(),
        "default exits 0 with findings present: {}",
        String::from_utf8_lossy(&default.stderr)
    );

    // --strict exits 1 only when an error-severity finding exists (the
    // fixture carries struct-item-mark and struct-roman-heading).
    let strict = hwp().args(["lint", "--strict"]).arg(&all).output().unwrap();
    assert!(
        !strict.status.success(),
        "--strict exits 1 on error-severity findings"
    );

    // --strict with warnings only still exits 0.
    let strict_warnings = hwp()
        .args(["lint", "--strict"])
        .arg(&warnings)
        .output()
        .unwrap();
    assert!(
        strict_warnings.status.success(),
        "--strict exits 0 on warnings-only input: {}",
        String::from_utf8_lossy(&strict_warnings.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn json_matches_schema_v1() {
    let dir = test_dir("json-schema");
    let validator = lint_report_validator();

    // Markdown input — findings carry no `context` (no conversion involved).
    let md = write_md(&dir, "all-violations.md", ALL_VIOLATIONS);
    let out = hwp()
        .args(["lint", "--json"])
        .arg(&md)
        .output()
        .expect("hwp lint --json");
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        validator.is_valid(&value),
        "schema rejected markdown report: {value}"
    );
    assert_eq!(value["contract"], "hwp-lint-report-v1");
    assert_eq!(value["schema_version"], "1.0");
    let md_findings = value["findings"].as_array().unwrap();
    assert!(!md_findings.is_empty());
    for finding in md_findings {
        assert!(
            finding.get("context").is_none(),
            "markdown findings carry no context: {finding}"
        );
    }

    // Generated .hwpx input — every finding carries the D-02 context note.
    let hwpx = dir.join("all-violations.hwpx");
    new_hwpx(&md, &hwpx);
    let out = hwp()
        .args(["lint", "--json"])
        .arg(&hwpx)
        .output()
        .expect("hwp lint --json");
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        validator.is_valid(&value),
        "schema rejected binary report: {value}"
    );
    let bin_findings = value["findings"].as_array().unwrap();
    assert!(!bin_findings.is_empty());
    for finding in bin_findings {
        assert!(
            finding["context"]
                .as_str()
                .unwrap_or_default()
                .contains("markdown 변환 경유"),
            "binary findings carry the D-02 note: {finding}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn human_output_format() {
    let dir = test_dir("human-output-format");
    let md = write_md(&dir, "all-violations.md", ALL_VIOLATIONS);

    let out = lint_ok(&md);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.is_empty(), "the fixture produces findings");

    // D-07: every line is `{line}: {rule_id} — {message}`, line numbers
    // ascending, no headers or context lines.
    let mut previous = 0;
    for line in stdout.lines() {
        let (line_no, rest) = line
            .split_once(": ")
            .unwrap_or_else(|| panic!("line must start with `N: `: {line}"));
        let number: u32 = line_no
            .parse()
            .unwrap_or_else(|_| panic!("numeric line prefix: {line}"));
        let (rule, message) = rest
            .split_once(" — ")
            .unwrap_or_else(|| panic!("`rule — message` shape: {line}"));
        assert!(
            !rule.is_empty() && rule.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
            "kebab-case rule id: {line}"
        );
        assert!(!message.is_empty(), "non-empty message: {line}");
        assert!(
            number >= previous,
            "ascending line order: {number} after {previous}"
        );
        previous = number;
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stdin_markdown() {
    let dir = test_dir("stdin-markdown");
    let md = write_md(&dir, "all-violations.md", ALL_VIOLATIONS);

    // D-08: `hwp lint -` lints piped stdin as markdown; the findings match
    // the file run exactly (human output carries no file name).
    let file_run = lint_ok(&md);
    let mut child = hwp()
        .args(["lint", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hwp lint -");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(ALL_VIOLATIONS.as_bytes())
        .unwrap();
    let piped = child.wait_with_output().unwrap();
    assert!(
        piped.status.success(),
        "stdin run exits 0: {}",
        String::from_utf8_lossy(&piped.stderr)
    );
    assert_eq!(
        piped.stdout, file_run.stdout,
        "stdin findings match the file run"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn binary_input_via_markdown() {
    let dir = test_dir("binary-input");
    let md = write_md(&dir, "all-violations.md", ALL_VIOLATIONS);
    let hwpx = dir.join("all-violations.hwpx");
    new_hwpx(&md, &hwpx);

    // D-01: one engine, two feeders — the binary input fires the same rule
    // table through the shared load_document → to_markdown_with read path.
    let out = lint_ok(&hwpx);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        rule_ids(&stdout),
        all_rules(),
        "binary input fires every rule via markdown conversion:\n{stdout}"
    );

    // D-02: every JSON finding carries the via-markdown-conversion context.
    let out = hwp()
        .args(["lint", "--json"])
        .arg(&hwpx)
        .output()
        .expect("hwp lint --json");
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let bin_findings = value["findings"].as_array().unwrap();
    assert!(!bin_findings.is_empty());
    for finding in bin_findings {
        assert!(
            finding["context"]
                .as_str()
                .unwrap_or_default()
                .contains("markdown 변환 경유"),
            "D-02 context on every finding: {finding}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
