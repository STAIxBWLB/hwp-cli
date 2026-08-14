//! 필드/누름틀 읽기·채우기 (한컴 GetFieldList/PutFieldText 동등).
//!
//! HWP 필드 = FIELD_START(문자 코드 3, 확장 컨트롤) ... 표시 텍스트 ... FIELD_END(코드 4).
//! - 종류: 컨트롤 ID (%clk=누름틀, %fmu=계산식, %hlk=하이퍼링크 등).
//! - 이름: CTRL_DATA(HWPTAG, Parameter Set)의 첫 BSTR — 누름틀에만 보통 존재.
//! - 명령: 필드 레코드의 command 문자열(계산식 식, 누름틀 지시문 등).
//! - 값: FIELD_START~FIELD_END 사이 텍스트.
//!
//! IR을 바꾸지 않고 온디맨드로 파싱한다(리더/writer/왕복 무영향). 채우기는 값 영역만
//! 교체하고 char_shape_run을 보정한다 — 쓰기는 편집 경로(write_hwp_edited)를 거친다.

use hwp_model::opaque::OpaqueRecord;
use hwp_model::{CharShapeId, Control, Document, GenericControl, HwpChar, Paragraph};

use crate::edit::{adjust_runs, find_match, utf16_len};

const CTRL_DATA_TAG: u16 = 0x0010 + 71; // HWPTAG_CTRL_DATA
const FIELD_START: u16 = 3;
const FIELD_END: u16 = 4;

/// 필드 하나의 정보.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    /// 종류 표시명(누름틀/계산식/...).
    pub kind: String,
    /// 컨트롤 ID 원문(예: "%clk").
    pub ctrl_id: String,
    /// 필드 이름(누름틀; CTRL_DATA). 없으면 None.
    pub name: Option<String>,
    /// 명령/지시문(필드 레코드 command). 없으면 None.
    pub command: Option<String>,
    /// 화면 표시 값.
    pub value: String,
}

/// 필드 종류 컨트롤 ID인지(누름틀·계산식·하이퍼링크 등). 스펙 표 128의 34종 중
/// **메모(`%%me`)를 제외한 33종**을 인식한다. 인식하지 못하면 hwpx write가 필드를
/// **통째 드롭**(catch-all)하므로, 종류 라벨을 몰라도 최소한 fieldBegin/End·내용
/// 텍스트는 보존하도록 목록에 담는다(OWPML type은 [`owpml_field_type`]의 UNKNOWN
/// 폴백 — GF-1과 동일 손실 등급). `%%me`는 메모 리스트(GB-7)와 짝을 이뤄야 하는데
/// 그 본문은 아직 미방출이라, 근거 없이 dangling 메모 필드를 내보내면 한글에서
/// 문서가 깨질 수 있어 **의도적으로 보류**한다(정답지 필요).
pub fn is_field_ctrl_id(ctrl_id: &[u8; 4]) -> bool {
    matches!(
        ctrl_id,
        // 기존 12종(표 128).
        b"%clk"
            | b"%fmu"
            | b"%hlk"
            | b"%mmg"
            | b"%dte"
            | b"%ddt"
            | b"%xrf"
            | b"%bmk"
            | b"%pat"
            | b"%smr"
            | b"%usr"
            | b"%unk"
            // 변경추적 필드 19종(FIELD_REVISION_*, 표 128).
            | b"%sig"
            | b"%spl"
            | b"%%mr"
            | b"%%*d"
            | b"%%*a"
            | b"%%*C"
            | b"%%*S"
            | b"%%*T"
            | b"%%*P"
            | b"%%*L"
            | b"%%*c"
            | b"%%*h"
            | b"%%*A"
            | b"%%*i"
            | b"%%*t"
            | b"%%*r"
            | b"%%*l"
            | b"%%*n"
            | b"%%*e"
            // 개인정보 보안·차례(표 128). 메모(%%me)는 GB-7 연동 우려로 보류.
            | b"%cpr"
            | b"%toc"
    )
}

/// 컨트롤 ID → OWPML fieldBegin type 속성.
pub fn owpml_field_type(ctrl_id: &[u8; 4]) -> &'static str {
    match ctrl_id {
        b"%clk" => "CLICK_HERE",
        b"%fmu" => "FORMULA",
        b"%hlk" => "HYPERLINK",
        b"%mmg" => "MAIL_MERGE",
        b"%dte" => "DATE",
        b"%ddt" => "DOCUMENT_DATE",
        b"%xrf" => "CROSS_REF",
        b"%bmk" => "BOOKMARK",
        b"%pat" => "PATH",
        b"%smr" => "SUMMARY",
        b"%usr" => "USER_INFO",
        // 차례 — 정답지 코퍼스 실측(과업지시서 hwpx `type="TABLEOFCONTENTS"`).
        b"%toc" => "TABLEOFCONTENTS",
        // 변경추적 19종·개인정보(%cpr)는 OWPML type 이름 근거가 없어 UNKNOWN 폴백
        // (fieldBegin/End·내용은 보존, 종류 라벨만 상실 — GF-1 등급).
        _ => "UNKNOWN",
    }
}

/// OWPML fieldBegin type → 컨트롤 ID(역매핑, 미지는 `%unk`).
pub fn field_ctrl_id_from_owpml(t: &str) -> [u8; 4] {
    match t {
        "CLICK_HERE" => *b"%clk",
        "FORMULA" => *b"%fmu",
        "HYPERLINK" => *b"%hlk",
        "MAIL_MERGE" => *b"%mmg",
        "DATE" => *b"%dte",
        "DOCUMENT_DATE" => *b"%ddt",
        "CROSS_REF" | "CROSS_REFERENCE" => *b"%xrf",
        "BOOKMARK" => *b"%bmk",
        "PATH" | "FILE_PATH" => *b"%pat",
        "SUMMARY" => *b"%smr",
        "USER_INFO" => *b"%usr",
        "TABLEOFCONTENTS" => *b"%toc",
        _ => *b"%unk",
    }
}

