//! IR → GFM markdown.
//!
//! 매핑 규칙:
//! - "개요 N" 스타일 문단 → `#` × N 헤딩
//! - 문자 모양 굵게/기울임 → `**`/`*` 스팬, 취소선 → `~~`, 밑줄·위/아래첨자 →
//!   `<u>`·`<sup>`·`<sub>` (char_shape_runs 기반)
//! - 하이퍼링크(%hlk 필드) → `[표시텍스트](URL)`
//! - 이미지(Picture) → `![image]()` (또는 media_dir 지정 시 추출·상대참조)
//! - 표 → GFM 표 (첫 행을 헤더로). 병합 셀(col_span/row_span>1)이나 셀 안 중첩 표가
//!   있으면 HTML `<table>`(colspan/rowspan + 인라인 HTML 태그)로 폴백 — 내용 보존 우선
//! - 글머리표/번호 문단 → `- `/`N. ` 목록 (번호는 numbering_levels 형식 합성;
//!   아라비아 숫자 외 형식은 `- 가. ` 식 리터럴 마커로 보존)
//! - 각주/미주 → 본문 `[^N]`/`[^eN]` 마커 + 문서 끝 정의 (GFM 풋노트)
//! - 수식(eqed) → 인라인 `$스크립트$`, 블록 `$$스크립트$$` (HWP 수식 스크립트 원문)
//! - 줄나눔(10) → 강제 줄바꿈, 탭 → 공백

use std::fs::OpenOptions;
use std::io::{Error, ErrorKind, Write};
use std::path::{Path, PathBuf};

use hwp_model::list::ListState;
use hwp_model::{
    Cell, CharShape, Control, Document, Equation, GenericControl, HwpChar, Paragraph, Table,
    TextOptions, ctrl_char,
};

/// markdown 출력 옵션.
#[derive(Default)]
pub struct MarkdownOptions<'a> {
    /// 이미지 바이너리를 추출할 디렉터리. `Some`이면 이미지를 `image1.png` 식으로
    /// 그 디렉터리에 뽑고 `![image](접두사/image1.png)`로 참조한다(디렉터리는
    /// 첫 이미지에서 지연 생성 — 이미지가 없으면 만들지 않는다). `None`이면 기존처럼
    /// 빈 참조 `![image]()`를 유지한다(동작 불변).
    pub media_dir: Option<&'a Path>,
    /// 이미지 참조 경로 접두사. `None`이면 `media_dir`의 디렉터리명을 쓴다(기존 동작).
    /// CLI `--media-dir figs`처럼 사용자가 준 상대경로를 링크에 그대로 쓸 때 지정한다.
    pub media_prefix: Option<&'a str>,
    /// 텍스트 추출 옵션(머리말/꼬리말·숨은 설명 포함 여부). 기본은 제외.
    pub text: TextOptions,
}

/// markdown 출력의 문자 범위 `[start, end)`가 유래한 원본 IR 좌표.
///
/// 오프셋은 바이트가 아니라 **유니코드 스칼라(문자)** 단위다 — Python `str` 인덱싱과
/// 동일하므로 소비자가 `md[start:end]`로 그대로 슬라이스할 수 있다. 좌표는 IR
/// (`--format json`)의 `sections[]`/`paragraphs[]` 인덱스라 재디코드에도 안정적이다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownSegment {
    /// `doc.sections[]` 인덱스.
    pub section: usize,
    /// `sections[section].paragraphs[]` 인덱스 (최상위 문단).
    pub para: usize,
    /// 문자 오프셋(포함).
    pub start: usize,
    /// 문자 오프셋(제외).
    pub end: usize,
}

/// IR 전체를 GFM markdown으로 직렬화한다(기존 시그니처 유지 — 이미지 미추출).
pub fn to_markdown(doc: &Document) -> String {
    // media_dir 미지정 → IO가 없어 실패할 수 없다.
    to_markdown_with(doc, &MarkdownOptions::default())
        .expect("media_dir 미지정 시 IO가 없어 실패할 수 없다")
}

/// 옵션을 받는 변형. `media_dir` 지정 시 이미지를 추출하며, 추출 IO 실패는 `Err`.
///
/// 세그먼트 맵을 버리는 래퍼다 — 방출 코어([`emit_markdown`])를
/// [`to_markdown_with_segments`]와 공유하므로 markdown 문자열은 세그먼트 유무와
/// 무관하게 바이트 단위로 동일함이 구조적으로 보장된다.
pub fn to_markdown_with(doc: &Document, opts: &MarkdownOptions) -> std::io::Result<String> {
    Ok(emit_markdown(doc, opts)?.0)
}

/// [`to_markdown_with`]와 같은 markdown을 내면서, 각 출력 문자 범위가 어느 원본 문단에서
/// 왔는지를 [`MarkdownSegment`] 목록으로 함께 돌려준다.
///
/// 세그먼트는 `start` 오름차순·비중첩이며, 미귀속 출력(빈 줄·구역 구분 등)에 해당하는
/// 간극은 허용한다. 표가 만든 줄은 표를 담은 문단 인덱스를 상속하고, 각주/미주 정의는
/// 참조 문단에 귀속된다. 머리말/꼬리말은 기본 제외(`opts.text`로 포함).
pub fn to_markdown_with_segments(
    doc: &Document,
    opts: &MarkdownOptions,
) -> std::io::Result<(String, Vec<MarkdownSegment>)> {
    emit_markdown(doc, opts)
}

