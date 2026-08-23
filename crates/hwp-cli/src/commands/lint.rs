//! `hwp lint` — official-document notation/structure lint (공문서 표기법·구조 검사).
//!
//! Advisory by default: findings never affect the exit code unless `--strict` is
//! given AND at least one error-severity finding exists (D-05). The JSON report
//! follows the hwp-lint-report-v1 contract (D-06, published at
//! `schemas/lint-report-v1.schema.json` — additive-only thereafter). Lint is
//! read-only — the input file is never modified, created or deleted.

use std::io::Read as _;
use std::path::Path;

use hwp_convert::lint::{Finding, LintProfile, Severity, lint_markdown};
use serde_json::{Value, json};

/// stdin read cap, mirroring the MCP `MAX_READ_BYTES` convention.
const MAX_STDIN_BYTES: u64 = 16 * 1024 * 1024;

/// D-02 note carried in every finding's `context` when the input reached the
/// engine via markdown conversion from a binary .hwp/.hwpx. A fixed string,
/// never content (T-02.3-04); it names the conversion path and the merged-cell
/// HTML-block blind spot (RESEARCH Pitfall 3) so a conversion artifact is never
/// mistaken for a source-document defect.
const VIA_CONVERSION_NOTE: &str =
    "markdown 변환 경유 입력입니다 — 병합 셀 표(HTML 블록) 안의 내용은 검사하지 않습니다";

/// The lint input: markdown text plus whether it arrived via binary→markdown
/// conversion (drives the D-02 `context` note in the JSON report).
struct LintInput {
    markdown: String,
    via_conversion: bool,
}

/// Bounded UTF-8 read (D-08 / T-02.3-02): input beyond `cap` bytes is a Korean
/// error naming the limit — never a silent truncation.
fn read_bounded(reader: impl std::io::Read, cap: u64) -> anyhow::Result<String> {
    let mut buf = String::new();
    reader.take(cap + 1).read_to_string(&mut buf)?;
    if buf.len() as u64 > cap {
        anyhow::bail!("입력이 크기 제한({cap}바이트)을 초과했습니다");
    }
    Ok(buf)
}

/// Reads the lint input as markdown. `"-"` reads stdin (bounded, D-08 — never
/// staged to a temp file, RESEARCH Pitfall 6); binary `.hwp`/`.hwpx` flows
/// through the shared read path (`load_document` → `to_markdown_with`) into the
/// SAME engine — one engine, two feeders (D-01). Read-only throughout: no
/// writer is ever invoked on the input.
fn read_lint_input(path: &Path) -> anyhow::Result<LintInput> {
    if path.as_os_str() == "-" {
        return Ok(LintInput {
            markdown: read_bounded(std::io::stdin(), MAX_STDIN_BYTES)?,
            via_conversion: false,
        });
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if matches!(ext.as_deref(), Some("hwp" | "hwpx")) {
        let doc = crate::commands::cat::load_document(path)?;
        let markdown =
            hwp_convert::to_markdown_with(&doc, &hwp_convert::MarkdownOptions::default())?;
        return Ok(LintInput {
            markdown,
            via_conversion: true,
        });
    }
    Ok(LintInput {
        markdown: std::fs::read_to_string(path)?,
        via_conversion: false,
    })
}

/// Builds the hwp-lint-report-v1 JSON object for already-computed findings (D-06
/// field set; the profile is echoed per D-04). When the input arrived via
/// binary→markdown conversion, every finding carries the D-02 `context` note.
fn report_json(
    file: &str,
    profile: LintProfile,
    findings: &[Finding],
    via_conversion: bool,
) -> Value {
    let error_count = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warning_count = findings.len() - error_count;
    json!({
        "schema_version": "1.0",
        "contract": "hwp-lint-report-v1",
        "file": file,
        "profile": profile.name(),
        "findings": findings
            .iter()
            .map(|f| {
                let mut finding = json!({
                    "rule_id": f.rule_id,
                    "severity": f.severity.name(),
                    "line": f.line,
                    "col": f.col,
                    "message": f.message,
                });
                if via_conversion {
                    finding["context"] = json!(VIA_CONVERSION_NOTE);
                }
                finding
            })
            .collect::<Vec<_>>(),
        "summary": {
            "error_count": error_count,
            "warning_count": warning_count,
        },
    })
}

/// Lint result as a JSON object (shared CLI/MCP entry — no `process::exit`, no
/// stdout). An unreadable or unsupported input is reported in the `error` field
/// instead of panicking or exiting.
pub fn lint_path_json(path: &Path, profile: LintProfile) -> Value {
    match read_lint_input(path) {
        Ok(input) => {
            let findings = lint_markdown(&input.markdown, profile);
            report_json(
                &path.display().to_string(),
                profile,
                &findings,
                input.via_conversion,
            )
        }
        Err(error) => json!({
            "schema_version": "1.0",
            "contract": "hwp-lint-report-v1",
            "file": path.display().to_string(),
            "profile": profile.name(),
            "error": error.to_string(),
            "findings": [],
            "summary": { "error_count": 0, "warning_count": 0 },
        }),
    }
}

pub fn run(file: &Path, profile: LintProfile, json: bool, strict: bool) -> anyhow::Result<()> {
    let input = read_lint_input(file)?;
    let findings = lint_markdown(&input.markdown, profile);

    if json {
        let report = report_json(
            &file.display().to_string(),
            profile,
            &findings,
            input.via_conversion,
        );
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        // D-07: one `{line}: {rule_id} — {message}` line per finding, ascending
        // line order — no headers, no context lines.
        for f in &findings {
            println!("{}: {} — {}", f.line, f.rule_id, f.message);
        }
    }

    // D-05 exit policy: findings are advisory — the exit code stays 0 unless
    // --strict is given AND an error-severity finding exists.
    if strict && findings.iter().any(|f| f.severity == Severity::Error) {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_read_accepts_input_within_the_cap() {
        let text = read_bounded(std::io::Cursor::new("시행: 2020.7.8\n"), MAX_STDIN_BYTES).unwrap();
        assert_eq!(text, "시행: 2020.7.8\n");
    }

    #[test]
    fn bounded_read_rejects_over_cap_naming_the_limit() {
        // A small stand-in cap keeps the test fast; the message must name it.
        let err = read_bounded(std::io::Cursor::new(vec![b'a'; 65]), 64).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("64바이트"),
            "error must name the limit: {message}"
        );
    }

    #[test]
    fn error_variant_report_matches_schema_v1() {
        // WR-1: the MCP-facing failure report is a deliberate lint-report-v1
        // variant (same contract, empty findings, optional `error` string) and
        // must validate against the published schema.
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../schemas/lint-report-v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap();
        let report = lint_path_json(
            Path::new("/nonexistent/hwp-lint-wr1-does-not-exist.md"),
            LintProfile::Gongmun,
        );
        assert!(
            report.get("error").is_some(),
            "unreadable input reports the error field: {report}"
        );
        assert!(
            validator.is_valid(&report),
            "schema rejected the error-variant report: {report}"
        );
    }
}