fn kind_of(ctrl_id: &[u8; 4]) -> &'static str {
    match ctrl_id {
        b"%clk" => "누름틀",
        b"%fmu" => "계산식",
        b"%hlk" => "하이퍼링크",
        b"%mmg" => "메일머지",
        b"%dte" => "날짜",
        b"%ddt" => "문서날짜",
        b"%xrf" => "상호참조",
        b"%bmk" => "책갈피",
        b"%pat" => "파일경로",
        b"%smr" => "문서요약",
        b"%usr" => "사용자정보",
        b"%cpr" => "개인정보보호",
        b"%toc" => "차례",
        b"%unk" => "알수없음",
        _ => "필드", // 변경추적(FIELD_REVISION_*) 등은 일반 라벨
    }
}

/// 문서의 모든 필드를 등장 순서로 나열한다(본문·표 셀·글상자 재귀).
pub fn list_fields(doc: &Document) -> Vec<FieldInfo> {
    let mut out = Vec::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            collect_fields(para, &mut out);
        }
    }
    out
}

fn collect_fields(para: &Paragraph, out: &mut Vec<FieldInfo>) {
    for (i, ch) in para.chars.iter().enumerate() {
        if let HwpChar::ExtCtrl {
            code,
            ctrl_id,
            ctrl_index,
            ..
        } = ch
            && *code == FIELD_START
        {
            let (name, command) = ctrl_index
                .and_then(|idx| para.controls.get(idx as usize))
                .map(field_meta)
                .unwrap_or((None, None));
            out.push(FieldInfo {
                kind: kind_of(ctrl_id).to_string(),
                ctrl_id: String::from_utf8_lossy(ctrl_id).into_owned(),
                name,
                command,
                value: field_value(&para.chars, i),
            });
        }
    }
    for ctrl in &para.controls {
        for_each_nested(ctrl, &mut |p| collect_fields(p, out));
    }
}

/// FIELD_START(start) 다음부터 FIELD_END 전까지의 표시 텍스트.
fn field_value(chars: &[HwpChar], start: usize) -> String {
    let mut s = String::new();
    for ch in &chars[start + 1..] {
        match ch {
            HwpChar::Text(c) => s.push(*c),
            HwpChar::CharCtrl(10) => s.push('\n'),
            HwpChar::InlineCtrl { code, .. } if *code == FIELD_END => break,
            _ => {}
        }
    }
    s
}

/// %hlk 하이퍼링크 컨트롤이면 대상 URL을 복원해 돌려준다(md/html 링크 방출용).
/// 표시 텍스트는 FIELD_START~FIELD_END 사이 문자열이고, URL은 커맨드
/// (`{URL};1;0;0;`)에 담긴다 — 첫 비이스케이프 `;` 전까지가 URL이며
/// [`hlk_command`]의 백슬래시 이스케이프를 되돌린다.
pub fn hyperlink_url(ctrl: &Control) -> Option<String> {
    let Control::Generic(g) = ctrl else {
        return None;
    };
    if &g.ctrl_id != b"%hlk" {
        return None;
    }
    let cmd = parse_command(&g.data)?;
    Some(unescape_hlk_url(&cmd))
}

/// [`hlk_command`]의 역: 첫 비이스케이프 `;`(필드 구분자) 전까지를 URL로 보고
/// `\\`·`\;`·`\:` 등 백슬래시 이스케이프를 되돌린다.
fn unescape_hlk_url(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut it = cmd.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                if let Some(n) = it.next() {
                    out.push(n);
                }
            }
            ';' => break,
            _ => out.push(c),
        }
    }
    out
}

/// 필드 컨트롤에서 (이름, 명령)을 읽는다.
pub fn field_meta(ctrl: &Control) -> (Option<String>, Option<String>) {
    let Control::Generic(g) = ctrl else {
        return (None, None);
    };
    let name = g
        .raw_children
        .iter()
        .find(|r| r.tag == CTRL_DATA_TAG)
        .and_then(|r| first_bstr(&r.data));
    (name, parse_command(&g.data))
}

/// 필드 레코드(ctrl_id 제거됨) command 문자열: 속성(4) 기타(1) len(2) WCHAR[len] id(4).
fn parse_command(data: &[u8]) -> Option<String> {
    if data.len() < 7 {
        return None;
    }
    let len = u16::from_le_bytes([data[5], data[6]]) as usize;
    let end = 7usize.checked_add(len * 2)?;
    if end > data.len() || len == 0 {
        return None;
    }
    Some(decode_utf16le(&data[7..end]))
}

/// CTRL_DATA Parameter Set의 첫 BSTR(필드 이름). 알 수 없는 항목 타입을 만나면 중단.
fn first_bstr(data: &[u8]) -> Option<String> {
    if data.len() < 4 {
        return None;
    }
    let count = i16::from_le_bytes([data[2], data[3]]).max(0) as usize;
    let mut o = 4usize;
    for _ in 0..count {
        if o + 4 > data.len() {
            break;
        }
        let item_type = u16::from_le_bytes([data[o + 2], data[o + 3]]);
        o += 4;
        match item_type {
            0 => {} // PIT_NULL
            1 => {
                // PIT_BSTR: UINT32 len + WCHAR[len]
                if o + 4 > data.len() {
                    break;
                }
                let len =
                    u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
                o += 4;
                let end = o.checked_add(len * 2)?;
                if end > data.len() {
                    break;
                }
                return Some(decode_utf16le(&data[o..end]));
            }
            2 | 6 => o += 1,         // I1/UI1
            3 | 7 => o += 2,         // I2/UI2
            4 | 5 | 8 | 9 => o += 4, // I4/I/UI4/UI
            _ => break,              // SET/ARRAY/BINDATA/미지 — 안전 중단
        }
    }
    None
}