/// markdown 방출 코어 — 문자열과 세그먼트 맵을 함께 만든다.
///
/// 계측(원시 세그먼트 `Vec` push 몇 번)은 오버헤드가 무시할 수준이라 항상 켠다.
/// 방출 순서: 섹션/문단 이중 루프에서 문단마다 방출 전후의 `out.len()`을 기록하고,
/// 문서 끝 각주/미주 정의는 수집 시점의 문단 좌표에 귀속시킨다. 이후 [`cleanup_with_map`]으로
/// 정리하며 삭제된 바이트만큼 오프셋을 재매핑하고, 마지막에 바이트→문자로 변환한다.
fn emit_markdown(
    doc: &Document,
    opts: &MarkdownOptions,
) -> std::io::Result<(String, Vec<MarkdownSegment>)> {
    let mut ctx = Ctx {
        media_dir: opts.media_dir,
        dir_name: opts
            .media_prefix
            .map(|p| p.replace('\\', "/").trim_end_matches('/').to_string())
            .or_else(|| {
                opts.media_dir
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_default(),
        img_no: 0,
        pending_media: Vec::new(),
        include_header_footer: opts.text.include_header_footer,
        include_hidden: opts.text.include_hidden,
        html_mode: false,
        last_was_list: false,
        list_widths: Vec::new(),
        list_keys: Vec::new(),
        notes: Vec::new(),
        foot_n: 0,
        end_n: 0,
        cur_section: 0,
        cur_para: 0,
    };

    let mut out = String::new();
    // 원시 세그먼트: (섹션, 문단, byte_start, byte_end) — 방출 순서(= start 오름차순).
    let mut raw: Vec<(usize, usize, usize, usize)> = Vec::new();

    for (section_index, section) in doc.sections.iter().enumerate() {
        if section_index > 0 {
            break_section_list(&mut ctx, &mut out);
        }
        // 목록 번호 카운터는 구역 단위로 리셋한다(렌더러와 같은 규칙).
        let mut list_state = ListState::default();
        for (para_index, para) in section.paragraphs.iter().enumerate() {
            // 각주/미주가 수집될 때 귀속시킬 현재 문단 좌표를 갱신한다.
            ctx.cur_section = section_index;
            ctx.cur_para = para_index;
            let start = out.len();
            render_paragraph(doc, para, &mut list_state, &mut ctx, &mut out);
            if out.len() > start {
                raw.push((section_index, para_index, start, out.len()));
            }
        }
    }
    // 각주/미주 정의는 문서 끝에 모은다. 수집 순서(문서 순서)대로 방출되므로 각 정의의
    // byte 범위는 자연히 본문 세그먼트 뒤에 오름차순으로 쌓인다.
    if !ctx.notes.is_empty() {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        for note in &ctx.notes {
            let start = out.len();
            emit_note(note, &mut out);
            if out.len() > start {
                raw.push((note.src.0, note.src.1, start, out.len()));
            }
        }
    }
    ctx.persist_media()?;

    // cleanup으로 정리하며 삭제된 바이트만큼 세그먼트 오프셋을 재매핑하고 문자 단위로 변환.
    let (cleaned, deletions) = cleanup_with_map(&out);
    let segments = build_segments(&cleaned, &deletions, &raw);
    Ok((cleaned, segments))
}

/// Emits one footnote/endnote definition at the document end in GFM footnote syntax.
/// Definitions are always `[^N]:` even for notes referenced inside HTML tables — the
/// importer (from_markdown) parses them via ENABLE_FOOTNOTES and reattaches them to
/// fnref markers (#47).
fn emit_note(note: &Note, out: &mut String) {
    let mut lines = note.text.lines();
    match lines.next() {
        Some(first) => {
            out.push_str(&format!("[^{}]: {first}\n", note.label));
            // Continuation lines get 4-space indentation (GFM footnote continuation rule).
            for l in lines {
                out.push_str(&format!("    {l}\n"));
            }
        }
        None => out.push_str(&format!("[^{}]:\n", note.label)),
    }
}

struct PendingMedia {
    path: PathBuf,
    data: Vec<u8>,
}

struct Note {
    label: String,
    text: String,
    /// 이 각주/미주를 참조한 최상위 문단 좌표 (섹션 인덱스, 문단 인덱스).
    src: (usize, usize),
}

/// 렌더 중 상태(이미지 추출 진행·텍스트 포함 정책·목록/각주·HTML 표 모드).
struct Ctx<'a> {
    media_dir: Option<&'a Path>,
    /// 참조 경로 접두사(media_prefix 또는 디렉터리명).
    dir_name: String,
    /// 다음 이미지 번호(1-기반 카운터).
    img_no: usize,
    /// 렌더 완료 뒤 충돌 검사를 거쳐 기록할 이미지.
    pending_media: Vec<PendingMedia>,
    include_header_footer: bool,
    include_hidden: bool,
    /// HTML 표 안 — 블록 HTML에선 md가 렌더되지 않으므로 마크·링크·이미지를 HTML 태그로 방출.
    html_mode: bool,
    /// 직전 출력이 목록 항목 — 목록 블록 종료 시 빈 줄 확보용.
    last_was_list: bool,
    /// 현재 활성 목록 수준별 콘텐츠 열 너비와 (머리 종류, 정의 ID).
    list_widths: Vec<usize>,
    list_keys: Vec<(u8, u16)>,
    /// 각주/미주 — 문서 끝 정의용.
    notes: Vec<Note>,
    foot_n: u32,
    end_n: u32,
    /// 세그먼트 귀속용 — 현재 방출 중인 최상위 문단 좌표(메인 루프에서 문단마다 갱신).
    cur_section: usize,
    cur_para: usize,
}

impl Ctx<'_> {
    /// Picture 바이트를 기록 대기열에 넣고 markdown 이미지 참조를 만든다.
    fn image_ref(&mut self, data: &[u8]) -> String {
        let html = self.html_mode;
        let fallback = || {
            if html {
                "<!-- image -->".to_string()
            } else {
                "![image]()".to_string()
            }
        };
        let Some(dir) = self.media_dir else {
            return fallback();
        };
        self.img_no += 1;
        let (ext, _) = crate::image::image_kind(data);
        let file = format!("image{}.{ext}", self.img_no);
        self.pending_media.push(PendingMedia {
            path: dir.join(&file),
            data: data.to_vec(),
        });
        let reference = if self.dir_name.is_empty() {
            file
        } else {
            format!("{}/{}", self.dir_name, file)
        };
        if html {
            format!(
                "<img src=\"{}\" alt=\"image\">",
                escape_html_attr(&reference)
            )
        } else {
            format!("![image]({})", md_link_dest(&reference))
        }
    }

    /// 기존 파일은 동일 바이트일 때만 재사용한다. 충돌을 모두 선확인한 뒤 새 파일을
    /// create_new로 기록하고, 도중 실패하면 이번 호출에서 만든 파일만 제거한다.
    fn persist_media(&self) -> std::io::Result<()> {
        if self.pending_media.is_empty() {
            return Ok(());
        }
        for item in &self.pending_media {
            match std::fs::read(&item.path) {
                Ok(existing) if existing == item.data => {}
                Ok(_) => return Err(media_collision(&item.path)),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }

        let dir = self.media_dir.expect("대기 이미지가 있으면 media_dir 존재");
        std::fs::create_dir_all(dir)?;
        let mut created = Vec::new();
        for item in &self.pending_media {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&item.path)
            {
                Ok(mut file) => {
                    created.push(item.path.clone());
                    if let Err(e) = file.write_all(&item.data) {
                        rollback_media(&created);
                        return Err(e);
                    }
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => match std::fs::read(&item.path) {
                    Ok(existing) if existing == item.data => {}
                    Ok(_) => {
                        rollback_media(&created);
                        return Err(media_collision(&item.path));
                    }
                    Err(read_error) => {
                        rollback_media(&created);
                        return Err(read_error);
                    }
                },
                Err(e) => {
                    rollback_media(&created);
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}

fn media_collision(path: &Path) -> Error {
    Error::new(
        ErrorKind::AlreadyExists,
        format!("기존 미디어 파일을 덮어쓸 수 없습니다: {}", path.display()),
    )
}

fn rollback_media(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

/// 과도한 빈 줄을 정리하고(연속 빈 줄을 1줄로), 정리된 문자열과 **삭제 구간 목록**(원본
/// `out` 바이트 기준 `(start, len)`, start 오름차순·비중첩)을 함께 낸다.
///
/// cleanup은 연속 빈 줄을 **통째 줄 단위로만** 삭제하므로 각 삭제는 원본의 연속 바이트
/// 구간이다. 마지막 줄에 개행이 없으면 cleanup이 끝에 `\n`을 붙이는데(순수 삽입) 앞쪽
/// 오프셋에는 영향이 없어 매핑에서 무시한다. 생성 markdown에는 `\r`이 없으므로
/// `str::lines()`와 동일하게 `\n` 기준으로만 분할한다.
fn cleanup_with_map(out: &str) -> (String, Vec<(usize, usize)>) {
    let mut cleaned = String::with_capacity(out.len());
    let mut deletions: Vec<(usize, usize)> = Vec::new();
    let mut blank_run = 0usize;
    let bytes = out.as_bytes();
    let mut i = 0usize;
    while i < out.len() {
        // 현재 줄의 내용 범위 [i, line_end)와 개행 포함 다음 줄 시작 next.
        let (line_end, next) = match bytes[i..].iter().position(|&b| b == b'\n') {
            Some(p) => (i + p, i + p + 1),  // '\n' 포함해 다음 줄로
            None => (out.len(), out.len()), // 마지막 줄(개행 없음)
        };
        let line = &out[i..line_end];
        let keep = if line.trim().is_empty() {
            blank_run += 1;
            blank_run <= 1
        } else {
            blank_run = 0;
            true
        };
        if keep {
            cleaned.push_str(line);
            cleaned.push('\n');
        } else {
            // 이 줄 전체(내용 + 개행)를 삭제 — 원본 [i, next) 구간.
            deletions.push((i, next - i));
        }
        i = next;
    }
    (cleaned, deletions)
}

/// 삭제 구간 목록을 원본→정리본 오프셋 재매핑 함수로 감싼다(이진 탐색, O(log D)).
struct DeletionMap {
    /// (삭제 시작, 삭제 끝(제외), 이 구간 이전까지의 누적 삭제 바이트).
    entries: Vec<(usize, usize, usize)>,
}

impl DeletionMap {
    fn new(deletions: &[(usize, usize)]) -> Self {
        let mut entries = Vec::with_capacity(deletions.len());
        let mut cum = 0usize;
        for &(start, len) in deletions {
            entries.push((start, start + len, cum));
            cum += len;
        }
        Self { entries }
    }

    /// 원본 바이트 오프셋 `old`의 정리본 바이트 오프셋. 삭제 구간 내부의 오프셋은 그 구간
    /// 시작으로 붕괴한다(삭제된 줄 내부는 자연히 붕괴 지점으로 수렴).
    fn remap(&self, old: usize) -> usize {
        // start < old 인 마지막 구간만이 old에 영향을 줄 수 있다(정렬·비중첩).
        let idx = self.entries.partition_point(|&(start, _, _)| start < old);
        if idx == 0 {
            return old;
        }
        let (start, end, before) = self.entries[idx - 1];
        let removed = if end <= old {
            before + (end - start) // 이 구간 전체가 old 이전
        } else {
            before + (old - start) // old가 이 구간 내부 → 시작으로 붕괴
        };
        old - removed
    }
}

/// 원시 세그먼트(원본 byte 범위)를 정리본 기준 문자 세그먼트로 확정한다.
///
/// ① cleanup 삭제맵으로 byte 범위 재매핑 후 정리본 길이로 clamp, ② 후행 공백·개행 트림
/// (byte 단위, char 경계 안전)으로 빈 것 제거, ③ 남은 경계를 char_indices 단일 패스로
/// 문자 오프셋으로 변환. 경계는 항상 줄 경계라 char 경계 위반이 없어야 하지만 방어적으로
/// 앞쪽 char 경계로 내림한다. 결과는 start 오름차순·비중첩·범위 내임을 debug_assert로 확인.
fn build_segments(
    cleaned: &str,
    deletions: &[(usize, usize)],
    raw: &[(usize, usize, usize, usize)],
) -> Vec<MarkdownSegment> {
    let map = DeletionMap::new(deletions);
    let clen = cleaned.len();

    // 1) 재매핑 + clamp + char 경계 내림 + 후행 트림 → 정리본 byte 세그먼트.
    let mut byte_segs: Vec<(usize, usize, usize, usize)> = Vec::with_capacity(raw.len());
    for &(section, para, s, e) in raw {
        let ns = floor_char_boundary(cleaned, map.remap(s).min(clen));
        let ne0 = floor_char_boundary(cleaned, map.remap(e).min(clen));
        if ne0 <= ns {
            continue;
        }
        // 후행 공백·개행만큼 end를 줄인다(인용이 깔끔하도록). 공백뿐이면 버린다.
        let trimmed = cleaned[ns..ne0].trim_end();
        let ne = ns + trimmed.len();
        if ne <= ns {
            continue;
        }
        byte_segs.push((section, para, ns, ne));
    }

    // 2) 모든 경계 byte 오프셋을 모아 정렬·유일화 후 char 오프셋으로 일괄 변환.
    let mut bounds: Vec<usize> = Vec::with_capacity(byte_segs.len() * 2);
    for &(_, _, s, e) in &byte_segs {
        bounds.push(s);
        bounds.push(e);
    }
    bounds.sort_unstable();
    bounds.dedup();
    let char_bounds = bytes_to_chars(cleaned, &bounds);
    let char_at = |b: usize| -> usize {
        let idx = bounds
            .binary_search(&b)
            .expect("경계는 bounds에 반드시 존재");
        char_bounds[idx]
    };

    // 3) 문자 세그먼트 조립.
    let segments: Vec<MarkdownSegment> = byte_segs
        .iter()
        .map(|&(section, para, s, e)| MarkdownSegment {
            section,
            para,
            start: char_at(s),
            end: char_at(e),
        })
        .collect();

    debug_assert!(
        segments.windows(2).all(|w| w[0].start <= w[1].start),
        "세그먼트는 start 오름차순이어야 함"
    );
    debug_assert!(
        segments.windows(2).all(|w| w[0].end <= w[1].start),
        "세그먼트는 비중첩이어야 함"
    );
    debug_assert!(
        segments
            .iter()
            .all(|seg| seg.start < seg.end && seg.end <= cleaned.chars().count()),
        "세그먼트는 범위 내(0 < 폭, end <= 문자수)여야 함"
    );
    segments
}

/// `i` 이하에서 가장 가까운 char 경계(std의 `floor_char_boundary` 대체 — 아직 unstable).
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 정렬·유일한 byte 경계 목록(모두 char 경계)을 char_indices 단일 패스로 char 오프셋으로
/// 변환한다. 입력과 같은 순서의 char 오프셋 벡터를 낸다.
fn bytes_to_chars(s: &str, bounds: &[usize]) -> Vec<usize> {
    let mut result = vec![0usize; bounds.len()];
    let mut bi = 0usize;
    let mut char_idx = 0usize;
    for (byte_off, _) in s.char_indices() {
        while bi < bounds.len() && bounds[bi] == byte_off {
            result[bi] = char_idx;
            bi += 1;
        }
        char_idx += 1;
    }
    // 남은 경계(= s.len(), 문자열 끝)는 전체 문자 수로 맺는다.
    while bi < bounds.len() {
        result[bi] = char_idx;
        bi += 1;
    }
    result
}

/// 인라인 문자 효과 상태. 열기 순서 bold→italic→strike→underline→sup/sub, 닫기는 역순.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct Marks {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    sup: bool,
    sub: bool,
}

impl Marks {
    fn from_shape(shape: Option<&CharShape>) -> Self {
        shape.map_or(Self::default(), |s| Self {
            bold: s.is_bold(),
            italic: s.is_italic(),
            strike: s.has_strike(),
            underline: s.has_underline(),
            sup: s.is_superscript(),
            sub: s.is_subscript(),
        })
    }

    fn open(self, html: bool) -> String {
        let mut s = String::new();
        if self.bold {
            s.push_str(if html { "<strong>" } else { "**" });
        }
        if self.italic {
            s.push_str(if html { "<em>" } else { "*" });
        }
        if self.strike {
            s.push_str(if html { "<s>" } else { "~~" });
        }
        if self.underline {
            s.push_str("<u>");
        }
        if self.sup {
            s.push_str("<sup>");
        }
        if self.sub {
            s.push_str("<sub>");
        }
        s
    }

    fn close(self, html: bool) -> String {
        let mut s = String::new();
        if self.sub {
            s.push_str("</sub>");
        }
        if self.sup {
            s.push_str("</sup>");
        }
        if self.underline {
            s.push_str("</u>");
        }
        if self.strike {
            s.push_str(if html { "</s>" } else { "~~" });
        }
        if self.italic {
            s.push_str(if html { "</em>" } else { "*" });
        }
        if self.bold {
            s.push_str(if html { "</strong>" } else { "**" });
        }
        s
    }
}

/// 열린 강조 스팬의 body 인덱스. `open_at`은 여는 마커의 시작, `content_at`은 여는
/// 마커 직후(= 내용 시작). 두 값은 스팬을 열 때만 갱신하며 닫을 때 공백 재배치에 쓴다.
#[derive(Default, Clone, Copy)]
struct Span {
    open_at: usize,
    content_at: usize,
}

/// 강조 스팬을 닫되 GFM 파싱 규칙을 지킨다.
///
/// GFM에서 `**`/`*`/`~~`는 닫는 구분자 **앞**이나 여는 구분자 **뒤**에 공백이 있으면
/// 강조로 파싱되지 않아 마커가 문자 그대로 노출된다(`**(목적) **` → 리터럴 `**`).
/// 그래서 굵게/기울임/취소선 스팬은 내용의 선두·후행 공백을 마커 **밖**으로 옮기고,
/// 내용이 공백뿐이면 마커를 아예 내지 않는다(`** **` 방지).
/// HTML 모드(`<strong>` 등)나 밑줄/첨자처럼 HTML 태그로 방출하는 효과만의 스팬은 공백에
/// 무관하므로 그대로 닫는다. `marks`가 비었으면(리셋 후) 아무것도 하지 않는다.
fn close_span(body: &mut String, marks: Marks, span: Span, html: bool) {
    if marks == Marks::default() {
        return;
    }
    // GFM 구분자가 없으면(HTML 모드이거나 밑줄/첨자뿐) 공백 재배치가 필요 없다.
    let needs_fix = !html && (marks.bold || marks.italic || marks.strike);
    if !needs_fix {
        body.push_str(&marks.close(html));
        return;
    }
    // 공백 판정은 char 단위 trim(멀티바이트 공백 U+3000 등 char 경계 안전).
    let content = &body[span.content_at..];
    let lead_len = content.len() - content.trim_start().len();
    let kept_len = content.trim_end().len();
    if kept_len == 0 {
        // 내용이 공백뿐 — 여는 마커를 제거하고 공백만 남긴다.
        body.replace_range(span.open_at..span.content_at, "");
        return;
    }
    // 후행 공백을 떼어 두고, 다듬은 내용 뒤에 닫는 마커를 붙인 다음 공백을 복원한다.
    let trail = body[span.content_at + kept_len..].to_string();
    body.truncate(span.content_at + kept_len);
    body.push_str(&marks.close(html));
    body.push_str(&trail);
    // 선두 공백은 여는 마커 앞으로 옮긴다.
    if lead_len > 0 {
        let lead = body[span.content_at..span.content_at + lead_len].to_string();
        body.replace_range(span.content_at..span.content_at + lead_len, "");
        body.insert_str(span.open_at, &lead);
    }
}

/// 열린 마크를 전부 닫고 상태를 리셋한다(링크 경계·줄바꿈 등 강제 경계).
/// 이후 Text 문자가 오면 모양 전환 로직이 다시 연다.
fn close_marks(body: &mut String, marks: &mut Marks, span: Span, html: bool) {
    close_span(body, *marks, span, html);
    *marks = Marks::default();
}

fn render_paragraph(
    doc: &Document,
    para: &Paragraph,
    list_state: &mut ListState,
    ctx: &mut Ctx,
    out: &mut String,
) {
    // 목록 번호는 문서 순서대로 모든 문단에 대해 갱신한다(빈 문단도 카운트 — 렌더와 동일).
    let marker = list_state.marker(doc, para);
    // 개요 스타일 → 헤딩
    let heading = doc
        .header
        .styles
        .get(para.style.0 as usize)
        .and_then(|s| s.name.strip_prefix("개요 "))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .filter(|n| (1..=6).contains(n));

    let fragments = render_fragments(doc, para, ctx);

    if let Some(level) = heading {
        close_list_block(ctx, out);
        emit_paragraph_fragments(&fragments, out, Some(&format!("{} ", "#".repeat(level))));
        return;
    }

    if let Some(mk) = marker.as_deref() {
        let (ty, level, definition_id) = list_head(doc, para).unwrap_or((3, 1, 0));
        emit_list_fragments(&fragments, ty, level, definition_id, mk, ctx, out);
        return;
    }

    close_list_block(ctx, out);
    emit_paragraph_fragments(&fragments, out, None);
}

/// 인라인과 블록을 문서 등장 순서대로 유지하는 중간 표현.
enum Fragment {
    Inline(String),
    Block(String),
}

fn emit_paragraph_fragments(fragments: &[Fragment], out: &mut String, prefix: Option<&str>) {
    let mut prefix = prefix;
    for fragment in fragments {
        match fragment {
            Fragment::Inline(text) => {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                if let Some(p) = prefix.take() {
                    out.push_str(p);
                }
                out.push_str(text);
                out.push_str("\n\n");
            }
            Fragment::Block(block) => append_block(out, block, 0),
        }
    }
}

fn append_block(out: &mut String, block: &str, indent: usize) {
    let block = block.trim_matches('\n');
    if block.is_empty() {
        return;
    }
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    let pad = " ".repeat(indent);
    for line in block.lines() {
        if !line.is_empty() {
            out.push_str(&pad);
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
}

fn emit_list_fragments(
    fragments: &[Fragment],
    ty: u8,
    level: u8,
    definition_id: u16,
    marker: &str,
    ctx: &mut Ctx,
    out: &mut String,
) {
    let (gfm_marker, literal_prefix) = if ty == 3 {
        ("-", None)
    } else if is_digit_marker(marker) {
        (marker, None)
    } else {
        ("-", Some(marker))
    };
    let indent = prepare_list_level(ctx, ty, level, definition_id, gfm_marker, out);
    let first_prefix = format!("{}{gfm_marker} ", " ".repeat(indent));
    let continuation = " ".repeat(indent + gfm_marker.chars().count() + 1);
    let mut emitted = false;
    let mut literal_pending = literal_prefix;

    for fragment in fragments {
        match fragment {
            Fragment::Inline(text) => {
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                let content = match literal_pending.take() {
                    Some(literal) => format!("{literal} {text}"),
                    None => text.to_string(),
                };
                if emitted {
                    out.push('\n');
                    push_indented_lines(out, &continuation, &continuation, &content);
                } else {
                    push_indented_lines(out, &first_prefix, &continuation, &content);
                    emitted = true;
                }
            }
            Fragment::Block(block) => {
                if !emitted {
                    let initial = literal_pending.take().unwrap_or_default();
                    out.push_str(&first_prefix);
                    out.push_str(initial);
                    out.push('\n');
                    emitted = true;
                }
                append_block(out, block, continuation.len());
            }
        }
    }
    if emitted && !out.ends_with('\n') {
        out.push('\n');
    }
    ctx.last_was_list = emitted;
}

fn push_indented_lines(out: &mut String, first: &str, continuation: &str, text: &str) {
    for (i, line) in text.lines().enumerate() {
        out.push_str(if i == 0 { first } else { continuation });
        out.push_str(line);
        out.push('\n');
    }
}

fn prepare_list_level(
    ctx: &mut Ctx,
    ty: u8,
    level: u8,
    definition_id: u16,
    marker: &str,
    out: &mut String,
) -> usize {
    let index = level.saturating_sub(1) as usize;
    if ctx
        .list_keys
        .get(index)
        .is_some_and(|key| *key != (ty, definition_id))
    {
        if index == 0 {
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str("<!-- list-break -->\n\n");
        } else if !out.ends_with("\n\n") {
            out.push('\n');
        }
        ctx.list_widths.truncate(index);
        ctx.list_keys.truncate(index);
    }
    while ctx.list_widths.len() < index {
        ctx.list_widths.push(2);
        ctx.list_keys.push((ty, definition_id));
    }
    let indent = ctx.list_widths.iter().take(index).sum();
    let width = marker.chars().count() + 1;
    if ctx.list_widths.len() == index {
        ctx.list_widths.push(width);
        ctx.list_keys.push((ty, definition_id));
    } else {
        ctx.list_widths[index] = width;
        ctx.list_keys[index] = (ty, definition_id);
        ctx.list_widths.truncate(index + 1);
        ctx.list_keys.truncate(index + 1);
    }
    indent
}

/// 목록 블록 종료 — 항목 뒤 일반 문단/헤딩 앞에 빈 줄을 확보한다.
fn close_list_block(ctx: &mut Ctx, out: &mut String) {
    if ctx.last_was_list && !out.ends_with("\n\n") {
        out.push('\n');
    }
    ctx.last_was_list = false;
    ctx.list_widths.clear();
    ctx.list_keys.clear();
}

fn break_section_list(ctx: &mut Ctx, out: &mut String) {
    if ctx.last_was_list {
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str("<!-- list-break -->\n\n");
    }
    ctx.last_was_list = false;
    ctx.list_widths.clear();
    ctx.list_keys.clear();
}

/// (머리 종류, 수준) — 2=번호, 3=글머리표. 목록이 아니면 None.
fn list_head(doc: &Document, para: &Paragraph) -> Option<(u8, u8, u16)> {
    let ps = doc.header.para_shapes.get(para.para_shape.0 as usize)?;
    let ty = ps.head_type();
    (ty == 2 || ty == 3).then(|| (ty, ps.head_level(), ps.numbering_id))
}

/// "1."/"1)" 같이 GFM 순서 목록으로 쓸 수 있는 형태인지.
fn is_digit_marker(mk: &str) -> bool {
    let Some(digits) = mk.strip_suffix('.').or_else(|| mk.strip_suffix(')')) else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// 문단 내용을 인라인/블록 fragment로 등장 순서대로 렌더링한다.
fn render_fragments(doc: &Document, para: &Paragraph, ctx: &mut Ctx) -> Vec<Fragment> {
    let mut fragments = Vec::new();
    let mut body = String::new();
    let mut wchar_pos = 0u32;
    let mut marks = Marks::default();
    // 현재 열린 강조 스팬의 body 인덱스. 스팬을 열 때만 갱신한다.
    let mut span = Span::default();
    // 하이퍼링크 필드 열림 상태(대상 URL). FIELD_START에서 채우고 FIELD_END에서 닫는다.
    let mut link_url: Option<String> = None;

    for ch in &para.chars {
        // 현재 위치의 문자 모양으로 효과 전환
        // (중첩 정합성을 위해 변경 시 전부 닫고 다시 연다)
        if let HwpChar::Text(_) = ch {
            let mut want = Marks::from_shape(shape_at(doc, para, wchar_pos));
            // 하이퍼링크 표시 텍스트의 밑줄은 링크 표기 자체가 함의한다 —
            // `[<u>텍스트</u>](URL)` 같은 군더더기를 막는다.
            if link_url.is_some() {
                want.underline = false;
            }
            if want != marks {
                close_span(&mut body, marks, span, ctx.html_mode);
                span.open_at = body.len();
                body.push_str(&want.open(ctx.html_mode));
                span.content_at = body.len();
                marks = want;
            }
        }
        match ch {
            HwpChar::Text(c) => {
                if ctx.html_mode {
                    push_html_escaped(&mut body, *c);
                } else {
                    body.push(*c);
                }
            }
            HwpChar::CharCtrl(code) => match *code {
                ctrl_char::LINE_BREAK => {
                    close_marks(&mut body, &mut marks, span, ctx.html_mode);
                    body.push_str(if ctx.html_mode { "<br/>" } else { "  \n" });
                }
                ctrl_char::HYPHEN => body.push('-'),
                ctrl_char::NB_SPACE | ctrl_char::FW_SPACE => body.push(' '),
                _ => {}
            },
            HwpChar::InlineCtrl { code, .. } => {
                if *code == ctrl_char::FIELD_END {
                    // 하이퍼링크 표시 텍스트 종료 → `](URL)`/`</a>`로 닫는다.
                    if let Some(url) = link_url.take() {
                        close_marks(&mut body, &mut marks, span, ctx.html_mode);
                        if ctx.html_mode {
                            body.push_str("</a>");
                        } else {
                            body.push_str("](");
                            body.push_str(&md_link_dest(&url));
                            body.push(')');
                        }
                    }
                } else if *code == ctrl_char::TAB {
                    body.push(' ');
                }
            }
            HwpChar::ExtCtrl {
                code, ctrl_index, ..
            } => {
                if let Some(idx) = ctrl_index
                    && let Some(control) = para.controls.get(*idx as usize)
                {
                    if *code == ctrl_char::FIELD_START
                        && let Some(url) = crate::field::hyperlink_url(control)
                    {
                        // 하이퍼링크 필드 시작 → `[`/`<a href>` 방출, 이후 표시 텍스트를 링크로 묶는다.
                        close_marks(&mut body, &mut marks, span, ctx.html_mode);
                        if ctx.html_mode {
                            body.push_str("<a href=\"");
                            for c in url.chars() {
                                match c {
                                    '&' => body.push_str("&amp;"),
                                    '"' => body.push_str("&quot;"),
                                    '<' => body.push_str("&lt;"),
                                    _ => body.push(c),
                                }
                            }
                            body.push_str("\">");
                        } else {
                            body.push('[');
                        }
                        link_url = Some(url);
                    } else {
                        render_control(
                            doc,
                            control,
                            *code,
                            ctx,
                            &mut body,
                            &mut marks,
                            &mut fragments,
                            span,
                        );
                    }
                }
            }
        }
        wchar_pos += ch.wchar_width();
    }
    close_span(&mut body, marks, span, ctx.html_mode);
    flush_inline(&mut body, &mut fragments);
    fragments
}

fn flush_inline(body: &mut String, fragments: &mut Vec<Fragment>) {
    if !body.is_empty() {
        fragments.push(Fragment::Inline(std::mem::take(body)));
    }
}

fn push_block(
    body: &mut String,
    marks: &mut Marks,
    html: bool,
    fragments: &mut Vec<Fragment>,
    block: String,
    span: Span,
) {
    close_marks(body, marks, span, html);
    flush_inline(body, fragments);
    fragments.push(Fragment::Block(block));
}

/// markdown 링크 대상 포맷: 공백·괄호가 있으면 `<...>`로 감싼다.
fn md_link_dest(url: &str) -> String {
    if url
        .chars()
        .any(|c| c.is_whitespace() || c == '(' || c == ')')
    {
        format!("<{}>", url.replace('<', "%3C").replace('>', "%3E"))
    } else {
        url.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_control(
    doc: &Document,
    control: &Control,
    code: u16,
    ctx: &mut Ctx,
    body: &mut String,
    marks: &mut Marks,
    fragments: &mut Vec<Fragment>,
    span: Span,
) {
    match control {
        Control::SectionDef(_) => {}
        Control::Picture(pic) => match doc.resolve_bin(&pic.bin_ref) {
            Some(data) => {
                let r = ctx.image_ref(data);
                body.push_str(&r);
            }
            None => body.push_str(if ctx.html_mode {
                "<!-- image -->"
            } else {
                "![image]()"
            }),
        },
        Control::Table(table) => {
            let mut block = String::new();
            // 병합 셀·셀 안 블록은 GFM 파이프 표로 표현 불가 → HTML 표 폴백.
            if ctx.html_mode || has_span(table) || has_block_content(table, ctx) {
                render_html_table(doc, table, ctx, &mut block);
            } else {
                render_gfm_table(doc, table, ctx, &mut block);
            }
            push_block(body, marks, ctx.html_mode, fragments, block, span);
        }
        Control::Generic(g) => {
            // 수식 → $스크립트$ (원문 보존).
            if let Some(eq) = &g.equation {
                render_equation(eq, ctx, body, marks, fragments, span);
                return;
            }
            // 각주/미주 → 본문 `[^N]` 마커 + 문서 끝 정의 (본문 인라인 흡수 대체).
            if code == ctrl_char::FOOTNOTE_ENDNOTE && matches!(&g.ctrl_id, b"fn  " | b"en  ") {
                let label = if g.ctrl_id == *b"fn  " {
                    ctx.foot_n += 1;
                    ctx.foot_n.to_string()
                } else {
                    ctx.end_n += 1;
                    format!("e{}", ctx.end_n)
                };
                // Render the definition body as markdown regardless of position (definitions
                // are always emitted as top-level `[^N]:` at the document end, so they must
                // not contain HTML inline markup — #47).
                let saved_html = ctx.html_mode;
                ctx.html_mode = false;
                let text = note_text(doc, g, ctx);
                ctx.html_mode = saved_html;
                let src = (ctx.cur_section, ctx.cur_para);
                ctx.notes.push(Note {
                    label: label.clone(),
                    text,
                    src,
                });
                if ctx.html_mode {
                    body.push_str(&format!(
                        "<sup id=\"fnref-{label}\"><a href=\"#fn-{label}\">{label}</a></sup>"
                    ));
                } else {
                    body.push_str(&format!("[^{label}]"));
                }
                return;
            }
            // 머리말/꼬리말·숨은설명은 옵션에 따라 제외 (텍스트 추출 정책과 동일).
            if (code == ctrl_char::HEADER_FOOTER && !ctx.include_header_footer)
                || (code == ctrl_char::HIDDEN_COMMENT && !ctx.include_hidden)
            {
                return;
            }
            for list in &g.paragraph_lists {
                for p in &list.paragraphs {
                    for fragment in render_fragments(doc, p, ctx) {
                        match fragment {
                            Fragment::Inline(inline) => {
                                let inline = inline.trim();
                                if !inline.is_empty() {
                                    if !body.is_empty() && !body.ends_with([' ', '\n']) {
                                        body.push(' ');
                                    }
                                    body.push_str(inline);
                                }
                            }
                            Fragment::Block(block) => {
                                push_block(body, marks, ctx.html_mode, fragments, block, span);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 수식 — 인라인은 `$..$`, 블록은 `$$..$$` (HWP 수식 스크립트 원문, LaTeX 아님).
fn render_equation(
    eq: &Equation,
    ctx: &mut Ctx,
    body: &mut String,
    marks: &mut Marks,
    fragments: &mut Vec<Fragment>,
    span: Span,
) {
    if ctx.html_mode && eq.inline {
        body.push_str("<code>");
        for c in eq.script.chars() {
            push_html_escaped(body, c);
        }
        body.push_str("</code>");
    } else if eq.inline {
        body.push('$');
        body.push_str(&eq.script);
        body.push('$');
    } else if ctx.html_mode {
        let mut block = String::from("<div class=\"hwp-equation\"><code>");
        for c in eq.script.chars() {
            push_html_escaped(&mut block, c);
        }
        block.push_str("</code></div>");
        push_block(body, marks, true, fragments, block, span);
    } else {
        push_block(
            body,
            marks,
            false,
            fragments,
            format!("$$\n{}\n$$", eq.script),
            span,
        );
    }
}

/// 각주/미주 본문 — 문단 fragment를 등장 순서대로 합친다.
fn note_text(doc: &Document, g: &GenericControl, ctx: &mut Ctx) -> String {
    let mut parts: Vec<String> = Vec::new();
    for list in &g.paragraph_lists {
        for p in &list.paragraphs {
            for fragment in render_fragments(doc, p, ctx) {
                let text = match fragment {
                    Fragment::Inline(text) | Fragment::Block(text) => text,
                };
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

/// 병합 셀(가로/세로)이 하나라도 있으면 GFM 파이프 표로 표현 불가.
fn has_span(table: &Table) -> bool {
    table.cells.iter().any(|c| c.col_span > 1 || c.row_span > 1)
}

/// 셀 안에 표·블록 수식 등 블록 fragment가 있으면 GFM 파이프 표로 표현 불가.
fn has_block_content(table: &Table, ctx: &Ctx) -> bool {
    table
        .cells
        .iter()
        .any(|c| c.paragraphs.iter().any(|p| paragraph_has_block(p, ctx)))
}

fn paragraph_has_block(para: &Paragraph, ctx: &Ctx) -> bool {
    para.controls.iter().any(|control| match control {
        Control::Table(_) => true,
        Control::Generic(g) => {
            let excluded = ((g.ctrl_id == *b"head" || g.ctrl_id == *b"foot")
                && !ctx.include_header_footer)
                || (g.ctrl_id == *b"tcmt" && !ctx.include_hidden);
            !excluded
                && (g.equation.as_ref().is_some_and(|eq| !eq.inline)
                    || g.paragraph_lists.iter().any(|list| {
                        list.paragraphs
                            .iter()
                            .any(|paragraph| paragraph_has_block(paragraph, ctx))
                    }))
        }
        _ => false,
    })
}

/// GFM 파이프 표 (첫 행 헤더, 병합 없음).
fn render_gfm_table(doc: &Document, table: &Table, ctx: &mut Ctx, out: &mut String) {
    let cols = table.cols.max(1) as usize;
    let mut grid: Vec<Vec<String>> = Vec::new();
    for cell in &table.cells {
        let row = cell.row as usize;
        while grid.len() <= row {
            grid.push(vec![String::new(); cols]);
        }
        let mut text = String::new();
        for p in &cell.paragraphs {
            for fragment in render_fragments(doc, p, ctx) {
                let Fragment::Inline(inline) = fragment else {
                    debug_assert!(false, "블록 셀은 HTML 표로 선분기되어야 함");
                    continue;
                };
                if !text.is_empty() && !inline.is_empty() {
                    text.push(' ');
                }
                text.push_str(inline.trim());
            }
        }
        if let Some(slot) = grid[row].get_mut(cell.col as usize) {
            *slot = text.replace('|', "\\|").replace('\n', " ");
        }
    }
    out.push('\n');
    for (i, row) in grid.iter().enumerate() {
        out.push_str("| ");
        out.push_str(&row.join(" | "));
        out.push_str(" |\n");
        if i == 0 {
            out.push_str(&format!("|{}\n", " --- |".repeat(cols)));
        }
    }
    out.push('\n');
}

/// HTML 표 — 병합 셀(colspan/rowspan)·셀 내 블록(중첩 표 포함)을 보존한다.
/// 블록 HTML 안에선 md가 렌더되지 않으므로 셀 내용은 html_mode로 방출한다.
///
/// **표 행 한 줄 불변식**: 각 `<tr>…</tr>`는 항상 한 줄에 담긴다(행 안에 개행 없음). 최상위
/// 표는 행마다 한 줄(가독성 + 소비자의 행 단위 인용). 셀 안에 든 **중첩 표**(html_mode)는
/// 개행 없이 통째로 한 줄로 직렬화해 바깥 행의 한 줄을 깨지 않게 한다 — 중첩 표의 모든 행이
/// 바깥 행과 같은 한 줄에 얹히며, 그래도 `<tr` 수 == `</tr>` 수가 줄마다 성립한다.
fn render_html_table(doc: &Document, table: &Table, ctx: &mut Ctx, out: &mut String) {
    let rows = table.rows.max(1) as usize;
    let cols = table.cols.max(1) as usize;
    // 셀 안 중첩 표는 html_mode에서 진입한다 — 이때만 개행 없는 한 줄 형태로 낸다.
    let nested = ctx.html_mode;
    // 병합 셀이 덮는 칸 표시 격자.
    let mut covered = vec![vec![false; cols]; rows];
    out.push_str(if nested { "<table>" } else { "\n<table>\n" });
    for r in 0..rows {
        out.push_str("<tr>");
        for c in 0..cols {
            if covered[r][c] {
                continue; // 앞선 병합 셀이 덮은 칸
            }
            let Some(cell) = table
                .cells
                .iter()
                .find(|cell| cell.row as usize == r && cell.col as usize == c)
            else {
                out.push_str("<td></td>");
                continue;
            };
            for dr in 0..cell.row_span.max(1) as usize {
                for dc in 0..cell.col_span.max(1) as usize {
                    if let Some(slot) = covered.get_mut(r + dr).and_then(|row| row.get_mut(c + dc))
                    {
                        *slot = true;
                    }
                }
            }
            let mut attrs = String::new();
            if cell.col_span > 1 {
                attrs.push_str(&format!(" colspan=\"{}\"", cell.col_span));
            }
            if cell.row_span > 1 {
                attrs.push_str(&format!(" rowspan=\"{}\"", cell.row_span));
            }
            let content = render_cell_html(doc, cell, ctx);
            out.push_str(&format!("<td{attrs}>{content}</td>"));
        }
        // 최상위 표만 행 뒤에 개행 — 중첩 표는 한 줄을 유지한다.
        out.push_str(if nested { "</tr>" } else { "</tr>\n" });
    }
    out.push_str(if nested { "</table>" } else { "</table>\n\n" });
}

/// 셀 내용을 html_mode fragment로 렌더해 원래 순서를 보존한다.
fn render_cell_html(doc: &Document, cell: &Cell, ctx: &mut Ctx) -> String {
    let saved = ctx.html_mode;
    ctx.html_mode = true;
    let mut content = String::new();
    for p in &cell.paragraphs {
        for fragment in render_fragments(doc, p, ctx) {
            let text = match fragment {
                Fragment::Inline(text) | Fragment::Block(text) => text,
            };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if !content.is_empty() {
                content.push_str("<br/>");
            }
            content.push_str(text);
        }
    }
    ctx.html_mode = saved;
    content
}

/// HTML 텍스트 노드 이스케이프 (& < >).
fn push_html_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        _ => out.push(c),
    }
}

fn escape_html_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// 주어진 WCHAR 위치의 문자 모양.
fn shape_at<'d>(doc: &'d Document, para: &Paragraph, pos: u32) -> Option<&'d CharShape> {
    let id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| *id)?;
    doc.header.char_shapes.get(id.0 as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown::from_markdown;
    use hwp_model::{CharShapeId, HwpChar, ParagraphList};

    /// 하이퍼링크가 md→IR→md 왕복에서 `[표시](URL)`로 보존된다.
    /// (링크 표시 텍스트의 밑줄 서식은 링크 표기가 함의하므로 `<u>`를 내지 않는다.)
    #[test]
    fn 하이퍼링크_왕복_보존() {
        let doc = from_markdown("자세히는 [여기](https://example.com/path)를 본다\n");
        let md = to_markdown(&doc);
        assert!(
            md.contains("[여기](https://example.com/path)"),
            "링크 왕복: {md}"
        );
    }

    /// media_dir 미지정이면 이미지 참조는 빈 참조를 유지한다(동작 불변).
    #[test]
    fn 이미지_기본은_빈참조() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        crate::image::insert_image(
            &mut doc,
            "사진:",
            &write_temp("md_img_none.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        let md = to_markdown(&doc);
        assert!(md.contains("![image]()"), "빈 참조 유지: {md}");
    }

    /// media_dir 지정 시 이미지가 디렉터리에 추출되고 상대경로로 참조된다.
    #[test]
    fn 이미지_media_dir_추출() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        crate::image::insert_image(
            &mut doc,
            "사진:",
            &write_temp("md_img_extract.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();

        let dir = unique_dir("md_media_extract");
        // 추출 전에는 디렉터리가 없어야 한다(지연 생성 확인).
        assert!(!dir.exists());
        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap();
        let name = dir.file_name().unwrap().to_string_lossy();
        assert!(
            md.contains(&format!("![image]({name}/image1.png)")),
            "상대경로 참조: {md}"
        );
        let extracted = dir.join("image1.png");
        assert!(extracted.exists(), "이미지 파일 추출");
        assert_eq!(std::fs::read(&extracted).unwrap(), png, "추출 바이트 일치");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 이미지가 없으면 media_dir 지정이어도 디렉터리를 만들지 않는다.
    #[test]
    fn 이미지_없으면_디렉터리_미생성() {
        let doc = from_markdown("본문만 있는 문단\n");
        let dir = unique_dir("md_media_empty");
        let _ = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!dir.exists(), "이미지 없으면 디렉터리 미생성");
    }

    /// 이미지 여러 개가 등장 순서대로 image1/image2로 번호 매겨진다.
    #[test]
    fn 이미지_카운터_증가() {
        // 두 이미지가 순서대로 image1/image2로 번호 매겨진다.
        let mut doc = from_markdown("첫 사진: 여기\n\n둘 사진: 저기");
        let png = png_bytes();
        crate::image::insert_image(
            &mut doc,
            "첫 사진:",
            &write_temp("md_cnt1.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        crate::image::insert_image(
            &mut doc,
            "둘 사진:",
            &write_temp("md_cnt2.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        let dir = unique_dir("md_media_counter");
        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap();
        let name = dir.file_name().unwrap().to_string_lossy();
        assert!(md.contains(&format!("{name}/image1.png")), "첫 이미지");
        assert!(md.contains(&format!("{name}/image2.png")), "둘째 이미지");
        assert!(dir.join("image2.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// media_prefix 지정 시 이미지 참조가 디렉터리명 대신 접두사 경로를 쓴다.
    #[test]
    fn 이미지_media_prefix() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        crate::image::insert_image(
            &mut doc,
            "사진:",
            &write_temp("md_img_prefix.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        let dir = unique_dir("md_media_prefix");
        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                media_prefix: Some("figs"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            md.contains("![image](figs/image1.png)"),
            "prefix 참조: {md}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 이미지_경로를_markdown에_안전하게_인코딩() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        crate::image::insert_image(
            &mut doc,
            "사진:",
            &write_temp("md_img_escaped.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        let dir = unique_dir("md_media_escaped");
        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                media_prefix: Some(r"my figs\(draft)"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            md.contains("![image](<my figs/(draft)/image1.png>)"),
            "공백·괄호·구분자 처리: {md}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 이미지_충돌은_덮어쓰지_않고_동일파일만_재사용() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        crate::image::insert_image(
            &mut doc,
            "사진:",
            &write_temp("md_img_collision.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        let dir = unique_dir("md_media_collision");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("image1.png");
        std::fs::write(&target, b"existing file").unwrap();

        let error = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&target).unwrap(), b"existing file");

        std::fs::write(&target, &png).unwrap();
        to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                ..Default::default()
            },
        )
        .expect("동일 바이트는 재사용");
        assert_eq!(std::fs::read(&target).unwrap(), png);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 이미지_충돌은_새파일_기록_전에_선검사() {
        let mut doc = from_markdown("첫 사진: 여기\n\n둘 사진: 저기");
        let png = png_bytes();
        crate::image::insert_image(
            &mut doc,
            "첫 사진:",
            &write_temp("md_preflight1.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        crate::image::insert_image(
            &mut doc,
            "둘 사진:",
            &write_temp("md_preflight2.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        let dir = unique_dir("md_media_preflight");
        std::fs::create_dir_all(dir.join("image2.png")).unwrap();

        assert!(
            to_markdown_with(
                &doc,
                &MarkdownOptions {
                    media_dir: Some(&dir),
                    ..Default::default()
                },
            )
            .is_err()
        );
        assert!(!dir.join("image1.png").exists(), "선검사 전에는 기록 금지");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 각주/미주가 본문 `[^N]`/`[^eN]` 마커 + 문서 끝 정의로 방출된다 (GH-3).
    #[test]
    fn 각주_미주_마커와_정의() {
        let mut doc = from_markdown("본문 문단\n");
        let note = |id: &[u8; 4], txt: &str| {
            Control::Generic(GenericControl {
                ctrl_id: *id,
                data: vec![],
                paragraph_lists: vec![ParagraphList {
                    header_data: vec![],
                    paragraphs: vec![Paragraph {
                        chars: txt.chars().map(HwpChar::Text).collect(),
                        ..Paragraph::default()
                    }],
                }],
                extras: vec![],
                raw_children: vec![],
                gso_shapes: vec![],
                equation: None,
                column_def: None,
            })
        };
        let anchor = |idx: u32, id: &[u8; 4]| HwpChar::ExtCtrl {
            code: ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id: *id,
            payload: vec![],
            ctrl_index: Some(idx),
        };
        let para = &mut doc.sections[0].paragraphs[0];
        // 첫 문단에는 구역 정의 컨트롤이 선주입돼 있으므로 현재 개수가 곧 인덱스다.
        let i0 = para.controls.len() as u32;
        para.controls.push(note(b"fn  ", "각주 내용"));
        let i1 = para.controls.len() as u32;
        para.controls.push(note(b"en  ", "미주 내용"));
        para.chars.push(anchor(i0, b"fn  "));
        para.chars.push(anchor(i1, b"en  "));

        let md = to_markdown(&doc);
        assert!(md.contains("본문 문단[^1][^e1]"), "본문 마커: {md}");
        assert!(md.contains("[^1]: 각주 내용"), "각주 정의: {md}");
        assert!(md.contains("[^e1]: 미주 내용"), "미주 정의: {md}");
        // 정의는 문서 끝에 모인다.
        assert!(
            md.trim_end().ends_with("[^e1]: 미주 내용"),
            "정의는 문서 끝: {md}"
        );
    }

    #[test]
    fn html_표_각주는_html_마커와_gfm_정의로_연결() {
        let note = Control::Generic(GenericControl {
            ctrl_id: *b"fn  ",
            data: vec![],
            paragraph_lists: vec![ParagraphList {
                header_data: vec![],
                paragraphs: vec![Paragraph {
                    chars: "표 안 각주".chars().map(HwpChar::Text).collect(),
                    ..Paragraph::default()
                }],
            }],
            extras: vec![],
            raw_children: vec![],
            gso_shapes: vec![],
            equation: None,
            column_def: None,
        });
        let cell_para = Paragraph {
            chars: "셀 본문"
                .chars()
                .map(HwpChar::Text)
                .chain(std::iter::once(HwpChar::ExtCtrl {
                    code: ctrl_char::FOOTNOTE_ENDNOTE,
                    ctrl_id: *b"fn  ",
                    payload: vec![],
                    ctrl_index: Some(0),
                }))
                .collect(),
            controls: vec![note],
            ..Paragraph::default()
        };
        let mut doc = from_markdown("표\n");
        insert_table(
            &mut doc.sections[0].paragraphs[0],
            one_cell_table(cell_para, 2, 2),
        );

        let md = to_markdown(&doc);
        assert!(
            md.contains(r##"<sup id="fnref-1"><a href="#fn-1">1</a></sup>"##),
            "HTML 각주 참조: {md}"
        );
        // Definitions are GFM syntax regardless of reference position — an HTML block would
        // be rejected by the importer contract (#47).
        assert!(md.contains("[^1]: 표 안 각주"), "GFM 각주 정의: {md}");
        assert!(!md.contains("hwp-footnote"), "HTML 정의 블록 금지: {md}");
    }

    /// 수식이 인라인은 `$..$`, 블록은 `$$..$$`로 방출된다.
    #[test]
    fn 수식_인라인_블록() {
        let mk = |script: &str, inline: bool| {
            Control::Generic(GenericControl {
                ctrl_id: *b"eqed",
                data: vec![],
                paragraph_lists: vec![],
                extras: vec![],
                raw_children: vec![],
                gso_shapes: vec![],
                equation: Some(Equation {
                    script: script.to_string(),
                    width: 0,
                    height: 0,
                    inline,
                    x: 0,
                    y: 0,
                    ..Equation::default()
                }),
                column_def: None,
            })
        };
        let anchor = |idx: u32| HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *b"eqed",
            payload: vec![],
            ctrl_index: Some(idx),
        };
        let mut doc = from_markdown("인라인 수식: \n");
        let p0 = &mut doc.sections[0].paragraphs[0];
        // 첫 문단의 선주입 구역 정의 컨트롤 뒤 인덱스를 쓴다.
        let i0 = p0.controls.len() as u32;
        p0.chars.push(anchor(i0));
        p0.controls.push(mk("a+b", true));
        // 블록 수식만 있는 문단 추가.
        doc.sections[0].paragraphs.push(Paragraph {
            chars: vec![anchor(0)],
            controls: vec![mk("x^2", false)],
            ..Paragraph::default()
        });

        let md = to_markdown(&doc);
        assert!(md.contains("$a+b$"), "인라인 수식: {md}");
        assert!(md.contains("$$\nx^2\n$$"), "블록 수식: {md}");
    }

    #[test]
    fn 블록_수식_앞뒤_순서_보존() {
        let mut doc = from_markdown("앞 뒤\n");
        let para = &mut doc.sections[0].paragraphs[0];
        let index = para.controls.len() as u32;
        para.controls.push(equation_control("x^2", false));
        para.chars = "앞 "
            .chars()
            .map(HwpChar::Text)
            .chain(std::iter::once(control_anchor(index, b"eqed")))
            .chain(" 뒤".chars().map(HwpChar::Text))
            .collect();

        let md = to_markdown(&doc);
        let before = md.find("앞").unwrap();
        let equation = md.find("$$\nx^2\n$$").unwrap();
        let after = md.find("뒤").unwrap();
        assert!(
            before < equation && equation < after,
            "등장 순서 보존: {md}"
        );
    }

    #[test]
    fn 블록_수식_셀은_html_표로_내용_보존() {
        use hwp_model::{BorderFillId, HwpUnit};
        let mut cell_para = Paragraph::default();
        cell_para.controls.push(equation_control("a+b", false));
        cell_para.chars = "앞"
            .chars()
            .map(HwpChar::Text)
            .chain(std::iter::once(control_anchor(0, b"eqed")))
            .chain("뒤".chars().map(HwpChar::Text))
            .collect();
        let table = Table {
            rows: 1,
            cols: 1,
            row_cell_counts: vec![1],
            cells: vec![Cell {
                list_attr: 0,
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
                width: HwpUnit(0),
                height: HwpUnit(0),
                margins: [0; 4],
                border_fill: BorderFillId(0),
                header_tail: vec![],
                paragraphs: vec![cell_para],
            }],
            common_data: vec![],
            placement: None,
            attr: 0,
            cell_spacing: 0,
            inner_margins: [0; 4],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            extras: vec![],
        };
        let mut doc = from_markdown("표\n");
        insert_table(&mut doc.sections[0].paragraphs[0], table);

        let md = to_markdown(&doc);
        assert!(md.contains("<table>"), "블록 셀은 HTML 폴백: {md}");
        let before = md.find("<td>앞").unwrap();
        let equation = md.find("hwp-equation").unwrap();
        let after = md.find("뒤</td>").unwrap();
        assert!(before < equation && equation < after, "셀 순서 보존: {md}");
    }

    /// 글머리표/번호 문단이 GFM 목록으로 방출된다 (GH-6).
    #[test]
    fn 목록_불릿과_번호() {
        use hwp_model::{NumFmt, NumLevel, ParaShape, ParaShapeId};
        let ps = |ty: u32, lv: u32, nid: u16| ParaShape {
            attr1: (ty << 23) | (lv << 25),
            numbering_id: nid,
            ..ParaShape::default()
        };
        // 숫자 번호 목록. 카운터는 번호 정의(numbering id)별 독립이며,
        // 형식이 다른 목록은 문서를 나눠 따로 검증한다.
        let mut doc = from_markdown("불릿 하나\n\n번호 하나\n\n번호 둘\n");
        let base = doc.header.para_shapes.len() as u16;
        doc.header.para_shapes.push(ps(3, 1, 0)); // 불릿
        doc.header.para_shapes.push(ps(2, 1, 0)); // 번호(숫자)
        doc.header.bullet_chars = vec!['•'];
        doc.header.numbering_levels = vec![vec![NumLevel::default(); 7]];
        for (i, p) in doc.sections[0].paragraphs.iter_mut().enumerate() {
            p.para_shape = ParaShapeId(if i == 0 { base } else { base + 1 });
        }
        let md = to_markdown(&doc);
        assert!(md.contains("- 불릿 하나\n"), "불릿: {md}");
        assert!(md.contains("1. 번호 하나\n"), "숫자 번호 1: {md}");
        assert!(md.contains("2. 번호 둘\n"), "숫자 번호 2: {md}");

        // 가나다 형식 번호는 GFM 목록 마커가 없어 리터럴 마커로 보존한다.
        let mut doc2 = from_markdown("한글 번호\n");
        let base2 = doc2.header.para_shapes.len() as u16;
        doc2.header.para_shapes.push(ps(2, 1, 1));
        doc2.header.numbering_levels = vec![
            vec![NumLevel::default(); 7],
            vec![
                NumLevel {
                    start: 1,
                    fmt: NumFmt::HangulSyllable,
                    template: String::new(),
                };
                7
            ],
        ];
        doc2.sections[0].paragraphs[0].para_shape = ParaShapeId(base2);
        let md2 = to_markdown(&doc2);
        assert!(md2.contains("- 가. 한글 번호"), "한글 형식 리터럴: {md2}");
    }

    #[test]
    fn 목록_중첩은_부모_마커_폭으로_들여쓰기() {
        use hwp_model::{NumLevel, ParaShape, ParaShapeId};
        use pulldown_cmark::{Event, Parser, Tag, TagEnd};

        let mut doc = from_markdown("상위\n\n하위\n\n다음\n");
        let base = doc.header.para_shapes.len() as u16;
        let ps = |ty: u32, level: u32| ParaShape {
            attr1: (ty << 23) | (level << 25),
            numbering_id: 0,
            ..ParaShape::default()
        };
        doc.header.para_shapes.push(ps(2, 1));
        doc.header.para_shapes.push(ps(3, 2));
        doc.header.numbering_levels = vec![vec![NumLevel::default(); 7]];
        doc.header.bullet_chars = vec!['•'];
        for (paragraph, shape) in doc.sections[0]
            .paragraphs
            .iter_mut()
            .zip([base, base + 1, base])
        {
            paragraph.para_shape = ParaShapeId(shape);
        }

        let md = to_markdown(&doc);
        assert!(
            md.contains("1. 상위\n   - 하위\n2. 다음"),
            "목록 들여쓰기: {md}"
        );
        let mut depth = 0usize;
        let mut max_depth = 0usize;
        for event in Parser::new(&md) {
            match event {
                Event::Start(Tag::List(_)) => {
                    depth += 1;
                    max_depth = max_depth.max(depth);
                }
                Event::End(TagEnd::List(_)) => depth -= 1,
                _ => {}
            }
        }
        assert_eq!(max_depth, 2, "파서에서도 중첩 목록이어야 함: {md}");
    }

    #[test]
    fn 순서목록_마커_판별() {
        assert!(is_digit_marker("1."));
        assert!(is_digit_marker("12)"));
        assert!(!is_digit_marker("1"));
        assert!(!is_digit_marker("가."));
    }

    #[test]
    fn 구역_경계에서_번호목록_재시작() {
        use hwp_model::{NumLevel, ParaShape, ParaShapeId};
        use pulldown_cmark::{Event, Parser, Tag};

        let mut doc = from_markdown("첫 구역\n");
        let shape = doc.header.para_shapes.len() as u16;
        doc.header.para_shapes.push(ParaShape {
            attr1: (2 << 23) | (1 << 25),
            numbering_id: 0,
            ..ParaShape::default()
        });
        doc.header.numbering_levels = vec![vec![NumLevel::default(); 7]];
        doc.sections[0].paragraphs[0].para_shape = ParaShapeId(shape);
        let mut second = doc.sections[0].clone();
        second.paragraphs[0].chars = "둘째 구역".chars().map(HwpChar::Text).collect();
        doc.sections.push(second);

        let md = to_markdown(&doc);
        assert!(md.contains("<!-- list-break -->"), "구역 목록 분리: {md}");
        let starts = Parser::new(&md)
            .filter(|event| matches!(event, Event::Start(Tag::List(_))))
            .count();
        assert_eq!(starts, 2, "서로 다른 최상위 목록이어야 함: {md}");
    }

    /// 병합 셀이 있으면 HTML 표(colspan/rowspan)로 폴백한다 (GH-4).
    #[test]
    fn 병합셀_html_표() {
        use hwp_model::{BorderFillId, Cell, HwpUnit, Table};
        let cell = |row: u16, col: u16, cs: u16, rs: u16, txt: &str| Cell {
            list_attr: 0,
            col,
            row,
            col_span: cs,
            row_span: rs,
            width: HwpUnit(0),
            height: HwpUnit(0),
            margins: [0; 4],
            border_fill: BorderFillId(0),
            header_tail: vec![],
            paragraphs: vec![Paragraph {
                chars: txt.chars().map(HwpChar::Text).collect(),
                ..Paragraph::default()
            }],
        };
        let table = Table {
            common_data: vec![],
            placement: None,
            attr: 0,
            rows: 2,
            cols: 2,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1, 2],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            cells: vec![
                cell(0, 0, 2, 1, "병합"),
                cell(1, 0, 1, 1, "가"),
                cell(1, 1, 1, 1, "나"),
            ],
            extras: vec![],
        };
        let mut doc = from_markdown("표 문단\n");
        let p = &mut doc.sections[0].paragraphs[0];
        // 첫 문단의 선주입 구역 정의 컨트롤 뒤 인덱스를 쓴다.
        let i0 = p.controls.len() as u32;
        p.chars.push(HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *b"tbl ",
            payload: vec![],
            ctrl_index: Some(i0),
        });
        p.controls.push(Control::Table(table));

        let md = to_markdown(&doc);
        assert!(md.contains("<table>"), "HTML 표: {md}");
        assert!(md.contains("<td colspan=\"2\">병합</td>"), "colspan: {md}");
        assert!(md.contains("<td>가</td><td>나</td>"), "나머지 행: {md}");
    }

    /// GH-46: exporter의 HTML 표 출력이 importer 계약을 지켜야
    /// `cat --format markdown` → `new --from` 왕복에서 표가 살아남는다.
    #[test]
    fn markdown_왕복_html_표_보존() {
        // 병합 셀(HTML 표 경로) + 셀 내 복수 문단(<br/> 경로) + 볼드(<strong> 경로)를 모두 포함.
        let bold_para = Paragraph {
            chars: "굵은 글씨".chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0)), (2, CharShapeId(1))],
            ..Paragraph::default()
        };
        let mut table = merged_table();
        table.cells[0].paragraphs = vec![bold_para, text_para("둘째 줄")];
        let mut doc = from_markdown("표\n");
        doc.header.char_shapes = vec![bold_shape(), CharShape::default()];
        insert_table(&mut doc.sections[0].paragraphs[0], table);

        let md = to_markdown(&doc);
        // 계약 불변식: bare <br>/<b>/<i>가 없어야 한다 (html.rs의 <br/> 불변식과 동일).
        assert!(!md.contains("<br>"), "self-closing <br/>만 방출: {md}");
        assert!(!md.contains("<b>"), "<strong>만 방출: {md}");
        assert!(!md.contains("<i>"), "<em>만 방출: {md}");

        let (back, warnings) = crate::from_markdown::from_markdown_report(
            &md,
            &crate::from_markdown::MarkdownImportOptions::default(),
        );
        assert!(
            !warnings.iter().any(|w| w.contains("계약 위반")),
            "계약 위반 경고가 없어야 함: {warnings:?}"
        );
        let tables = back.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| p.controls.iter())
            .filter(|c| matches!(c, Control::Table(_)))
            .count();
        assert_eq!(tables, 1, "왕복 후 표가 보존되어야 함: {md}");
    }

    /// GH-47: footnotes/endnotes referenced inside an HTML table still emit their
    /// definitions as GFM (`[^N]:`), and the importer restores fnref markers to real
    /// footnote anchors, closing the round-trip.
    #[test]
    fn markdown_왕복_html_표_각주_보존() {
        let note = |id: &[u8; 4], txt: &str| {
            Control::Generic(GenericControl {
                ctrl_id: *id,
                data: vec![],
                paragraph_lists: vec![ParagraphList {
                    header_data: vec![],
                    paragraphs: vec![text_para(txt)],
                }],
                extras: vec![],
                raw_children: vec![],
                gso_shapes: vec![],
                equation: None,
                column_def: None,
            })
        };
        let anchor = |idx: u32, id: &[u8; 4]| HwpChar::ExtCtrl {
            code: ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id: *id,
            payload: vec![],
            ctrl_index: Some(idx),
        };
        let cell_para = Paragraph {
            chars: "셀 본문"
                .chars()
                .map(HwpChar::Text)
                .chain([anchor(0, b"fn  "), anchor(1, b"en  ")])
                .collect(),
            controls: vec![note(b"fn  ", "표 안 각주"), note(b"en  ", "표 안 미주")],
            ..Paragraph::default()
        };
        let mut table = merged_table();
        table.cells[0].paragraphs = vec![cell_para];
        let mut doc = from_markdown("표\n");
        insert_table(&mut doc.sections[0].paragraphs[0], table);

        let md = to_markdown(&doc);
        assert!(md.contains("[^1]: 표 안 각주"), "GFM 각주 정의: {md}");
        assert!(md.contains("[^e1]: 표 안 미주"), "GFM 미주 정의: {md}");
        assert!(!md.contains("hwp-footnote"), "HTML 정의 블록 금지: {md}");

        let (back, warnings) = crate::from_markdown::from_markdown_report(
            &md,
            &crate::from_markdown::MarkdownImportOptions::default(),
        );
        assert!(
            !warnings.iter().any(|w| w.contains("계약 위반")),
            "계약 위반 경고가 없어야 함: {warnings:?}"
        );
        // Find the fn/en anchors and their bodies inside the restored table cell.
        let mut bodies: Vec<(String, String)> = Vec::new();
        for p in &back.sections[0].paragraphs {
            for c in &p.controls {
                let Control::Table(t) = c else { continue };
                for cell in &t.cells {
                    for cp in &cell.paragraphs {
                        for cc in &cp.controls {
                            if let Control::Generic(g) = cc
                                && matches!(&g.ctrl_id, b"fn  " | b"en  ")
                            {
                                let text: String = g.paragraph_lists[0].paragraphs[0]
                                    .chars
                                    .iter()
                                    .filter_map(|ch| match ch {
                                        HwpChar::Text(c) => Some(*c),
                                        _ => None,
                                    })
                                    .collect();
                                bodies.push((
                                    String::from_utf8_lossy(&g.ctrl_id).trim().to_string(),
                                    text,
                                ));
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            bodies,
            vec![
                ("fn".to_string(), "표 안 각주".to_string()),
                ("en".to_string(), "표 안 미주".to_string())
            ],
            "각주/미주 본문 복원: {md}"
        );
        // The marker's back-link (<a href="#fn-N">) must not leak as a hyperlink.
        let cell_text: String = back.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| p.controls.iter())
            .filter_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .flat_map(|t| t.cells.iter())
            .flat_map(|c| c.paragraphs.iter())
            .flat_map(|p| p.chars.iter())
            .filter_map(|ch| match ch {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert!(
            !cell_text.contains('1') && !cell_text.contains('2'),
            "마커 숫자는 평문으로 남지 않음: {cell_text}"
        );
    }

    #[test]
    fn html_표_이미지_경로_속성_이스케이프() {
        let mut doc = from_markdown("사진: 여기");
        let png = png_bytes();
        crate::image::insert_image(
            &mut doc,
            "사진:",
            &write_temp("md_html_attr.png", &png),
            crate::image::ImageSize::Natural,
        )
        .unwrap();
        let cell_para = doc.sections[0].paragraphs.remove(0);
        let mut parent = Paragraph::default();
        insert_table(&mut parent, one_cell_table(cell_para, 2, 2));
        doc.sections[0].paragraphs.push(parent);
        let dir = unique_dir("md_html_attr");

        let md = to_markdown_with(
            &doc,
            &MarkdownOptions {
                media_dir: Some(&dir),
                media_prefix: Some("my figs/\"draft\"&more"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            md.contains(r#"src="my figs/&quot;draft&quot;&amp;more/image1.png""#),
            "HTML 속성 이스케이프: {md}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 밑줄/취소선/위·아래첨자가 스팬으로 방출된다.
    #[test]
    fn 글자효과_스팬() {
        let mut doc = from_markdown("효과\n");
        let shapes = [
            CharShape {
                attr: 1 << 2, // 밑줄(글자 아래)
                ..CharShape::default()
            },
            CharShape {
                strike: true,
                ..CharShape::default()
            },
            CharShape {
                attr: 1 << 15, // 위첨자
                ..CharShape::default()
            },
            CharShape {
                attr: 1 << 16, // 아래첨자
                ..CharShape::default()
            },
        ];
        let base = doc.header.char_shapes.len() as u16;
        doc.header.char_shapes.extend(shapes);
        let para = &mut doc.sections[0].paragraphs[0];
        para.chars = "ABCD".chars().map(HwpChar::Text).collect();
        para.char_shape_runs = (0..4)
            .map(|i| (i as u32, CharShapeId(base + i as u16)))
            .collect();

        let md = to_markdown(&doc);
        assert!(
            md.contains("<u>A</u>~~B~~<sup>C</sup><sub>D</sub>"),
            "효과 스팬: {md}"
        );
    }

    /// 문단 하나짜리 문서에 텍스트와 문자모양 run을 심는다. run은 (WCHAR 시작, 셰이프 인덱스).
    fn doc_with_runs(text: &str, shapes: &[CharShape], runs: &[(u32, usize)]) -> Document {
        let mut doc = from_markdown("x\n");
        let base = doc.header.char_shapes.len() as u16;
        doc.header.char_shapes.extend_from_slice(shapes);
        let para = &mut doc.sections[0].paragraphs[0];
        para.chars = text.chars().map(HwpChar::Text).collect();
        para.char_shape_runs = runs
            .iter()
            .map(|(pos, idx)| (*pos, CharShapeId(base + *idx as u16)))
            .collect();
        doc
    }

    fn bold_shape() -> CharShape {
        CharShape {
            attr: 1 << 1,
            ..CharShape::default()
        }
    }

    /// 볼드 run의 후행 공백은 닫는 `**` 뒤로 옮긴다(닫는 구분자 앞 공백 → 강조 파싱 실패 해소).
    #[test]
    fn 강조_후행공백_마커밖으로() {
        // "(목적) "가 볼드, "예시"가 보통.
        let doc = doc_with_runs(
            "(목적) 예시",
            &[bold_shape(), CharShape::default()],
            &[(0, 0), (5, 1)],
        );
        let md = to_markdown(&doc);
        assert!(md.contains("**(목적)** 예시"), "후행 공백 재배치: {md}");
        assert!(!md.contains("(목적) **"), "닫는 마커 앞 공백 없어야: {md}");
    }

    /// 볼드 run의 선두 공백은 여는 `**` 앞으로 옮긴다(여는 구분자 뒤 공백 → 강조 파싱 실패 해소).
    #[test]
    fn 강조_선두공백_마커밖으로() {
        // "앞"이 보통, " 강조"가 볼드(선두 공백 포함).
        let doc = doc_with_runs(
            "앞 강조",
            &[CharShape::default(), bold_shape()],
            &[(0, 0), (1, 1)],
        );
        let md = to_markdown(&doc);
        assert!(md.contains("앞 **강조**"), "선두 공백 재배치: {md}");
        assert!(!md.contains("** 강조"), "여는 마커 뒤 공백 없어야: {md}");
    }

    /// 내용이 공백뿐인 볼드 run은 마커를 아예 내지 않는다(`** **` 방지).
    #[test]
    fn 강조_공백뿐이면_마커미방출() {
        // "앞"·"뒤"는 보통, 가운데 " "만 볼드.
        let doc = doc_with_runs(
            "앞 뒤",
            &[CharShape::default(), bold_shape()],
            &[(0, 0), (1, 1), (2, 0)],
        );
        let md = to_markdown(&doc);
        assert!(md.contains("앞 뒤"), "공백만 남아야: {md}");
        assert!(!md.contains("**"), "마커 미방출: {md}");
    }

    /// 취소선도 동일하게 후행 공백을 닫는 `~~` 뒤로 옮긴다.
    #[test]
    fn 취소선_후행공백_마커밖으로() {
        let strike = CharShape {
            strike: true,
            ..CharShape::default()
        };
        // "지운 "이 취소선, "글"이 보통.
        let doc = doc_with_runs(
            "지운 글",
            &[strike, CharShape::default()],
            &[(0, 0), (3, 1)],
        );
        let md = to_markdown(&doc);
        assert!(md.contains("~~지운~~ 글"), "취소선 후행 공백 재배치: {md}");
        assert!(!md.contains("지운 ~~"), "닫는 마커 앞 공백 없어야: {md}");
    }

    /// HTML 모드(`<strong>`)는 태그가 공백에 무관하므로 후행 공백을 그대로 둔다.
    #[test]
    fn html_모드_볼드는_공백유지() {
        // 병합 셀 표 → HTML 폴백 → 셀 내용 html_mode. 셀에 "가 "(볼드) + "나"(보통).
        let cell_para = Paragraph {
            chars: "가 나".chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0)), (2, CharShapeId(1))],
            ..Paragraph::default()
        };
        let mut doc = from_markdown("표\n");
        doc.header.char_shapes = vec![bold_shape(), CharShape::default()];
        insert_table(
            &mut doc.sections[0].paragraphs[0],
            one_cell_table(cell_para, 2, 2),
        );
        let md = to_markdown(&doc);
        assert!(
            md.contains("<strong>가 </strong>나"),
            "HTML 볼드 공백 유지: {md}"
        );
    }

    // ── 세그먼트 맵 (기능 A) ──────────────────────────────────────────────

    /// 세그먼트 [start,end)의 문자 슬라이스(문자 단위 — Python str 인덱싱과 동일).
    fn char_slice(md: &str, seg: &MarkdownSegment) -> String {
        md.chars()
            .skip(seg.start)
            .take(seg.end - seg.start)
            .collect()
    }

    /// 텍스트만 있는 최상위 문단.
    fn text_para(text: &str) -> Paragraph {
        Paragraph {
            chars: text.chars().map(HwpChar::Text).collect(),
            ..Paragraph::default()
        }
    }

    /// 병합 셀(colspan)을 가진 2행 표 — HTML 경로로 폴백된다.
    fn merged_table() -> Table {
        use hwp_model::{BorderFillId, HwpUnit};
        let cell = |row: u16, col: u16, cs: u16, rs: u16, txt: &str| Cell {
            list_attr: 0,
            col,
            row,
            col_span: cs,
            row_span: rs,
            width: HwpUnit(0),
            height: HwpUnit(0),
            margins: [0; 4],
            border_fill: BorderFillId(0),
            header_tail: vec![],
            paragraphs: vec![text_para(txt)],
        };
        Table {
            common_data: vec![],
            placement: None,
            attr: 0,
            rows: 2,
            cols: 2,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1, 2],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            cells: vec![
                cell(0, 0, 2, 1, "제목행"),
                cell(1, 0, 1, 1, "가"),
                cell(1, 1, 1, 1, "나"),
            ],
            extras: vec![],
        }
    }

    /// 각주/미주 컨트롤(본문 참조 앵커 없이 컨트롤만).
    fn note_control(id: &[u8; 4], txt: &str) -> Control {
        Control::Generic(GenericControl {
            ctrl_id: *id,
            data: vec![],
            paragraph_lists: vec![ParagraphList {
                header_data: vec![],
                paragraphs: vec![text_para(txt)],
            }],
            extras: vec![],
            raw_children: vec![],
            gso_shapes: vec![],
            equation: None,
            column_def: None,
        })
    }

    /// 첫 문단을 "개요 1" 헤딩으로 만들고 인덱스를 돌려준다.
    fn make_heading(doc: &mut Document, para_index: usize) {
        use hwp_model::StyleId;
        let style_idx = doc.header.styles.len() as u16;
        doc.header.styles.push(hwp_model::header::Style {
            name: "개요 1".to_string(),
            ..Default::default()
        });
        doc.sections[0].paragraphs[para_index].style = StyleId(style_idx);
    }

    /// (1) 한국어 여러 문단(제목 포함)에서 세그먼트가 **문자 오프셋**으로 정확한 원본
    /// 문단 텍스트를 가리키는지 — 바이트 오프셋이었다면 반드시 어긋나는 한국어 배치.
    #[test]
    fn 세그먼트_한국어_문자오프셋_정합() {
        let mut doc = from_markdown("가나다\n\n라마바\n\n사아자\n");
        make_heading(&mut doc, 0); // "가나다" → "# 가나다"
        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();

        // (a) 논-ASCII → 바이트 수 != 문자 수.
        assert_ne!(
            md.len(),
            md.chars().count(),
            "한국어 포함이면 byte != char: {md:?}"
        );

        // (b)(c) 각 세그먼트 슬라이스와 (section, para) 인덱스가 기대와 정확히 일치.
        let expected = [
            (0usize, 0usize, "# 가나다"),
            (0, 1, "라마바"),
            (0, 2, "사아자"),
        ];
        assert_eq!(
            segs.len(),
            expected.len(),
            "세그먼트 3개: {segs:?} / {md:?}"
        );
        for (seg, (sec, par, text)) in segs.iter().zip(expected) {
            assert_eq!((seg.section, seg.para), (sec, par), "좌표: {seg:?}");
            assert_eq!(char_slice(&md, seg), text, "슬라이스: {seg:?} in {md:?}");
        }
    }

    /// (2) 불변식(정렬·비중첩·범위·인덱스 유효) + to_markdown_with와 문자열 완전 동일.
    #[test]
    fn 세그먼트_불변식과_문자열_동일() {
        let mut doc = from_markdown("서론 문단\n\n본론 문단\n\n결론 문단\n");
        make_heading(&mut doc, 0);
        // 표 문단도 하나 끼운다(블록 포함 경로).
        insert_table(&mut doc.sections[0].paragraphs[1], merged_table());

        let opts = MarkdownOptions::default();
        let (md, segs) = to_markdown_with_segments(&doc, &opts).unwrap();
        let plain = to_markdown_with(&doc, &opts).unwrap();
        assert_eq!(plain, md, "to_markdown_with와 세그먼트 경로 문자열 동일");

        let n = md.chars().count();
        for w in segs.windows(2) {
            assert!(w[0].start <= w[1].start, "정렬: {:?}", segs);
            assert!(w[0].end <= w[1].start, "비중첩: {:?}", segs);
        }
        for seg in &segs {
            assert!(seg.start < seg.end && seg.end <= n, "범위: {seg:?} (n={n})");
            assert!(seg.section < doc.sections.len(), "섹션 인덱스: {seg:?}");
            assert!(
                seg.para < doc.sections[seg.section].paragraphs.len(),
                "문단 인덱스: {seg:?}"
            );
        }
    }

    /// (3) 표가 만든 `<tr>` 줄들이 표를 담은 문단의 세그먼트 범위 안에 있다.
    #[test]
    fn 세그먼트_표_문단_상속() {
        let mut doc = from_markdown("표 앞 문단\n\n표 문단\n\n표 뒤 문단\n");
        insert_table(&mut doc.sections[0].paragraphs[1], merged_table());
        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();
        assert!(md.contains("<table>"), "HTML 표 경로: {md}");

        let seg = segs
            .iter()
            .find(|s| s.section == 0 && s.para == 1)
            .expect("표 문단 세그먼트");
        // md 안 모든 "<tr" 시작 문자 오프셋이 표 문단 세그먼트 범위 안에 있어야 한다.
        let tr_offsets: Vec<usize> = md
            .char_indices()
            .enumerate()
            .filter(|(_, (b, _))| md[*b..].starts_with("<tr"))
            .map(|(ci, _)| ci)
            .collect();
        assert!(!tr_offsets.is_empty(), "<tr> 존재: {md}");
        for off in tr_offsets {
            assert!(
                seg.start <= off && off < seg.end,
                "<tr>@{off}가 표 문단 세그먼트 {seg:?} 밖: {md}"
            );
        }
    }

    /// (4) 연속 빈 줄이 삭제(cleanup)된 뒤에도 각 세그먼트 슬라이스가 올바른 문단을 가리킨다.
    /// 문단 안 강제 줄바꿈 3회가 빈 줄 2개를 만들어 cleanup이 한 줄을 삭제하게 한다.
    #[test]
    fn 세그먼트_cleanup_재매핑() {
        let mut doc = from_markdown("머리 문단\n\n채움 문단\n\n꼬리 문단\n");
        // 가운데 문단을 "앞[줄바꿈×3]뒤"로 바꿔 연속 빈 줄을 유발.
        let p1 = &mut doc.sections[0].paragraphs[1];
        p1.char_shape_runs = vec![];
        p1.chars = vec![
            HwpChar::Text('앞'),
            HwpChar::CharCtrl(ctrl_char::LINE_BREAK),
            HwpChar::CharCtrl(ctrl_char::LINE_BREAK),
            HwpChar::CharCtrl(ctrl_char::LINE_BREAK),
            HwpChar::Text('뒤'),
        ];
        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();
        assert_eq!(
            to_markdown_with(&doc, &MarkdownOptions::default()).unwrap(),
            md
        );

        let slice_of = |sec: usize, par: usize| -> String {
            let seg = segs
                .iter()
                .find(|s| s.section == sec && s.para == par)
                .unwrap_or_else(|| panic!("세그먼트 ({sec},{par}) 없음: {segs:?}"));
            char_slice(&md, seg)
        };
        // 삭제 앞 문단과 삭제 뒤 문단이 정확한 텍스트를 가리켜야 한다(재매핑 검증).
        assert_eq!(slice_of(0, 0), "머리 문단", "삭제 앞 문단: {md:?}");
        assert_eq!(slice_of(0, 2), "꼬리 문단", "삭제 뒤 문단(재매핑): {md:?}");
        let mid = slice_of(0, 1);
        assert!(
            mid.starts_with('앞') && mid.ends_with('뒤'),
            "가운데 문단이 앞..뒤 범위를 덮어야: {mid:?}"
        );
    }

    /// cleanup_with_map/DeletionMap의 오프셋 재매핑 단위 검증.
    #[test]
    fn cleanup_삭제맵_오프셋_재매핑() {
        // 빈 줄 3개 → 2개 삭제. 정리본은 빈 줄 1개.
        let (cleaned, dels) = cleanup_with_map("a\n\n\n\nb\n");
        assert_eq!(cleaned, "a\n\nb\n");
        let map = DeletionMap::new(&dels);
        assert_eq!(map.remap(0), 0);
        assert_eq!(map.remap(1), 1);
        assert_eq!(map.remap(5), 3, "'b'는 원본 byte 5 → 정리본 byte 3");
        // 삭제 없는 문자열은 항등.
        let (c2, d2) = cleanup_with_map("한\n글\n");
        assert_eq!(c2, "한\n글\n");
        assert!(d2.is_empty());
    }

    /// (5) 각주 정의 줄의 세그먼트가 참조 문단 좌표를 갖는다.
    #[test]
    fn 세그먼트_각주_정의_귀속() {
        let mut doc = from_markdown("첫째 문단\n\n둘째 문단\n");
        // 둘째 문단(인덱스 1)에 각주를 단다.
        let para = &mut doc.sections[0].paragraphs[1];
        let idx = para.controls.len() as u32;
        para.controls.push(note_control(b"fn  ", "각주 정의 내용"));
        para.chars.push(HwpChar::ExtCtrl {
            code: ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id: *b"fn  ",
            payload: vec![],
            ctrl_index: Some(idx),
        });
        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();
        assert!(md.contains("[^1]: 각주 정의 내용"), "각주 정의 방출: {md}");

        // "[^1]:" 로 시작하는 슬라이스를 가진 세그먼트가 참조 문단 (0,1)에 귀속돼야 한다.
        let def_seg = segs
            .iter()
            .find(|s| char_slice(&md, s).starts_with("[^1]:"))
            .expect("각주 정의 세그먼트");
        assert_eq!(
            (def_seg.section, def_seg.para),
            (0, 1),
            "각주 정의는 참조 문단에 귀속: {def_seg:?}"
        );
    }

    /// (6) 구역 2개 문서에서 section 인덱스가 정확하다.
    #[test]
    fn 세그먼트_다중_구역() {
        let mut doc = from_markdown("첫 구역 내용\n");
        let mut second = doc.sections[0].clone();
        second.paragraphs[0].char_shape_runs = vec![];
        second.paragraphs[0].chars = "둘째 구역 내용".chars().map(HwpChar::Text).collect();
        doc.sections.push(second);

        let (md, segs) = to_markdown_with_segments(&doc, &MarkdownOptions::default()).unwrap();
        let first = segs
            .iter()
            .find(|s| char_slice(&md, s) == "첫 구역 내용")
            .expect("첫 구역 세그먼트");
        let sec2 = segs
            .iter()
            .find(|s| char_slice(&md, s) == "둘째 구역 내용")
            .expect("둘째 구역 세그먼트");
        assert_eq!(first.section, 0, "첫 구역 section: {first:?}");
        assert_eq!(sec2.section, 1, "둘째 구역 section: {sec2:?}");
    }

    /// (7) 표 행 한 줄 불변식(단위): HTML 표는 줄마다 `<tr` 수 == `</tr>` 수(중첩 표가 한
    /// 줄에 여럿 있어도 성립), GFM 표는 trim 후 '|' 시작 줄이 '|'로 끝난다.
    #[test]
    fn 표_행_한줄_불변식_단위() {
        // 중첩 표: 바깥 셀 안에 1×1 표 → 바깥 행과 같은 한 줄에 인라인 직렬화.
        let inner = one_cell_table(text_para("속"), 1, 1);
        let mut cell_para = Paragraph::default();
        insert_table(&mut cell_para, inner);
        let outer = one_cell_table(cell_para, 1, 1);
        let mut doc = from_markdown("표\n");
        insert_table(&mut doc.sections[0].paragraphs[0], outer);
        let md = to_markdown(&doc);
        assert!(
            md.contains("<table><tr><td>속</td></tr></table>"),
            "중첩 표가 한 줄로 인라인: {md}"
        );
        for line in md.lines() {
            assert_eq!(
                line.matches("<tr").count(),
                line.matches("</tr>").count(),
                "행 한 줄 불변식 위반: {line:?}"
            );
        }

        // GFM 표: 병합 없는 표는 파이프 표로 렌더 — '|' 시작 줄은 '|'로 끝난다.
        let gfm = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let md2 = to_markdown(&gfm);
        let mut pipe_rows = 0;
        for line in md2.lines() {
            let t = line.trim();
            if t.starts_with('|') {
                pipe_rows += 1;
                assert!(
                    t.ends_with('|'),
                    "파이프 행이 '|'로 끝나야: {line:?} in {md2}"
                );
            }
        }
        assert!(pipe_rows >= 2, "파이프 표 행이 있어야: {md2}");
    }

    fn equation_control(script: &str, inline: bool) -> Control {
        Control::Generic(GenericControl {
            ctrl_id: *b"eqed",
            data: vec![],
            paragraph_lists: vec![],
            extras: vec![],
            raw_children: vec![],
            gso_shapes: vec![],
            equation: Some(Equation {
                script: script.to_string(),
                width: 0,
                height: 0,
                inline,
                x: 0,
                y: 0,
                ..Equation::default()
            }),
            column_def: None,
        })
    }

    fn control_anchor(index: u32, id: &[u8; 4]) -> HwpChar {
        HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *id,
            payload: vec![],
            ctrl_index: Some(index),
        }
    }

    fn insert_table(paragraph: &mut Paragraph, table: Table) {
        let index = paragraph.controls.len() as u32;
        paragraph.controls.push(Control::Table(table));
        paragraph.chars.push(control_anchor(index, b"tbl "));
    }

    fn one_cell_table(paragraph: Paragraph, cols: u16, col_span: u16) -> Table {
        use hwp_model::{BorderFillId, HwpUnit};
        Table {
            common_data: vec![],
            placement: None,
            attr: 0,
            rows: 1,
            cols,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            cells: vec![Cell {
                list_attr: 0,
                col: 0,
                row: 0,
                col_span,
                row_span: 1,
                width: HwpUnit(0),
                height: HwpUnit(0),
                margins: [0; 4],
                border_fill: BorderFillId(0),
                header_tail: vec![],
                paragraphs: vec![paragraph],
            }],
            extras: vec![],
        }
    }

    fn png_bytes() -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]);
        png
    }

    fn write_temp(name: &str, data: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    fn unique_dir(stem: &str) -> std::path::PathBuf {
        let uniq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{stem}_{uniq}"))
    }
}
