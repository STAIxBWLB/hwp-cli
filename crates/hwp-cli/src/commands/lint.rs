//! `hwp lint` — official-document notation/structure lint (공문서 표기법·구조 검사).
//!
//! Advisory by default: findings never affect the exit code unless `--strict` is
//! given AND at least one error-severity finding exists (D-05). The JSON report
//! follows the hwp-lint-report-v1 field set (D-06; the schema file itself lands
//! with the contract-lock plan). Lint is read-only — the input file is never
//! modified, created or deleted.

use std::io::Read as _;
use std::path::Path;

use hwp_convert::lint::{Finding, LintProfile, Severity, lint_markdown};
use serde_json::{Value, json};

/// stdin read cap, mirroring the MCP `MAX_READ_BYTES` convention.
const MAX_STDIN_BYTES: u64 = 16 * 1024 * 1024;

/// Reads the lint input as markdown. `"-"` reads stdin (bounded, D-08); binary
/// `.hwp`/`.hwpx` feeding is planned but not available yet — named explicitly.
fn read_markdown_input(path: &Path) -> anyhow::Result<String> {
    if path.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .take(MAX_STDIN_BYTES)
            .read_to_string(&mut buf)?;
        return Ok(buf);
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if matches!(ext.as_deref(), Some("hwp" | "hwpx")) {
        anyhow::bail!(
            "아직 지원하지 않는 입력입니다: {} — 바이너리(.hwp/.hwpx) 린트는 추후 지원 예정이며, 지금은 markdown(.md) 파일을 사용하세요",
            path.display()
        );
    }
    Ok(std::fs::read_to_string(path)?)
}

/// Builds the hwp-lint-report-v1 JSON object for already-computed findings (D-06
/// field set; the profile is echoed per D-04).
fn report_json(file: &str, profile: LintProfile, findings: &[Finding]) -> Value {
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
            .map(|f| json!({
                "rule_id": f.rule_id,
                "severity": f.severity.name(),
                "line": f.line,
                "col": f.col,
                "message": f.message,
            }))
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
    match read_markdown_input(path) {
        Ok(markdown) => {
            let findings = lint_markdown(&markdown, profile);
            report_json(&path.display().to_string(), profile, &findings)
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
    let markdown = read_markdown_input(file)?;
    let findings = lint_markdown(&markdown, profile);

    if json {
        let report = report_json(&file.display().to_string(), profile, &findings);
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