fn decode_utf16le(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// 이름이 일치하는 필드의 값을 `value`로 채운다. 반환=채운 개수.
pub fn set_field(doc: &mut Document, name: &str, value: &str) -> usize {
    let mut count = 0;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            count += set_field_para(para, name, value);
        }
    }
    count
}

fn set_field_para(para: &mut Paragraph, name: &str, value: &str) -> usize {
    // 불변 스캔: 이름 일치 필드의 (값 시작 char idx, 값 끝 char idx, 값 WCHAR 위치, 옛 값 WCHAR 길이).
    let mut regions: Vec<(usize, usize, u32, u32)> = Vec::new();
    let mut wpos = 0u32;
    for (idx, ch) in para.chars.iter().enumerate() {
        if let HwpChar::ExtCtrl {
            code, ctrl_index, ..
        } = ch
            && *code == FIELD_START
        {
            let nm = ctrl_index
                .and_then(|i| para.controls.get(i as usize))
                .and_then(|c| field_meta(c).0);
            if nm.as_deref() == Some(name) {
                let value_wpos = wpos + ch.wchar_width();
                let mut e = idx + 1;
                let mut vw = 0u32;
                while e < para.chars.len() {
                    match &para.chars[e] {
                        HwpChar::InlineCtrl { code, .. } if *code == FIELD_END => break,
                        other => vw += other.wchar_width(),
                    }
                    e += 1;
                }
                regions.push((idx + 1, e, value_wpos, vw));
            }
        }
        wpos += ch.wchar_width();
    }

    let count = regions.len();
    let new_wlen = utf16_len(value);
    // 인덱스 무효화 방지: 뒤에서 앞으로 적용.
    for (s, e, vp, old_wlen) in regions.into_iter().rev() {
        para.chars.splice(s..e, build_value_chars(value));
        adjust_runs(&mut para.char_shape_runs, vp, old_wlen, new_wlen);
    }
    if count > 0 {
        para.line_segs.clear();
    }

    let mut nested = 0;
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        nested += set_field_para(p, name, value);
                    }
                }
            }
            Control::Generic(g) => {
                for l in &mut g.paragraph_lists {
                    for p in &mut l.paragraphs {
                        nested += set_field_para(p, name, value);
                    }
                }
            }
            _ => {}
        }
    }
    count + nested
}

fn build_value_chars(value: &str) -> Vec<HwpChar> {
    value
        .chars()
        .map(|c| {
            if c == '\n' {
                HwpChar::CharCtrl(10)
            } else {
                HwpChar::Text(c)
            }
        })
        .collect()
}

/// `{{name}}` 텍스트 자리표시자 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderInfo {
    /// 자리표시자 이름(중괄호 안, 양끝 공백 제거).
    pub name: String,
    /// 문서 전체 등장 횟수.
    pub occurrences: usize,
}

/// 본문(표 셀·글상자 재귀)에서 `{{name}}` 텍스트 자리표시자를 등장 순서로 수집한다.
///
/// 누름틀(form field, [`list_fields`])과 별개 — 순수 텍스트 `{{...}}` 템플릿용.
/// `name`은 `[\w가-힣.-]`(영숫자·한글·`.`·`-`·`_`)만 허용하며, 한 문단 내 연속된
/// 텍스트만 이어 스캔하므로 제어문자/줄나눔을 가로지르는 패턴은 매칭하지 않는다.
/// 채우기는 `set_field`가 아니라 [`replace_text`](crate::replace_text)`("{{name}}", v)` 로 한다.
pub fn scan_placeholders(doc: &Document) -> Vec<PlaceholderInfo> {
    let mut out: Vec<PlaceholderInfo> = Vec::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            collect_placeholders(para, &mut out);
        }
    }
    out
}

fn collect_placeholders(para: &Paragraph, out: &mut Vec<PlaceholderInfo>) {
    let mut seg = String::new();
    for ch in &para.chars {
        match ch {
            HwpChar::Text(c) => seg.push(*c),
            _ => {
                scan_segment(&seg, out);
                seg.clear();
            }
        }
    }
    scan_segment(&seg, out);
    for ctrl in &para.controls {
        for_each_nested(ctrl, &mut |p| collect_placeholders(p, out));
    }
}

fn scan_segment(seg: &str, out: &mut Vec<PlaceholderInfo>) {
    let mut rest = seg;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else { break };
        let name = after[..close].trim();
        if !name.is_empty() && name.chars().all(is_name_char) {
            if let Some(p) = out.iter_mut().find(|p| p.name == name) {
                p.occurrences += 1;
            } else {
                out.push(PlaceholderInfo {
                    name: name.to_string(),
                    occurrences: 1,
                });
            }
        }
        rest = &after[close + 2..];
    }
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '-' | '_')
}

