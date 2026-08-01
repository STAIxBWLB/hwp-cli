//! IR → CSV (표 추출, GJ-5).
//!
//! 문서의 모든 표를 등장 순서대로 CSV로 낸다. 행은 그리드 순서(row, col),
//! 병합 셀은 origin 값만 쓰고 덮인 칸은 비운다. 표와 표 사이는 빈 줄 하나.

use hwp_model::{Control, Document, HwpChar, Paragraph, Table};

/// 문서의 모든 표를 CSV로 직렬화한다. 표가 없으면 빈 문자열.
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
                write_table(table, out);
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

fn write_table(table: &Table, out: &mut String) {
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

/// RFC 4180 — `,` `"` 개행이 있으면 따옴표로 싸고 따옴표는 두 겹으로.
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
}
