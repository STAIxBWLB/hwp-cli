//! Owner-authored synthetic forms from #410; no private corpus or font golden.
use std::path::Path;
use std::process::Command;

use serde_json::json;

fn run(args: &[&str], dir: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn synthetic_form_breaks_restart_page_relative_cache() {
    let dir = std::env::temp_dir().join(format!("hwp-410-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let para = |text: String| json!({"type":"paragraph", "style":"body", "runs":[{"type":"text", "text":text}]});
    let mut blocks = Vec::new();
    for n in 0..4 {
        if n > 0 { blocks.push(json!({"type":"break", "kind":"page"})); }
        blocks.push(para(format!("Public form {}", n + 1)));
        let rows: Vec<_> = (0..12).map(|j| json!({"cells":[
            {"blocks":[para(format!("Field {}", j+1))]},
            {"blocks":[para("Synthetic content for deterministic pagination. ".repeat(3)),
                para(format!("Second paragraph with an explicit test marker {n}-{j}"))]}
        ]})).collect();
        blocks.push(json!({"type":"table", "width_mm":160,
            "columns":[{"width_mm":35},{"width_mm":125}], "rows": rows}));
    }
    let spec = json!({"version":"1.0", "metadata":{"title":"Synthetic form pagination probe", "author":"hwp-cli regression probe"},
        "page":{"width_mm":210,"height_mm":297,"margin_left_mm":20,"margin_right_mm":20,"margin_top_mm":20,"margin_bottom_mm":20},
        "styles":{"body":{"font_family":"Noto Sans CJK KR","font_size_pt":10,"align":"left","line_height_percent":120}},
        "sections":[{"blocks":blocks}]});
    std::fs::write(dir.join("probe.json"), serde_json::to_vec_pretty(&spec).unwrap()).unwrap();
    run(&["compose","probe.json","-o","probe.hwpx"], &dir);
    run(&["edit","probe.hwpx","-o","probe.hwpx","--table-placement","floating","--verify"], &dir);
    run(&["convert","probe.hwpx","-o","probe.hwp","--strict","--loss-report","loss.json"], &dir);
    let before = hwpx::read_document(dir.join("probe.hwpx")).unwrap();
    let after = hwp5::read_document(dir.join("probe.hwp")).unwrap().document;
    let source_paras = &before.sections[0].paragraphs;
    let output_paras = &after.sections[0].paragraphs;
    assert_eq!(source_paras.len(), output_paras.len());
    let mut breaks = 0;
    for (source, output) in source_paras.iter().zip(output_paras) {
        assert_eq!(source.header.break_type & 4, output.header.break_type & 4);
        if source.header.break_type & 4 != 0 {
            breaks += 1;
            assert_eq!(output.line_segs.first().unwrap().v_pos, 0,
                "explicit form-start break must restart the synthesized page-relative anchor");
        }
    }
    assert_eq!(breaks, 3);
    std::fs::remove_dir_all(dir).unwrap();
}