/// 컨트롤의 중첩 문단(표 셀·글상자 리스트)에 f를 적용한다(읽기 전용).
fn for_each_nested(ctrl: &Control, f: &mut impl FnMut(&Paragraph)) {
    match ctrl {
        Control::Table(t) => {
            for cell in &t.cells {
                for p in &cell.paragraphs {
                    f(p);
                }
            }
        }
        Control::Generic(g) => {
            for l in &g.paragraph_lists {
                for p in &l.paragraphs {
                    f(p);
                }
            }
        }
        _ => {}
    }
}

/// ExtCtrl payload(12B): 선두 4B = 역순 ctrl_id(리더가 역순으로 파싱), 나머지 0.
pub fn rev_payload(ctrl_id: &[u8; 4]) -> Vec<u8> {
    let mut p = vec![0u8; 12];
    p[0] = ctrl_id[3];
    p[1] = ctrl_id[2];
    p[2] = ctrl_id[1];
    p[3] = ctrl_id[0];
    p
}

/// FIELD_END(코드 4) payload(12B): 역순 ctrl_id 3바이트(`%` 제외) + 0.
/// 정품 실측(테스트.hwp 한글 제작): %hlk END = `6b 6c 68 00`(="klh\0"). END payload가
/// 전부 0이면 한글이 FIELD_START↔END 짝을 못 지어 필드 미완성 → **클릭 이동 불가**
/// (4차 정답지 대조 확정). START는 `%`까지(4B) 담고 END는 `%`를 뺀 3B만 담는다.
pub fn field_end_payload(ctrl_id: &[u8; 4]) -> Vec<u8> {
    let mut p = vec![0u8; 12];
    p[0] = ctrl_id[3];
    p[1] = ctrl_id[2];
    p[2] = ctrl_id[1];
    // p[3] = 0 ('%' 마커는 END에 담지 않음)
    p
}

/// CTRL_DATA Parameter Set: 필드 이름 BSTR 1개(setid0·count1·id1·BSTR).
pub fn make_field_ctrl_data(name: &str) -> Vec<u8> {
    let mut cd = vec![0u8, 0, 1, 0]; // setid=0, count=1
    cd.extend([1u8, 0, 1, 0]); // item id=1, type=BSTR(1)
    let units: Vec<u16> = name.encode_utf16().collect();
    cd.extend((units.len() as u32).to_le_bytes());
    for u in units {
        cd.extend(u.to_le_bytes());
    }
    cd
}

/// 필드 컨트롤 레코드. `name`이 있으면 CTRL_DATA(누름틀 이름), `command`가 있으면
/// data에 명령(하이퍼링크 URL 등)을 싣는다.
fn make_field_control(ctrl_id: [u8; 4], name: Option<&str>, command: Option<&str>) -> Control {
    let data = match command {
        Some(cmd) => make_field_command_data(&ctrl_id, cmd),
        None => vec![0u8; 11], // 속성4 기타1 len2=0 id4
    };
    let raw_children = match name {
        Some(nm) => vec![OpaqueRecord {
            tag: CTRL_DATA_TAG,
            data: make_field_ctrl_data(nm),
            children: Vec::new(),
        }],
        None => Vec::new(),
    };
    Control::Generic(GenericControl {
        ctrl_id,
        data,
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children,
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
        caption: None,
    })
}

/// 필드 커맨드 레코드 data: 속성(4) 기타(1) len(2) WCHAR[len] **id(4≠0)** trailing(4).
/// 종류별 속성/기타(정품 실측): %hlk=(0x00008800,0) · %fmu=(0,0x08) · 기타=(0,0).
/// **id는 반드시 비영** — 한글은 id=0 필드를 하이퍼링크/필드로 인식하지 않는다(실기 확인:
/// id=0이면 %hlk가 평문 취급, 정품 work_report %hlk는 id=0xd707bf6d). command 해시로
/// 결정론적 비영 id를 부여(같은 명령=같은 id, 서로 다른 URL=다른 id).
pub fn make_field_command_data(ctrl_id: &[u8; 4], command: &str) -> Vec<u8> {
    let (attr, etc): (u32, u8) = match ctrl_id {
        // 정품 한글 %hlk 실측 = 0x0000a800(bit 0x2000 포함). 0x00008800(work_report 복제)만
        // 쓰면 파랑+밑줄은 되나 한글이 클릭 이동을 하지 않는다(2차 실기 확인).
        b"%hlk" => (0x0000_a800, 0),
        b"%fmu" => (0x0000_0000, 0x08),
        _ => (0x0000_0000, 0),
    };
    let mut data = attr.to_le_bytes().to_vec();
    data.push(etc);
    let units: Vec<u16> = command.encode_utf16().collect();
    data.extend((units.len() as u16).to_le_bytes());
    for u in units {
        data.extend(u.to_le_bytes());
    }
    data.extend(field_instance_id(command).to_le_bytes()); // id(4) ≠ 0
    data.extend([0u8; 4]); // trailing(4)=0
    data
}

/// 명령 문자열에서 결정론적 비영 필드 instance id(FNV-1a 32bit, 0이면 1로 보정).
fn field_instance_id(command: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in command.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    if h == 0 { 1 } else { h }
}

/// 인라인 하이퍼링크 조각(markdown 등 스트리밍 삽입용): (FIELD_START 문자, FIELD_END 문자,
/// %hlk 컨트롤). 호출자는 두 문자 사이에 표시 텍스트를 넣고 파랑+밑줄 글자모양을 입힌 뒤
/// `relink_ctrl_index`로 ctrl_index를 잇는다. `create_hyperlink`(앵커 치환)와 달리 파싱 중
/// 위치에 바로 삽입할 때 쓴다.
pub(crate) fn hyperlink_field_parts(url: &str) -> (HwpChar, HwpChar, Control) {
    let ctrl_id = *b"%hlk";
    (
        HwpChar::ExtCtrl {
            code: FIELD_START,
            ctrl_id,
            payload: rev_payload(&ctrl_id),
            ctrl_index: None,
        },
        HwpChar::InlineCtrl {
            code: FIELD_END,
            payload: field_end_payload(&ctrl_id),
        },
        make_field_control(ctrl_id, None, Some(&hlk_command(url))),
    )
}

