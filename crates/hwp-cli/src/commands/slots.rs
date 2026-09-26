//! `hwp slots` — `{{name}}` 텍스트 자리표시자 스캔.
//!
//! 누름틀(form field, `hwp fields`)과 별개로, 순수 텍스트 `{{...}}` 템플릿의
//! 자리표시자를 등장 순서로 나열한다. `hwp edit --replace "{{name}}=>값"` 으로 채운다.
//! `--forms` also lists Korean form fields (`hwp_convert::scan_form_fields`), which
//! `hwp fill --forms` fills.

use std::path::Path;

use crate::commands::cat::load_document;

pub fn run(path: &Path, json: bool, forms: bool) -> anyhow::Result<()> {
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
