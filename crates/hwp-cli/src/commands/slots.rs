//! `hwp slots`: text-placeholder and explicit form-occurrence discovery.
//!
//! Ordinary slots are distinct from native fields (`hwp fields`). Exact occurrences are an
//! opt-in local inventory; they include document text and must not be used as a public receipt.
//! `--forms` also lists Korean form fields (`hwp_convert::scan_form_fields`), which
//! `hwp fill --forms` fills.

use std::path::Path;

use crate::commands::cat::load_document;

pub fn run(path: &Path, json: bool, forms: bool, occurrences: bool) -> anyhow::Result<()> {
    if occurrences {
        println!("{}", serde_json::to_string_pretty(&discover(path)?)?);
        return Ok(());
    }
    let doc = load_document(path)?;
    let slots = hwp_convert::scan_placeholders(&doc);

    if json {
        let items: Vec<serde_json::Value> = slots
            .iter()
            .map(|p| serde_json::json!({ "name": p.name, "occurrences": p.occurrences }))
            .collect();
        let mut out = serde_json::json!({ "placeholders": items });
        if forms {
            out["fields"] = form_fields_json(&hwp_convert::scan_form_fields(&doc));
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if forms {
        let fields = hwp_convert::scan_form_fields(&doc);
        if fields.is_empty() {
            eprintln!("양식 필드 없음");
        }
        for f in &fields {
            println!("{}\t{}\t{}", f.key, f.source.as_str(), f.occurrences);
        }
    } else if slots.is_empty() {
        eprintln!("자리표시자 없음");
    } else {
        for p in &slots {
            println!("{}\t{}", p.name, p.occurrences);
        }
    }
    Ok(())
}

/// Hash and parse the same immutable private copy, independent of later source replacements.
pub(crate) fn discover(path: &Path) -> anyhow::Result<hwp_convert::form::binding::FormDiscovery> {
    use super::output::{SnapshotOutputMode, write_with_private_input_snapshot};
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| {
        anyhow::anyhow!(
            "discovery 임시 입력을 준비할 수 없습니다. 시스템 난수 상태를 확인한 뒤 다시 실행하세요"
        )
    })?;
    let destination = std::env::temp_dir().join(format!(
        "hwp-form-discovery-{}.json",
        hwp_convert::form::binding::digest(&nonce)
    ));
    let (_, result) = write_with_private_input_snapshot(
        &destination,
        path,
        64 * 1024 * 1024,
        SnapshotOutputMode::PlanOnly,
        |snapshot, _, hash| {
            let doc = load_document(snapshot)?;
            hwp_convert::form::binding::scan_form_occurrences(&doc, hash)
                .map_err(anyhow::Error::msg)
        },
        |_, _| Ok(()),
    )?;
    Ok(result)
}

/// The `fields` array of `hwp slots --json --forms` and MCP `hwp_slots` with `forms`.
pub fn form_fields_json(fields: &[hwp_convert::FormField]) -> serde_json::Value {
    fields
        .iter()
        .map(|f| {
            serde_json::json!({
                "key": f.key,
                "label": f.label,
                "source": f.source.as_str(),
                "confidence": f.source.confidence(),
                "occurrences": f.occurrences,
                "required": f.required,
            })
        })
        .collect()
}