/// 하이퍼링크 필드 커맨드: `{URL};1;0;0;` — URL 특수문자를 정품 규칙으로 백슬래시 이스케이프.
/// v1은 `\ ; :`만 이스케이프(정품 `http\://…` 확인). 복잡한 URL은 근사.
fn hlk_command(url: &str) -> String {
    let mut esc = String::with_capacity(url.len() + 8);
    for c in url.chars() {
        match c {
            '\\' => esc.push_str("\\\\"),
            ';' => esc.push_str("\\;"),
            ':' => esc.push_str("\\:"),
            _ => esc.push(c),
        }
    }
    format!("{esc};1;0;0;")
}

/// 필드 문자열: FIELD_START(ExtCtrl, 코드 3, ctrl_id) + 표시값 + FIELD_END(InlineCtrl, 코드 4).
fn make_field_chars(ctrl_id: [u8; 4], value: &str) -> Vec<HwpChar> {
    let mut chars = vec![HwpChar::ExtCtrl {
        code: FIELD_START,
        ctrl_id,
        payload: rev_payload(&ctrl_id),
        ctrl_index: None,
    }];
    chars.extend(build_value_chars(value));
    chars.push(HwpChar::InlineCtrl {
        code: FIELD_END,
        payload: field_end_payload(&ctrl_id),
    });
    chars
}

/// 문단의 ExtCtrl ↔ controls 등장순서 연결(ctrl_index)을 다시 매긴다.
pub fn relink_ctrl_index(para: &mut Paragraph) {
    let mut next = 0u32;
    for ch in &mut para.chars {
        if let HwpChar::ExtCtrl { ctrl_index, .. } = ch {
            *ctrl_index = Some(next);
            next += 1;
        }
    }
}

/// 하이퍼링크 표시 텍스트용 글자모양(파랑 + 밑줄)을 header에 확보하고 id를 돌려준다.
/// 정품 하이퍼링크(work_report "설치하기")는 별도 charPr(파랑+밑줄)를 쓴다 — 이 글자
/// 모양이 없으면 한글이 링크로 인식/표시하지 않는다(실기 확인). 이미 있으면 재사용.
fn hyperlink_char_shape(doc: &mut Document) -> CharShapeId {
    const BLUE: u32 = 0x00FF_0000; // COLORREF 0x00BBGGRR = RGB(0,0,255)
    if let Some(i) = doc
        .header
        .char_shapes
        .iter()
        .position(|c| c.text_color == BLUE && c.underline_kind() == 1)
    {
        return CharShapeId(i as u16);
    }
    let mut cs = doc.header.char_shapes.first().cloned().unwrap_or_default();
    cs.text_color = BLUE;
    cs.underline_color = BLUE;
    cs.attr = (cs.attr & !(0x3 << 2)) | (1 << 2); // 밑줄 종류 1(글자 아래)
    if cs.shade_color == 0 {
        cs.shade_color = 0xFFFF_FFFF; // 검정 음영 방지
    }
    let id = doc.header.char_shapes.len() as u16;
    doc.header.char_shapes.push(cs);
    CharShapeId(id)
}

/// char_shape_runs의 [start, end) WCHAR 구간을 `shape`로 지정하고 end에서 이전 모양을 복원.
fn apply_run_style(runs: &mut Vec<(u32, CharShapeId)>, start: u32, end: u32, shape: CharShapeId) {
    if end <= start {
        return;
    }
    let after = runs
        .iter()
        .rfind(|(p, _)| *p <= end)
        .map(|(_, id)| *id)
        .unwrap_or_default();
    runs.retain(|(p, _)| *p < start || *p > end);
    runs.push((start, shape));
    runs.push((end, after));
    runs.sort_by_key(|(p, _)| *p);
    let mut out: Vec<(u32, CharShapeId)> = Vec::with_capacity(runs.len());
    for &(p, id) in runs.iter() {
        match out.last() {
            Some(&(_, lid)) if lid == id => {} // 같은 모양 연속 — 잉여 경계 제거
            _ => out.push((p, id)),
        }
    }
    if out.first().map(|(p, _)| *p) != Some(0) {
        out.insert(0, (0, CharShapeId::default()));
    }
    *runs = out;
}

/// 한 문단에서 앵커 텍스트 뒤에 필드(누름틀/하이퍼링크 등)를 삽입한다. 반환=삽입 여부.
/// `value_shape`가 있으면 표시 텍스트 구간에 그 글자모양(하이퍼링크 파랑+밑줄)을 적용한다.
fn create_field_in_para(
    para: &mut Paragraph,
    anchor: &str,
    ctrl_id: [u8; 4],
    name: Option<&str>,
    command: Option<&str>,
    value: &str,
    value_shape: Option<CharShapeId>,
) -> bool {
    let Some((cidx, wpos)) = find_match(&para.chars, anchor, 0) else {
        return false;
    };
    let ins = (cidx + anchor.chars().count()).min(para.chars.len());
    let iw = wpos + utf16_len(anchor);
    // control 삽입 위치 = ins 이전 ExtCtrl 개수(등장순서가 chars와 정합해야 함).
    let ci = para.chars[..ins]
        .iter()
        .filter(|c| matches!(c, HwpChar::ExtCtrl { .. }))
        .count()
        .min(para.controls.len());
    para.controls
        .insert(ci, make_field_control(ctrl_id, name, command));
    let field_chars = make_field_chars(ctrl_id, value);
    let inserted_w: u32 = field_chars.iter().map(HwpChar::wchar_width).sum();
    para.chars.splice(ins..ins, field_chars);
    adjust_runs(&mut para.char_shape_runs, iw, 0, inserted_w);
    // 표시 텍스트 구간 = [iw + FIELD_START(8), + display] 에 하이퍼링크 글자모양 적용.
    if let Some(shape) = value_shape {
        let dstart = iw + 8;
        apply_run_style(
            &mut para.char_shape_runs,
            dstart,
            dstart + utf16_len(value),
            shape,
        );
    }
    relink_ctrl_index(para);
    para.header.ctrl_mask = 0; // writer가 chars에서 재계산(FIELD_START bit3 포함)
    para.line_segs.clear();
    true
}

