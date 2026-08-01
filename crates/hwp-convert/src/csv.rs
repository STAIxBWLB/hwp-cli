//! IR → CSV (table extraction, GJ-5).
//!
//! Emits every table in the document as CSV in order of appearance. Rows follow grid order
//! (row, col); merged cells write only the origin value and leave covered slots empty. Tables
//! are separated by one blank line.

use hwp_model::{Control, Document, HwpChar, Paragraph, Table};

/// Serializes every table in the document to CSV. Empty string if there are no tables.
pub fn to_csv(doc: &Document) -> String {
    let mut out = String::new();
    let mut first = true;
    for section in &doc.sections {
        for para in &section.paragraphs {
            write_para_tables(para, &mut out, &mut first);
        }
    }
    out
}

fn write_para_tables(para: &Paragraph, out: &mut String, first: &mut bool) {
    for control in &para.controls {
        match control {
            Control::Table(table) => {
                if !*first {
                    out.push('\n');
                }
                *first = false;
                write_table(table, out, first);
            }
            Control::Generic(g) => {
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        write_para_tables(p, out, first);
                    }
                }
            }
            _ => {}
        }
    }
}

fn write_table(table: &Table, out: &mut String, first: &mut bool) {
    let cols = table.cols.max(1) as usize;
    let rows = table.rows.max(1) as usize;
    for r in 0..rows {
        let mut line: Vec<String> = vec![String::new(); cols];
        for cell in table.cells.iter().filter(|c| c.row as usize == r) {
            if let Some(slot) = line.get_mut(cell.col as usize) {
                *slot = escape_csv(&cell_text(cell));
            }
        }
        out.push_str(&line.join(","));
        out.push('\n');
    }
    // Nested tables inside cells are also emitted in order of appearance (module promise: every table in the document).
    for cell in &table.cells {
        for p in &cell.paragraphs {
            write_para_tables(p, out, first);
        }
    }
}

fn cell_text(cell: &hwp_model::Cell) -> String {
    let mut text = String::new();
    for p in &cell.paragraphs {
        for ch in &p.chars {
            if let HwpChar::Text(c) = ch {
                text.push(*c);
            }
        }
        text.push(' ');
    }
    text.trim().to_string()
}

/// RFC 4180 — if the value contains `,` `"` or a newline, wrap it in quotes and double the quotes.
fn escape_csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 표를_csv로() {
        let doc = crate::from_markdown::from_markdown(
            "본문\n\n| 이름, 호 | 메모 \"쌍따옴표\" |\n|---|---|\n| 1 | 2 |\n",
        );
        let csv = to_csv(&doc);
        assert!(
            csv.contains("\"이름, 호\",\"메모 \"\"쌍따옴표\"\"\""),
            "이스케이프: {csv}"
        );
        assert!(csv.contains("1,2"), "행: {csv}");
    }

    #[test]
    fn 중첩_표도_추출() {
        // 바깥 표 셀 안의 중첩 표도 빠짐없이 낸다 (리뷰 회귀).
        let mut doc = crate::from_markdown::from_markdown(
            "본문\n\n<table><tr><td>바깥<table><tr><td>중첩</td></tr></table></td></tr></table>\n",
        );
        let csv = to_csv(&doc);
        assert!(csv.contains("바깥"), "바깥: {csv}");
        assert!(csv.contains("중첩"), "중첩: {csv}");
        let _ = &mut doc;
    }
}
