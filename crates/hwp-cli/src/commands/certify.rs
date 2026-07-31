//! `hwp certify` CLI wrapper. The implementation is shared with MCP in the library module.

use std::path::Path;

pub fn run(input: &Path, policy: &Path, report: &Path) -> anyhow::Result<()> {
    let outcome = hwp_cli::certification::execute(input, policy, report)?;
    println!(
        "{}",
        serde_json::json!({
            "overall": outcome.overall,
            "report": outcome.report_dir,
        })
    );
    if outcome.overall == hwp_cli::certification::OverallStatus::Passed {
        Ok(())
    } else {
        anyhow::bail!("certification did not pass; see the atomic report directory")
    }
}