/// 본문/표 셀/글상자 문단을 재귀로 훑어 첫 매칭에 필드를 삽입한다.
fn create_field_rec(
    para: &mut Paragraph,
    anchor: &str,
    ctrl_id: [u8; 4],
    name: Option<&str>,
    command: Option<&str>,
    value: &str,
    value_shape: Option<CharShapeId>,
) -> bool {
    if create_field_in_para(para, anchor, ctrl_id, name, command, value, value_shape) {
        return true;
    }
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        if create_field_rec(p, anchor, ctrl_id, name, command, value, value_shape) {
                            return true;
                        }
                    }
                }
            }
            Control::Generic(g) => {
                for l in &mut g.paragraph_lists {
                    for p in &mut l.paragraphs {
                        if create_field_rec(p, anchor, ctrl_id, name, command, value, value_shape) {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// 문서를 훑어 첫 앵커 매칭에 필드를 삽입한다(본문·표 셀·글상자 재귀).
fn create_field_generic(
    doc: &mut Document,
    anchor: &str,
    ctrl_id: [u8; 4],
    name: Option<&str>,
    command: Option<&str>,
    value: &str,
    value_shape: Option<CharShapeId>,
) -> bool {
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            if create_field_rec(para, anchor, ctrl_id, name, command, value, value_shape) {
                return true;
            }
        }
    }
    false
}

/// `anchor` 텍스트를 가진 첫 문단의 그 뒤에 `name` 이름의 %clk 누름틀을 삽입한다
/// (표시값 `value`, 보통 빈 문자열). 반환=삽입 여부. 삽입 후 `set_field`로 채울 수 있다.
pub fn create_field(doc: &mut Document, anchor: &str, name: &str, value: &str) -> bool {
    create_field_generic(doc, anchor, *b"%clk", Some(name), None, value, None)
}

/// `anchor` 텍스트 뒤에 하이퍼링크(%hlk)를 삽입한다. `display`=클릭 표시 텍스트, `url`=대상.
/// 반환=삽입 여부. hwp5·hwpx 양쪽에 동일하게 방출된다. 표시 텍스트에 하이퍼링크 글자모양
/// (파랑+밑줄)을 적용해 한글이 링크로 인식/표시하게 한다(실기 확인 — 미적용 시 평문 취급).
pub fn create_hyperlink(doc: &mut Document, anchor: &str, url: &str, display: &str) -> bool {
    let cmd = hlk_command(url);
    let shape = hyperlink_char_shape(doc);
    create_field_generic(
        doc,
        anchor,
        *b"%hlk",
        None,
        Some(&cmd),
        display,
        Some(shape),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::opaque::OpaqueRecord;
    use hwp_model::{CharShapeId, GenericControl};

    fn wbytes(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn parse_command_계산식() {
        // 속성(4) 기타(1) len(2) WCHAR[len] id(4)
        let cmd = "SUM(A1:A3)";
        let mut data = vec![0u8; 5];
        data.extend((cmd.encode_utf16().count() as u16).to_le_bytes());
        data.extend(wbytes(cmd));
        data.extend([0u8; 4]);
        assert_eq!(parse_command(&data).as_deref(), Some("SUM(A1:A3)"));
    }

    #[test]
    fn first_bstr_파라미터셋() {
        // SetID(2) count(2)=2; item1: id(2) type(2)=3(I2) + 2바이트; item2: id(2) type(2)=1(BSTR)
        let name = "수신";
        let mut data = vec![0, 0, 2, 0]; // setid=0, count=2
        data.extend([1, 0, 3, 0, 0xAA, 0xBB]); // item id=1 type=I2(3) value 2B
        data.extend([2, 0, 1, 0]); // item id=2 type=BSTR(1)
        data.extend((name.encode_utf16().count() as u32).to_le_bytes());
        data.extend(wbytes(name));
        assert_eq!(first_bstr(&data).as_deref(), Some("수신"));
    }

    /// 이름 있는 누름틀 필드 하나를 가진 합성 문단.
    fn field_para(name: &str, value: &str) -> Paragraph {
        // CTRL_DATA: Parameter Set(setid=0, count=1) + item(id=1, type=BSTR) + len + WCHAR[]
        let mut cd = vec![0, 0, 1, 0];
        cd.extend([1, 0, 1, 0]); // item id=1, type=BSTR(1)
        cd.extend((name.encode_utf16().count() as u32).to_le_bytes());
        cd.extend(wbytes(name));
        let ctrl = Control::Generic(GenericControl {
            ctrl_id: *b"%clk",
            data: vec![0u8; 11], // 속성4 기타1 len2=0 id4
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: vec![OpaqueRecord {
                tag: CTRL_DATA_TAG,
                data: cd,
                children: Vec::new(),
            }],
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
        });
        let mut chars = vec![HwpChar::ExtCtrl {
            code: FIELD_START,
            ctrl_id: *b"%clk",
            payload: vec![0u8; 12],
            ctrl_index: Some(0),
        }];
        chars.extend(value.chars().map(HwpChar::Text));
        chars.push(HwpChar::InlineCtrl {
            code: FIELD_END,
            payload: vec![0u8; 12],
        });
        chars.push(HwpChar::CharCtrl(13));
        Paragraph {
            chars,
            char_shape_runs: vec![(0, CharShapeId(0))],
            controls: vec![ctrl],
            ..Default::default()
        }
    }

    #[test]
    fn list_fields_이름_종류_값() {
        let mut doc = Document::default();
        doc.sections.push(hwp_model::Section {
            paragraphs: vec![field_para("수신", "내부결재")],
            extras: Vec::new(),
        });
        let fields = list_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].kind, "누름틀");
        assert_eq!(fields[0].name.as_deref(), Some("수신"));
        assert_eq!(fields[0].value, "내부결재");
    }

    #[test]
    fn scan_placeholders_이름_횟수() {
        let doc = crate::from_markdown::from_markdown("{{기관명}} 안녕 {{제목}} 끝 {{기관명}}\n");
        let slots = scan_placeholders(&doc);
        let map: std::collections::HashMap<_, _> = slots
            .iter()
            .map(|p| (p.name.as_str(), p.occurrences))
            .collect();
        assert_eq!(map.get("기관명"), Some(&2));
        assert_eq!(map.get("제목"), Some(&1));
        assert_eq!(slots.len(), 2);
    }

    #[test]
    fn set_field_값_교체() {
        let mut doc = Document::default();
        doc.sections.push(hwp_model::Section {
            paragraphs: vec![field_para("수신", "")], // 빈 누름틀
            extras: Vec::new(),
        });
        let n = set_field(&mut doc, "수신", "기획팀");
        assert_eq!(n, 1);
        let fields = list_fields(&doc);
        assert_eq!(fields[0].value, "기획팀");
        // 다른 이름은 안 바뀜
        assert_eq!(set_field(&mut doc, "없는이름", "x"), 0);
    }

    #[test]
    fn create_field_삽입_후_읽기_채우기() {
        let mut doc = crate::from_markdown::from_markdown("수신: 부서");
        // 앵커 "수신:" 뒤에 누름틀 삽입.
        assert!(create_field(&mut doc, "수신:", "수신처", ""));
        let fields = list_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].kind, "누름틀");
        assert_eq!(fields[0].ctrl_id, "%clk");
        assert_eq!(fields[0].name.as_deref(), Some("수신처"));
        assert_eq!(fields[0].value, "");
        // 생성한 누름틀을 이름으로 채우기.
        assert_eq!(set_field(&mut doc, "수신처", "홍길동"), 1);
        assert_eq!(list_fields(&doc)[0].value, "홍길동");
        // 없는 앵커는 삽입 안 됨.
        assert!(!create_field(&mut doc, "없는앵커", "x", ""));
    }

    #[test]
    fn create_field_ctrl_index_정합() {
        // 삽입된 ExtCtrl이 controls[ctrl_index]와 등장순서로 맞물리는지.
        let mut doc = crate::from_markdown::from_markdown("가나다");
        assert!(create_field(&mut doc, "가", "필드", "값"));
        let para = &doc.sections[0].paragraphs[0];
        // ExtCtrl의 ctrl_index가 %clk 컨트롤을 가리킨다.
        let ext = para
            .chars
            .iter()
            .find_map(|c| match c {
                HwpChar::ExtCtrl {
                    code, ctrl_index, ..
                } if *code == FIELD_START => *ctrl_index,
                _ => None,
            })
            .expect("ExtCtrl 존재");
        match &para.controls[ext as usize] {
            Control::Generic(g) => assert_eq!(&g.ctrl_id, b"%clk"),
            _ => panic!("필드 컨트롤이 %clk Generic이어야"),
        }
        // 필드 ExtCtrl payload 선두 4B = 역순 ctrl_id("%clk"→"klc%").
        // (문단 선두 secd ExtCtrl과 구분하려 code로 필터.)
        let payload = para.chars.iter().find_map(|c| match c {
            HwpChar::ExtCtrl { code, payload, .. } if *code == FIELD_START => Some(payload.clone()),
            _ => None,
        });
        assert_eq!(&payload.unwrap()[..4], b"klc%");
    }

    #[test]
    fn hlk_command_정품_포맷() {
        // work_report.hwp 정품 %hlk와 동일: URL의 `:`→`\:`, 뒤에 `;1;0;0;`.
        assert_eq!(
            hlk_command("http://hangeul.naver.com/font"),
            "http\\://hangeul.naver.com/font;1;0;0;"
        );
    }

    #[test]
    fn make_field_command_data_바이트() {
        // 정품 %hlk data 구조: attr=0x00008800·etc=0·len(2)·WCHAR·id(4≠0)·trailing(4).
        let cmd = "http\\://hangeul.naver.com/font;1;0;0;";
        let data = make_field_command_data(b"%hlk", cmd);
        assert_eq!(&data[0..5], &[0x00, 0xa8, 0x00, 0x00, 0x00]); // attr=0x0000a800+etc
        let len = u16::from_le_bytes([data[5], data[6]]) as usize;
        assert_eq!(len, cmd.encode_utf16().count());
        assert_eq!(len, 37); // 정품 work_report %hlk와 동일 길이
        // parse_command이 커맨드를 그대로 복원한다.
        assert_eq!(parse_command(&data).as_deref(), Some(cmd));
        // ★id(4)는 비영이어야 한글이 하이퍼링크로 인식한다(실기 확인).
        let id = u32::from_le_bytes([
            data[7 + len * 2],
            data[8 + len * 2],
            data[9 + len * 2],
            data[10 + len * 2],
        ]);
        assert_ne!(id, 0, "필드 id는 비영");
        // trailing(4)=0.
        assert_eq!(&data[11 + len * 2..], &[0u8; 4]);
    }

    #[test]
    fn make_field_command_data_종류별_속성() {
        // %fmu는 정품 실측 attr=0·etc=0x08, %hlk는 attr=0x00008800.
        let fmu = make_field_command_data(b"%fmu", "=SUM(A1:A2)");
        assert_eq!(&fmu[0..5], &[0x00, 0x00, 0x00, 0x00, 0x08]);
        let hlk = make_field_command_data(b"%hlk", "x;1;0;0;");
        assert_eq!(&hlk[0..5], &[0x00, 0xa8, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn create_hyperlink_삽입_후_읽기() {
        let mut doc = crate::from_markdown::from_markdown("자세히: 여기");
        assert!(create_hyperlink(
            &mut doc,
            "자세히:",
            "https://example.com/path",
            "링크"
        ));
        let fields = list_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].kind, "하이퍼링크");
        assert_eq!(fields[0].ctrl_id, "%hlk");
        assert_eq!(fields[0].name, None);
        assert_eq!(fields[0].value, "링크");
        assert_eq!(
            fields[0].command.as_deref(),
            Some("https\\://example.com/path;1;0;0;")
        );
    }

    #[test]
    fn hyperlink_url_커맨드에서_복원() {
        // %hlk 컨트롤의 커맨드(`{URL};1;0;0;`)에서 URL을 이스케이프 해제해 복원한다.
        let ctrl = make_field_control(*b"%hlk", None, Some(&hlk_command("http://x.io/a")));
        assert_eq!(hyperlink_url(&ctrl).as_deref(), Some("http://x.io/a"));
        // %clk 등 비하이퍼링크는 None.
        let clk = make_field_control(*b"%clk", Some("이름"), None);
        assert_eq!(hyperlink_url(&clk), None);
    }

    #[test]
    fn create_hyperlink_표시텍스트_파랑밑줄() {
        // 정품 하이퍼링크처럼 표시 텍스트에 파랑+밑줄 글자모양이 입혀져야 한다
        // (없으면 한글이 평문 취급 — 실기 확인).
        let mut doc = crate::from_markdown::from_markdown("여기: 자리");
        assert!(create_hyperlink(
            &mut doc,
            "여기:",
            "https://x.io",
            "링크텍스트"
        ));
        // 파랑+밑줄 글자모양이 header에 추가됐다.
        let hlk = doc
            .header
            .char_shapes
            .iter()
            .position(|c| c.text_color == 0x00FF_0000 && c.underline_kind() == 1)
            .expect("하이퍼링크 글자모양");
        let para = &doc.sections[0].paragraphs[0];
        // 표시 텍스트 구간에 하이퍼링크 글자모양 run이 존재한다(위치는 선두 컨트롤에
        // 따라 달라지므로 값으로 확인). 표시 텍스트 5 WCHAR("링크텍스트") 폭.
        let hlk_run = para
            .char_shape_runs
            .iter()
            .find(|(_, id)| id.0 == hlk as u16)
            .map(|(p, _)| *p)
            .expect("하이퍼링크 글자모양 run");
        // 링크 구간 뒤(원래 모양)로 복원하는 경계가 있어야 한다.
        assert!(
            para.char_shape_runs
                .iter()
                .any(|(p, id)| *p > hlk_run && id.0 != hlk as u16),
            "링크 구간 뒤 원래 글자모양 복원: {:?}",
            para.char_shape_runs
        );
    }

    #[test]
    fn field_end_payload_ctrl_id_담김() {
        // 정품(테스트.hwp 한글 제작) FIELD_END = 역순 ctrl_id 3B(`%` 제외) + 0.
        // %hlk END = 6b 6c 68 00…; 전부 0이면 한글이 필드 짝을 못 지어 클릭 불가.
        assert_eq!(
            field_end_payload(b"%hlk"),
            vec![0x6b, 0x6c, 0x68, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        // 생성한 하이퍼링크의 FIELD_END(InlineCtrl 코드4) payload가 비영(ctrl_id 담김).
        let mut doc = crate::from_markdown::from_markdown("여기: 자리");
        assert!(create_hyperlink(&mut doc, "여기:", "https://x.io", "링크"));
        let end = doc.sections[0].paragraphs[0]
            .chars
            .iter()
            .find_map(|c| match c {
                HwpChar::InlineCtrl { code, payload } if *code == FIELD_END => {
                    Some(payload.clone())
                }
                _ => None,
            })
            .expect("FIELD_END");
        assert_eq!(&end[..3], &[0x6b, 0x6c, 0x68], "END payload에 역순 ctrl_id");
    }

    #[test]
    fn create_field_누름틀은_글자모양_불변() {
        // %clk 누름틀은 하이퍼링크 글자모양을 추가하지 않는다(파랑 미적용).
        let mut doc = crate::from_markdown::from_markdown("수신: 부서");
        let before = doc.header.char_shapes.len();
        assert!(create_field(&mut doc, "수신:", "수신처", ""));
        assert_eq!(doc.header.char_shapes.len(), before, "글자모양 추가 없음");
    }
}
