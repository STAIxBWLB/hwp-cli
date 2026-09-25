//! Builds an HWPX whose one table nests inside its own first cell a given
//! number of times. Shared by the nesting-depth and serve tests (#317).

use std::io::{Read, Write};
use std::path::Path;

/// Markdown for the source document: one 2x2 table whose first cell reads `a`.
pub const TABLE_MARKDOWN: &str = "| a | b |\n|---|---|\n| 1 | 2 |";

/// Rewrites `flat` (made by `hwp new` from [`TABLE_MARKDOWN`]) into `out`, with
/// the table nested `depth` times. Section element depth is `4 + 6 * depth`.
pub fn write_nested_tables(flat: &Path, depth: usize, out: &Path) {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(flat).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(out).unwrap());
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name == "Contents/section0.xml" {
            bytes = nest_first_table(&String::from_utf8(bytes).unwrap(), depth).into_bytes();
        }
        let method = if name == "mimetype" {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        writer
            .start_file(
                &name,
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap();
}

/// Wraps the section's only table into its own first cell until `depth`
/// tables nest.
fn nest_first_table(section: &str, depth: usize) -> String {
    let start = section.find("<hp:tbl").expect("표가 없습니다");
    let end = section.find("</hp:tbl>").expect("표 끝이 없습니다") + "</hp:tbl>".len();
    let table = &section[start..end];
    let marker = r#"<hp:t xml:space="preserve">a</hp:t>"#;
    assert_eq!(
        table.matches(marker).count(),
        1,
        "첫 셀 표식이 하나가 아닙니다"
    );
    let mut nested = String::new();
    for _ in 0..depth {
        nested = table.replace(marker, &format!("{nested}{marker}"));
    }
    format!("{}{nested}{}", &section[..start], &section[end..])
}
