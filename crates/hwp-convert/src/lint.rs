//! Official-document lint engine (표기법·구조 규칙 검사).
//!
//! Pure, IO-free module colocated with `official.rs` (the official-document policy
//! home). Markdown input is parsed with the pulldown-cmark AST (D-01): fenced code
//! blocks, GFM tables and HTML blocks are skipped structurally via event depth, not
//! line heuristics. Both lint profiles run the same rule table in v1 (D-04); the
//! profile enum exists so calibration can diverge later without breaking the CLI.
//!
//! Rule table (ROADMAP SC1 + D-05): `notation-date`, `notation-time`,
//! `notation-money`, `notation-attach-colon`, `notation-attach-number`,
//! `notation-end-dot`, `notation-punctuation` and `ai-style-marks` at warning
//! severity; `struct-item-mark` and `struct-roman-heading` are the only two
//! error-severity rules.
//!
//! Detection runs per text block (paragraph or heading): the block's Text events
//! are concatenated with a source-offset map, so rules can anchor at line starts
//! and still report exact positions. Findings carry rule id, severity, 1-based
//! line and character-based column, and an advisory Korean message only — never
//! source-text excerpts (T-02.3-04).

use std::ops::Range;
use std::sync::LazyLock;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use regex::Regex;

/// A lint profile (D-03). Both variants run the same rule table in v1 (D-04).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LintProfile {
    /// Statutory 공문서 rules (default).
    #[default]
    Gongmun,
    /// 보고서 conventions (reserved for later calibration).
    Report,
}

impl LintProfile {
    /// Stable canonical CLI/report name (echoed into the JSON report per D-04).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gongmun => "gongmun",
            Self::Report => "report",
        }
    }
}

/// Finding severity (D-05). Only `Error` affects the `--strict` exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    /// Lowercase report spelling (D-06).
    pub const fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// One lint finding. `line` is 1-based; `col` is character-based (Korean is 3
/// bytes/char in UTF-8, so byte columns would drift).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule_id: &'static str,
    pub severity: Severity,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

// Rule IDs (kebab-case, stable — echoed into the hwp-lint-report-v1 JSON report).
const RULE_NOTATION_DATE: &str = "notation-date";
const RULE_NOTATION_TIME: &str = "notation-time";
const RULE_NOTATION_MONEY: &str = "notation-money";
const RULE_NOTATION_ATTACH_COLON: &str = "notation-attach-colon";
const RULE_NOTATION_ATTACH_NUMBER: &str = "notation-attach-number";
const RULE_NOTATION_END_DOT: &str = "notation-end-dot";
const RULE_NOTATION_PUNCTUATION: &str = "notation-punctuation";
const RULE_AI_STYLE_MARKS: &str = "ai-style-marks";
const RULE_STRUCT_ITEM_MARK: &str = "struct-item-mark";
const RULE_STRUCT_ROMAN_HEADING: &str = "struct-roman-heading";

/// D-05 severity split: exactly the two structure rules are error-severity;
/// every notation rule and ai-style-marks is a warning.
fn severity_of(rule_id: &str) -> Severity {
    match rule_id {
        RULE_STRUCT_ITEM_MARK | RULE_STRUCT_ROMAN_HEADING => Severity::Error,
        _ => Severity::Warning,
    }
}

/// Candidate date span: `YYYY. M. D.` with optional spaces and optional final
/// period. The canonical form is `YYYY. M. D.` (space after each period, mandatory
/// final period, no leading zeros) per the 2020 행정업무운영 편람 표기법
/// (skills/hwp/references/korean-official-format.md §6 날짜). A 4-digit year is
/// required so `제3.01.호` and `v2.05.1` can never match.
static DATE_CANDIDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})\. ?(\d{1,2})\. ?(\d{1,2})\.?").unwrap());

/// Bounded bare-URL mask — the single sanctioned textual exception to D-01's
/// structural-only framing (adopted research Open Question 5). pulldown-cmark
/// 0.13.4 does not GFM-autolink bare URLs, so they arrive as plain Text events
/// and would otherwise fire the notation rules (`…/2026.8.20/…` dates,
/// `http://…:8080` colons). Inline links and `<…>` autolinks are structurally
/// safe (the destination is never a Text event; autolink text is skipped in the
/// walk). `\S+` keeps the scan linear (T-02.3-03).
static URL_MASK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:https?://|www\.)\S+").unwrap());

/// `오전/오후 N시( N분)?` — the meridiem form must be rewritten as 24-hour
/// `HH:MM` (§6 시각).
static TIME_MERIDIEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:오전|오후)\s*\d{1,2}\s*시(?:\s*\d{1,2}\s*분)?").unwrap());
/// `H:MM` candidate; the leading zero is mandatory (`08:09`), so only a bare
/// single-digit hour fires. Two-digit hours are filtered after the match.
static TIME_HM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{1,2}):(\d{2})").unwrap());

/// `345천원`-style abstract thousand-units (§6 금액).
static MONEY_ABBR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+천원").unwrap());
/// `금NNN,NNN원` candidate; silent only when the alteration-proof Korean reading
/// `(금…원)` follows immediately (§6 금액).
static MONEY_GEUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"금\d[\d,]*원").unwrap());

/// Line whose first token is `붙임` followed by a colon (must be `붙임∨∨`, §6
/// 붙임). Anchored at line starts so prose mentions of 붙임 never fire.
static ATTACH_COLON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^붙임[ \t]*:").unwrap());
/// Paragraph opening an attachment block: first token `붙임` with no colon.
static ATTACH_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^붙임(?:\s|$)").unwrap());
/// Attachment quantity candidate; silent only when closed by a period (`1부.`).
static ATTACH_QUANTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+부").unwrap());
/// The `끝` end-mark candidate; followed by `.` it is the canonical `끝.`.
static END_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new("끝").unwrap());
/// 공문서 markers that scope notation-end-dot (adopted research A5): a 붙임 line
/// or a 수신/시행 line. Plain markdown notes never match, so they stay silent.
static GONGMUN_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:붙임|수신|시행)(?:[\s:]|$)").unwrap());
/// 쌍점 candidate: a colon directly after a Hangul word. The rule requires one
/// space after the colon (`원장: 김갑동`), so a non-space follower fires.
static PUNCT_COLON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[가-힣]:").unwrap());
/// Minimal line-leading decorative set (adopted research A1). `□ `/`○ `
/// paragraph starts are the sanctioned ladder (markdown contract §1,
/// skills/hwp/templates/minutes.md) and `★` is statutory before 직위
/// (시행규칙 제6조제1항), so neither is in the set.
static AI_STYLE_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[■▶▲◆●※]").unwrap());
/// Hand-typed item marks at a line start (adopted research A2): statutory
/// ladder marks (`가.`, `1)`, `가)`, `(1)`, `(가)`, circled digits/letters) that
/// must come from nested-list depth, and the `-`/`·`/`ㆍ` rung symbols. The
/// Hangul class is the exact 14-letter ladder so prose like `밥. 먹자` stays
/// silent. Literal `□ `/`○ ` is the sanctioned ladder and is never in the set.
static ITEM_MARK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^(?:[가나다라마바사아자차카타파하][.)]|\d{1,2}\)|\(\d{1,2}\)|\([가나다라마바사아자차카타파하]\)|[①-⑳㉑-㉟㉮-㉻]|[-·ㆍ])(?:\s|$)",
    )
    .unwrap()
});
/// ASCII roman-numeral heading start (`I.`, `II.`, … `XII.`). The full-width
/// forms `Ⅰ`-`Ⅻ` (U+2160…) are the correct form and never match. Longer
/// numerals come first so alternation preference cannot truncate a match.
static ROMAN_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:VIII|XII|VII|III|XI|IX|VI|IV|II|X|V|I)\.").unwrap());

/// Advisory messages — cite the rule source with its confidence tag, never
/// statutory-violation wording beyond the tag and never a source-text excerpt
/// (T-02.3-04).
const NOTATION_DATE_MESSAGE: &str = "날짜는 `2026. 6. 19.`처럼 표기합니다 (마침표 뒤 한 칸, 마지막 마침표 필수, 앞자리 0 제거) — 행정업무운영 편람 표기법";
const NOTATION_TIME_MESSAGE: &str = "시각은 `15:20`처럼 24시간제 `HH:MM`으로 표기합니다 (한 자리 시간도 `08:09`처럼 0을 채웁니다) — 행정업무운영 편람 표기법";
const NOTATION_MONEY_ABBR_MESSAGE: &str = "`345천원` 같은 천 단위 표기는 `345,000원` 또는 `34만 5천 원`처럼 표기합니다 — 행정업무운영 편람 표기법";
const NOTATION_MONEY_READING_MESSAGE: &str = "금액은 `금113,560원(금일십일만삼천오백육십원)`처럼 한글 금액을 괄호 안에 병기합니다 — 행정업무운영 편람 표기법";
const NOTATION_ATTACH_COLON_MESSAGE: &str =
    "`붙임` 뒤에는 쌍점(:)을 붙이지 않고 두 칸을 띄워 씁니다 — 행정업무운영 편람 표기법";
const NOTATION_ATTACH_NUMBER_MESSAGE: &str =
    "붙임이 하나일 때는 번호 `1.`을 붙이지 않습니다 — 행정업무운영 편람 표기법";
const NOTATION_ATTACH_QUANTITY_MESSAGE: &str =
    "붙임 수량은 `1부.`처럼 마침표로 닫습니다 — 행정업무운영 편람 표기법";
const NOTATION_END_DOT_MESSAGE: &str =
    "공문서 끝에는 `끝.`처럼 마침표를 붙입니다 — 행정업무운영 편람 표기법";
const NOTATION_PUNCT_TILDE_MESSAGE: &str =
    "범위에는 키보드 `~` 대신 물결표 `∼`를 사용합니다 — 행정업무운영 편람 표기법";
const NOTATION_PUNCT_COLON_MESSAGE: &str =
    "쌍점(:)은 앞말에 붙이고 뒤에 한 칸을 띄웁니다 (`원장: 김갑동`) — 행정업무운영 편람 표기법";
const AI_STYLE_MARKS_MESSAGE: &str =
    "장식 기호(■ ▶ ▲ ◆ ● ※)로 항목을 시작하지 말고 공문서 항목 기호(□ ○)나 중첩 목록을 사용합니다";
const STRUCT_ITEM_MARK_MESSAGE: &str = "항목 부호(`가.`, `(1)`, `①` 등)는 직접 입력하지 말고 중첩 목록 들여쓰기로 표현합니다 — 공문서 마크다운 계약";
const STRUCT_ITEM_MARK_DOUBLE_MESSAGE: &str = "목록 항목 앞에 부호를 직접 입력하면 엔진이 부여하는 부호와 겹칩니다 (직접 입력한 부호를 지웁니다) — 공문서 마크다운 계약";
const STRUCT_ROMAN_HEADING_MESSAGE: &str =
    "머리글 로마 숫자는 ASCII `I.` 대신 전각 `Ⅰ.`(U+2160)을 사용합니다 — 공문서 마크다운 계약";

/// One linted text block: a paragraph or heading at structural depth 0, with
/// its Text events concatenated (`text`), a per-segment source-offset map
/// (`segs`: concat byte offset → source byte offset) and the masked URL spans
/// (`masked`, concat byte ranges) that notation rules must skip.
struct Para {
    text: String,
    segs: Vec<(usize, usize)>,
    masked: Vec<Range<usize>>,
    in_heading: bool,
    /// Inside a list item — drives the double-mark guard's message.
    in_item: bool,
    /// The block's first Text event arrived inside a Strong span — records the
    /// bold-wrapped case behind the A3 exactly-one-finding guard (see
    /// `lint_struct_item_mark`).
    first_text_in_strong: bool,
    /// Inside a 붙임 block: the 붙임 paragraph itself or an item of the
    /// attachment list that immediately follows it.
    attach_entry: bool,
}

impl Para {
    fn new(in_heading: bool, in_item: bool) -> Self {
        Self {
            text: String::new(),
            segs: Vec::new(),
            masked: Vec::new(),
            in_heading,
            in_item,
            first_text_in_strong: false,
            attach_entry: false,
        }
    }

    /// Append one Text event, masking bare-URL spans (the bounded D-01
    /// exception — see `URL_MASK`).
    fn push_text(&mut self, text: &str, source_start: usize, in_strong: bool) {
        if self.segs.is_empty() {
            self.first_text_in_strong = in_strong;
        }
        let base = self.text.len();
        for m in URL_MASK.find_iter(text) {
            self.masked.push((base + m.start())..(base + m.end()));
        }
        self.segs.push((base, source_start));
        self.text.push_str(text);
    }

    /// Append a synthetic line break for a soft/hard break so rules can anchor
    /// at line starts within a paragraph.
    fn push_break(&mut self, source_start: usize) {
        self.segs.push((self.text.len(), source_start));
        self.text.push('\n');
    }

    /// Map a concat byte offset back to a source byte offset.
    fn source_offset(&self, concat_idx: usize) -> usize {
        let i = self
            .segs
            .partition_point(|&(concat_start, _)| concat_start <= concat_idx);
        let (concat_start, source_start) = self.segs[i - 1];
        source_start + (concat_idx - concat_start)
    }

    /// True when the `[start, end)` concat span intersects a masked URL span.
    fn masked_overlap(&self, start: usize, end: usize) -> bool {
        self.masked.iter().any(|m| m.start < end && start < m.end)
    }
}

/// Lint markdown text and return findings in ascending source order.
///
/// The profile is recorded by callers (report echo, D-04) but selects no rules in
/// v1 — both profiles share one rule table.
pub fn lint_markdown(markdown: &str, profile: LintProfile) -> Vec<Finding> {
    let _ = profile; // Same rule table for every profile in v1 (D-04).

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    // Parses strikethrough (`~~`) and footnotes (`[^N]`). Task lists (TASKLISTS) are excluded — no corresponding IR meaning.
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);

    // 1-based line index: byte offset of each line start, for binary search.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(markdown.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let locate = |offset: usize| -> (u32, u32) {
        let idx = line_starts.partition_point(|&start| start <= offset);
        let line_start = line_starts[idx - 1];
        let col = markdown[line_start..offset].chars().count() as u32 + 1;
        (idx as u32, col)
    };

    let (paras, single_attach_lists) = collect_paras(markdown, options);

    // notation-end-dot scope (adopted research A5): fire only in documents
    // carrying a 공문서 marker (a 붙임 line or a 수신/시행 line) so plain
    // markdown notes stay silent.
    let has_gongmun_marker = paras.iter().any(|p| is_gongmun_marker(&p.text));

    let mut findings = Vec::new();
    for para in &paras {
        if para.text.is_empty() {
            continue;
        }
        lint_notation_date(para, &locate, &mut findings);
        lint_notation_time(para, &locate, &mut findings);
        lint_notation_money(para, &locate, &mut findings);
        lint_notation_punctuation(para, &locate, &mut findings);
        if has_gongmun_marker {
            lint_notation_end_dot(para, &locate, &mut findings);
        }
        if !para.in_heading {
            lint_notation_attach_colon(para, &locate, &mut findings);
            lint_notation_attach_number(para, &locate, &mut findings);
            lint_ai_style_marks(para, &locate, &mut findings);
            lint_struct_item_mark(para, &locate, &mut findings);
        }
        lint_struct_roman_heading(para, &locate, &mut findings);
        if para.attach_entry {
            lint_attach_quantity(para, &locate, &mut findings);
        }
    }
    // A single-item ordered list right after a 붙임 paragraph is the typed `1.`
    // number the rule forbids for a lone attachment.
    for idx in single_attach_lists {
        let para = &paras[idx];
        if para.text.is_empty() {
            // An empty first block (e.g. an image-only paragraph) carries no
            // typed `1.` text position — skip it like the main rule loop does.
            continue;
        }
        push(
            &mut findings,
            RULE_NOTATION_ATTACH_NUMBER,
            &locate,
            para.source_offset(0),
            NOTATION_ATTACH_NUMBER_MESSAGE,
        );
    }
    // D-07: ascending source order regardless of rule evaluation order.
    findings.sort_by_key(|f| (f.line, f.col));
    findings
}

/// Walk the pulldown-cmark AST and collect the lintable text blocks plus the
/// paragraph indices of single-item ordered attachment lists (the typed `1.`
/// number forbidden for a lone attachment).
fn collect_paras(markdown: &str, options: Options) -> (Vec<Para>, Vec<usize>) {
    let mut paras: Vec<Para> = Vec::new();
    let mut single_attach_lists: Vec<usize> = Vec::new();
    // Structural skip depth (D-01): Text inside code blocks, tables and HTML
    // blocks is never linted; InlineHtml events never reach a block's text.
    let mut skip_depth: u32 = 0;
    let mut item_depth: u32 = 0;
    let mut list_depth: u32 = 0;
    // `<…>` autolinks and `<addr>` email autolinks carry their URL as the link
    // text; that text is never prose, so it is skipped structurally.
    let mut autolink_depth: u32 = 0;
    let mut link_stack: Vec<bool> = Vec::new();
    // Image alt text is an attribute of an embedded resource, not document
    // prose (D-01); it arrives as ordinary Text inside the Image span and is
    // skipped structurally like autolink text.
    let mut image_depth: u32 = 0;
    // Strong-span depth — records whether a block's first text is bold-wrapped
    // (the A3 bold-x2 guard; detection itself stays paragraph-level).
    let mut strong_depth: u32 = 0;
    let mut cur: Option<Para> = None;
    // 붙임 block tracking: a paragraph opening with the `붙임` token starts an
    // attachment block; a list immediately following it is the attachment
    // ladder and its items are linted for quantity periods and counted for the
    // single-attachment number rule.
    let mut pending_attach = false;
    let mut attach_list = false;
    let mut attach_list_depth: u32 = 0;
    let mut attach_item_level: u32 = 0;
    let mut attach_item_count: u32 = 0;
    let mut attach_list_ordered = false;
    let mut expect_first_attach_para = false;
    let mut first_attach_para: Option<usize> = None;
    // Tight list items carry their Text directly under the Item tag with no
    // Paragraph wrapper; that text is collected in this implicit block and
    // flushed at the next block boundary.
    let mut implicit: Option<Para> = None;

    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::Table(_) | Tag::HtmlBlock) => {
                flush_implicit(&mut implicit, &mut paras, &mut pending_attach);
                skip_depth += 1;
                pending_attach = false;
            }
            Event::End(TagEnd::CodeBlock | TagEnd::Table | TagEnd::HtmlBlock) => {
                skip_depth = skip_depth.saturating_sub(1);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush_implicit(&mut implicit, &mut paras, &mut pending_attach);
                pending_attach = false;
            }
            Event::Start(Tag::Heading { .. }) => {
                flush_implicit(&mut implicit, &mut paras, &mut pending_attach);
                pending_attach = false;
                if skip_depth == 0 {
                    cur = Some(Para::new(true, item_depth > 0));
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(para) = cur.take() {
                    paras.push(para);
                }
            }
            Event::Start(Tag::List(start)) => {
                flush_implicit(&mut implicit, &mut paras, &mut pending_attach);
                list_depth += 1;
                if pending_attach {
                    attach_list = true;
                    attach_list_depth = list_depth;
                    attach_item_level = item_depth + 1;
                    attach_item_count = 0;
                    attach_list_ordered = start.is_some();
                    first_attach_para = None;
                    pending_attach = false;
                }
            }
            Event::End(TagEnd::List(_)) => {
                if attach_list && list_depth == attach_list_depth {
                    if attach_list_ordered
                        && attach_item_count == 1
                        && let Some(idx) = first_attach_para
                    {
                        single_attach_lists.push(idx);
                    }
                    attach_list = false;
                }
                list_depth = list_depth.saturating_sub(1);
            }
            Event::Start(Tag::Item) => {
                flush_implicit(&mut implicit, &mut paras, &mut pending_attach);
                item_depth += 1;
                if attach_list && item_depth == attach_item_level {
                    attach_item_count += 1;
                    expect_first_attach_para = attach_item_count == 1;
                }
            }
            Event::End(TagEnd::Item) => {
                flush_implicit(&mut implicit, &mut paras, &mut pending_attach);
                item_depth = item_depth.saturating_sub(1);
            }
            Event::Start(Tag::Paragraph) => {
                flush_implicit(&mut implicit, &mut paras, &mut pending_attach);
                // A non-list block after the 붙임 paragraph closes the block.
                pending_attach = false;
                if skip_depth == 0 {
                    let mut para = Para::new(false, item_depth > 0);
                    if attach_list && item_depth == attach_item_level {
                        para.attach_entry = true;
                        if expect_first_attach_para {
                            first_attach_para = Some(paras.len());
                            expect_first_attach_para = false;
                        }
                    }
                    cur = Some(para);
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if let Some(mut para) = cur.take() {
                    if is_attach_start(&para.text) {
                        para.attach_entry = true;
                        pending_attach = true;
                    }
                    paras.push(para);
                }
            }
            Event::Start(Tag::Strong) => {
                strong_depth += 1;
            }
            Event::End(TagEnd::Strong) => {
                strong_depth = strong_depth.saturating_sub(1);
            }
            Event::Start(Tag::Link { link_type, .. }) => {
                let autolink = matches!(link_type, LinkType::Autolink | LinkType::Email);
                if autolink {
                    autolink_depth += 1;
                }
                link_stack.push(autolink);
            }
            Event::End(TagEnd::Link) => {
                if link_stack.pop().unwrap_or(false) {
                    autolink_depth = autolink_depth.saturating_sub(1);
                }
            }
            Event::Start(Tag::Image { .. }) => {
                image_depth += 1;
            }
            Event::End(TagEnd::Image) => {
                image_depth = image_depth.saturating_sub(1);
            }
            Event::Text(text) => {
                if skip_depth == 0 && autolink_depth == 0 && image_depth == 0 {
                    // Tight list items carry their Text directly under the Item
                    // (no Paragraph wrapper) — collect it in an implicit block.
                    if cur.is_none() && item_depth > 0 && implicit.is_none() {
                        let mut para = Para::new(false, true);
                        if attach_list && item_depth == attach_item_level {
                            para.attach_entry = true;
                            if expect_first_attach_para {
                                first_attach_para = Some(paras.len());
                                expect_first_attach_para = false;
                            }
                        }
                        implicit = Some(para);
                    }
                    if let Some(para) = cur.as_mut().or(implicit.as_mut()) {
                        para.push_text(&text, range.start, strong_depth > 0);
                    }
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if skip_depth == 0
                    && let Some(para) = cur.as_mut().or(implicit.as_mut())
                {
                    para.push_break(range.start);
                }
            }
            _ => {}
        }
    }
    (paras, single_attach_lists)
}

/// Push a collected tight-list-item block, applying the same 붙임-block
/// classification as a real paragraph end.
fn flush_implicit(implicit: &mut Option<Para>, paras: &mut Vec<Para>, pending_attach: &mut bool) {
    if let Some(mut para) = implicit.take() {
        if is_attach_start(&para.text) {
            para.attach_entry = true;
            *pending_attach = true;
        }
        paras.push(para);
    }
}

/// Paragraph opening an attachment block (first token `붙임`, no colon).
fn is_attach_start(text: &str) -> bool {
    ATTACH_START.is_match(text)
}
/// Document-level 공문서 marker scoping notation-end-dot (adopted A5).
fn is_gongmun_marker(text: &str) -> bool {
    GONGMUN_MARKER.is_match(text)
}

fn is_hangul_syllable(c: char) -> bool {
    ('가'..='힣').contains(&c)
}

/// Append one finding, deriving severity from the rule id (D-05).
fn push(
    findings: &mut Vec<Finding>,
    rule_id: &'static str,
    locate: &impl Fn(usize) -> (u32, u32),
    source_offset: usize,
    message: &str,
) {
    let (line, col) = locate(source_offset);
    findings.push(Finding {
        rule_id,
        severity: severity_of(rule_id),
        line,
        col,
        message: message.to_string(),
    });
}

/// `notation-date` (warning): flag any date-like span that deviates from the
/// canonical `YYYY. M. D.` form — missing spaces (`2020.7.8`), a missing final
/// period (`2026. 8. 20`), or leading zeros (`1985.09.06.`).
fn lint_notation_date(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    let text = &para.text;
    for caps in DATE_CANDIDATE.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        if para.masked_overlap(whole.start(), whole.end()) {
            continue;
        }
        // Guard: a digit on either side means a longer numeric run (e.g. the
        // `12026. 8. 20.` prefix or the `2020.7.8.1` 4-part version), not a date.
        let before = text[..whole.start()].bytes().next_back();
        let after = text[whole.end()..].bytes().next();
        if before.is_some_and(|b| b.is_ascii_digit()) || after.is_some_and(|b| b.is_ascii_digit()) {
            continue;
        }
        let year: u32 = caps[1].parse().unwrap();
        let month: u32 = caps[2].parse().unwrap();
        let day: u32 = caps[3].parse().unwrap();
        let canonical = format!("{year}. {month}. {day}.");
        if whole.as_str() == canonical {
            continue;
        }
        push(
            findings,
            RULE_NOTATION_DATE,
            locate,
            para.source_offset(whole.start()),
            NOTATION_DATE_MESSAGE,
        );
    }
}

/// `notation-time` (warning): meridiem forms (`오후 3시 20분`) and bare
/// single-digit hours (`8:09`) must be rewritten as 24-hour `HH:MM` (`15:20`,
/// `08:09`).
fn lint_notation_time(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    let text = &para.text;
    for m in TIME_MERIDIEM.find_iter(text) {
        if para.masked_overlap(m.start(), m.end()) {
            continue;
        }
        push(
            findings,
            RULE_NOTATION_TIME,
            locate,
            para.source_offset(m.start()),
            NOTATION_TIME_MESSAGE,
        );
    }
    for caps in TIME_HM.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        // Canonical `HH:MM` keeps the leading zero; only a bare single-digit
        // hour violates the form.
        if caps[1].len() != 1 || para.masked_overlap(whole.start(), whole.end()) {
            continue;
        }
        // Guard: digits or colons on either side mean a longer numeric/time
        // run, not an `H:MM` time.
        let before = text[..whole.start()].bytes().next_back();
        let after = text[whole.end()..].bytes().next();
        if before.is_some_and(|b| b.is_ascii_digit() || b == b':')
            || after.is_some_and(|b| b.is_ascii_digit() || b == b':')
        {
            continue;
        }
        push(
            findings,
            RULE_NOTATION_TIME,
            locate,
            para.source_offset(whole.start()),
            NOTATION_TIME_MESSAGE,
        );
    }
}

/// `notation-money` (warning): abstract thousand-units (`345천원` → `345,000원`
/// or `34만 5천 원`) and `금NNN,NNN원` without the alteration-proof Korean
/// reading `(금…원)` in parentheses.
fn lint_notation_money(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    let text = &para.text;
    for m in MONEY_ABBR.find_iter(text) {
        if para.masked_overlap(m.start(), m.end()) {
            continue;
        }
        push(
            findings,
            RULE_NOTATION_MONEY,
            locate,
            para.source_offset(m.start()),
            NOTATION_MONEY_ABBR_MESSAGE,
        );
    }
    for m in MONEY_GEUM.find_iter(text) {
        if para.masked_overlap(m.start(), m.end()) {
            continue;
        }
        // Silent only when the Korean reading follows immediately.
        if text[m.end()..].starts_with("(금") {
            continue;
        }
        push(
            findings,
            RULE_NOTATION_MONEY,
            locate,
            para.source_offset(m.start()),
            NOTATION_MONEY_READING_MESSAGE,
        );
    }
}

/// `notation-attach-colon` (warning): a line whose first token is `붙임` must
/// not carry a colon. Line-start anchoring keeps prose mentions of 붙임 silent.
fn lint_notation_attach_colon(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    for m in ATTACH_COLON.find_iter(&para.text) {
        push(
            findings,
            RULE_NOTATION_ATTACH_COLON,
            locate,
            para.source_offset(m.start()),
            NOTATION_ATTACH_COLON_MESSAGE,
        );
    }
}

/// `notation-attach-number` (warning), paragraph form: a 붙임 block with
/// exactly one entry must not carry the `1.` number (two or more keep the
/// `1. 2.` ladder). Conservative by design — only the unambiguous single-entry
/// case fires. The list form is detected in `collect_paras`.
fn lint_notation_attach_number(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    if !is_attach_start(&para.text) {
        return;
    }
    let mut lines = para.text.split('\n');
    let first = lines.next().unwrap_or_default();
    let rest = first["붙임".len()..].trim();
    let mut entries = vec![rest];
    for line in lines {
        let t = line.trim();
        if !t.is_empty() {
            entries.push(t);
        }
    }
    entries.retain(|e| !e.is_empty());
    if entries.len() == 1
        && entries[0].starts_with("1.")
        && let Some(idx) = para.text.find("1.")
    {
        push(
            findings,
            RULE_NOTATION_ATTACH_NUMBER,
            locate,
            para.source_offset(idx),
            NOTATION_ATTACH_NUMBER_MESSAGE,
        );
    }
}

/// `notation-attach-number` (warning), quantity form: inside a 붙임 block the
/// quantity must be closed by a period (`1부.`).
fn lint_attach_quantity(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    for m in ATTACH_QUANTITY.find_iter(&para.text) {
        if para.masked_overlap(m.start(), m.end()) {
            continue;
        }
        if para.text[m.end()..].starts_with('.') {
            continue;
        }
        push(
            findings,
            RULE_NOTATION_ATTACH_NUMBER,
            locate,
            para.source_offset(m.start()),
            NOTATION_ATTACH_QUANTITY_MESSAGE,
        );
    }
}

/// `notation-end-dot` (warning): a `끝` end mark without its period. Runs only
/// in documents carrying a 공문서 marker (adopted research A5).
fn lint_notation_end_dot(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    for m in END_MARK.find_iter(&para.text) {
        if para.masked_overlap(m.start(), m.end()) {
            continue;
        }
        match para.text[m.end()..].chars().next() {
            // Canonical `끝.`.
            Some('.') => {}
            // Part of a longer word (끝내다, 끝까지) — not the end mark.
            Some(c) if is_hangul_syllable(c) => {}
            _ => push(
                findings,
                RULE_NOTATION_END_DOT,
                locate,
                para.source_offset(m.start()),
                NOTATION_END_DOT_MESSAGE,
            ),
        }
    }
}

/// `notation-punctuation` (warning): keyboard `~` inside a date/number range
/// (the 물결표 `∼` is correct) and a 쌍점 without the required following space
/// (`원장:김갑동` → `원장: 김갑동`).
fn lint_notation_punctuation(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    let text = &para.text;
    for (idx, _) in text.match_indices('~') {
        if para.masked_overlap(idx, idx + 1) {
            continue;
        }
        // Fire only inside a date/number range — a digit or period on both
        // sides. Strikethrough markers never reach Text events, and `~` with
        // spaces or at a boundary is not a range.
        let range_edge = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit() || c == '.');
        let prev = text[..idx].chars().next_back();
        let next = text[idx + 1..].chars().next();
        if range_edge(prev) && range_edge(next) {
            push(
                findings,
                RULE_NOTATION_PUNCTUATION,
                locate,
                para.source_offset(idx),
                NOTATION_PUNCT_TILDE_MESSAGE,
            );
        }
    }
    for m in PUNCT_COLON.find_iter(text) {
        if para.masked_overlap(m.start(), m.end()) {
            continue;
        }
        // 쌍점: attached to the preceding word, one space after. A Hangul-word
        // colon directly followed by a non-space character fires; ASCII
        // scheme-like colons (`http:`) never match the Hangul anchor.
        if text[m.end()..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace())
        {
            push(
                findings,
                RULE_NOTATION_PUNCTUATION,
                locate,
                para.source_offset(m.start()),
                NOTATION_PUNCT_COLON_MESSAGE,
            );
        }
    }
}

/// `ai-style-marks` (warning): paragraph/item text starting with a decorative
/// line-leading symbol (■ ▶ ▲ ◆ ● ※) — the AI-generation tell. The sanctioned
/// `□ `/`○ ` ladder and the statutory `★` are not in the set.
fn lint_ai_style_marks(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    for m in AI_STYLE_MARK.find_iter(&para.text) {
        push(
            findings,
            RULE_AI_STYLE_MARKS,
            locate,
            para.source_offset(m.start()),
            AI_STYLE_MARKS_MESSAGE,
        );
    }
}

/// `struct-item-mark` (error, D-05): hand-typed item marks where the markdown
/// contract requires nested-list depth — statutory ladder marks (`가.`, `1)`,
/// `가)`, `(1)`, `(가)`, circled digits/letters) and the `-`/`·`/`ㆍ` rung
/// symbols at a line start, plus any rung mark at the start of a list item's
/// own text (the double-mark case). Paragraph-initial literal `□ `/`○ ` is the
/// sanctioned ladder (contract §1, minutes.md) and is never flagged.
fn lint_struct_item_mark(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    // Adopted research A3 (the SC1 bold-x2 guard): detection is anchored at
    // line starts and emits once per position, so a Strong-wrapped mark
    // (`**가. 항목**`) yields exactly one finding — never zero (the bold span
    // is not a skip) and never two (no per-span re-emission).
    let _strong_wrapped = para.first_text_in_strong;
    for m in ITEM_MARK.find_iter(&para.text) {
        let message = if para.in_item {
            STRUCT_ITEM_MARK_DOUBLE_MESSAGE
        } else {
            STRUCT_ITEM_MARK_MESSAGE
        };
        push(
            findings,
            RULE_STRUCT_ITEM_MARK,
            locate,
            para.source_offset(m.start()),
            message,
        );
    }
}

/// `struct-roman-heading` (error, D-05): ASCII roman numerals (`I.`, `II.`, …)
/// where the contract requires the full-width forms (`Ⅰ.` U+2160). Inside a
/// `#` heading the numeral alone fires; in a paragraph the numeral must be
/// followed by `. ` + Hangul content so ordinary English sentences
/// ("I. Introduction") stay silent.
fn lint_struct_roman_heading(
    para: &Para,
    locate: &impl Fn(usize) -> (u32, u32),
    findings: &mut Vec<Finding>,
) {
    let Some(m) = ROMAN_START.find(&para.text) else {
        return;
    };
    let after = &para.text[m.end()..];
    let fires = if para.in_heading {
        after.chars().next().is_none_or(|c| c.is_whitespace())
    } else {
        after
            .trim_start()
            .chars()
            .next()
            .is_some_and(is_hangul_syllable)
    };
    if fires {
        push(
            findings,
            RULE_STRUCT_ROMAN_HEADING,
            locate,
            para.source_offset(m.start()),
            STRUCT_ROMAN_HEADING_MESSAGE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint(md: &str) -> Vec<Finding> {
        lint_markdown(md, LintProfile::Gongmun)
    }

    #[test]
    fn notation_date_fires_on_each_wrong_form() {
        for (md, why) in [
            ("시행: 2020.7.8\n", "missing spaces and final period"),
            ("시행: 2026. 8. 20\n", "missing final period"),
            ("시행: 1985.09.06.\n", "leading zeros and missing spaces"),
            ("시행: 2020. 7. 8\n", "missing final period only"),
        ] {
            let findings = lint(md);
            assert_eq!(findings.len(), 1, "{why}: {md:?} -> {findings:?}");
            assert_eq!(findings[0].rule_id, "notation-date");
            assert_eq!(findings[0].severity, Severity::Warning);
        }
    }

    #[test]
    fn notation_date_silent_on_correct_forms() {
        for md in [
            "시행: 2020. 7. 8.\n",
            "시행일: 2026. 6. 19.\n",
            "2023. 11. 11.(토) 개최\n",
            "기간: 2026. 4. 23.∼2026. 6. 15.\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_date_never_matches_non_date_numeric_runs() {
        for md in [
            "제3.01.호 참조\n",  // no 4-digit year
            "버전 v2.05.1\n",    // no 4-digit year
            "버전 2020.7.8.1\n", // 4-part version: digit after the span
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn structural_skips_code_table_and_html_blocks() {
        let md = "```\n2020.7.8\n```\n\n| 날짜 |\n|------|\n| 2020.7.8 |\n\n<div>\n2020.7.8\n</div>\n\n시행: 2020.7.8\n";
        let findings = lint(md);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 13);
    }

    #[test]
    fn line_and_col_are_1_based_and_character_based() {
        // Korean text before the date: byte cols would drift (3 bytes/char).
        let md = "본문입니다.\n\n시행: 2020.7.8\n";
        let findings = lint(md);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 3);
        // "시행: " is 4 characters, so the date starts at column 5.
        assert_eq!(findings[0].col, 5);
    }

    #[test]
    fn report_profile_runs_the_same_rule_table_v1() {
        let md = "시행: 2020.7.8\n";
        assert_eq!(
            lint_markdown(md, LintProfile::Gongmun),
            lint_markdown(md, LintProfile::Report)
        );
    }

    // ---- Task 1: notation families + ai-style-marks + bare-URL mask ----

    fn only_rule(md: &str, rule_id: &str) -> Vec<Finding> {
        let findings = lint(md);
        for f in &findings {
            assert_eq!(
                f.rule_id, rule_id,
                "unexpected rule in {md:?}: {findings:?}"
            );
        }
        findings
    }

    #[test]
    fn notation_time_fires_on_meridiem_and_single_digit_hour() {
        let findings = only_rule("일시: 오후 3시 20분\n", "notation-time");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].severity, Severity::Warning);
        let findings = only_rule("회의는 8:09에 시작합니다\n", "notation-time");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_time_silent_on_canonical_24h_forms() {
        for md in ["일시: 15:20\n", "일시: 08:09\n", "15:20∼17:00\n"] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_money_fires_on_abstract_thousand_and_missing_reading() {
        let findings = only_rule("비용: 345천원\n", "notation-money");
        assert_eq!(findings.len(), 1, "{findings:?}");
        let findings = only_rule("금액은 금113,560원이다\n", "notation-money");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_money_silent_on_canonical_amounts() {
        for md in [
            "금113,560원(금일십일만삼천오백육십원)\n",
            "참석 50여 명\n",
            "34만 5천 원\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_attach_colon_fires_on_line_initial_colon() {
        let findings = only_rule("붙임: 계획서 1부.  끝.\n", "notation-attach-colon");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].line, 1);
        assert_eq!(findings[0].col, 1);
    }

    #[test]
    fn notation_attach_colon_silent_in_prose() {
        for md in [
            "자세한 내용은 붙임 파일을 참고하시기 바랍니다.\n",
            "붙임  계획서 1부.  끝.\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_attach_number_fires_on_single_number_and_missing_period() {
        // One attachment must not carry the `1.` number (inline form).
        let findings = only_rule("붙임  1. 계획서 1부.  끝.\n", "notation-attach-number");
        assert_eq!(findings.len(), 1, "{findings:?}");
        // One attachment in list form: the single-item ordered list is the
        // typed `1.` number.
        let findings = only_rule("붙임\n\n1. 계획서 1부.\n", "notation-attach-number");
        assert_eq!(findings.len(), 1, "{findings:?}");
        // Quantity missing the final period.
        let findings = only_rule("붙임  계획서 1부\n", "notation-attach-number");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn single_attach_list_with_empty_first_paragraph_does_not_panic() {
        // CR-1 regression: a loose single-item ordered list after a 붙임
        // paragraph whose first block carries no Text events (an image-only
        // paragraph) used to underflow `Para::source_offset` (exit 101). The
        // empty block carries no typed `1.` position, so it is skipped.
        let md = "붙임\n\n1. ![](x.png)\n\n   추가 설명 문단\n";
        let findings = lint(md);
        assert!(
            findings
                .iter()
                .all(|f| f.rule_id != "notation-attach-number"),
            "{findings:?}"
        );
    }

    #[test]
    fn notation_attach_number_silent_on_ladder_and_canonical_single() {
        for md in [
            // Two or more attachments keep the 1. 2. ladder.
            "붙임  1. 계획서 1부.\n      2. 서류 1부.  끝.\n",
            "붙임\n\n1. 계획서 1부.\n2. 서류 1부.\n",
            // One attachment, unnumbered, quantity closed by a period.
            "붙임  계획서 1부.  끝.\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_end_dot_fires_when_gongmun_marker_present() {
        let findings = only_rule("붙임  계획서 1부.  끝\n", "notation-end-dot");
        assert_eq!(findings.len(), 1, "{findings:?}");
        let findings = only_rule(
            "수신  교육부장관\n\n협조하여 주시기 바랍니다.  끝\n",
            "notation-end-dot",
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_end_dot_silent_without_markers() {
        for md in [
            "메모: 오늘 회의 끝\n",
            "붙임  계획서 1부.  끝.\n",
            "회의가 끝나는 대로 공유\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn notation_punctuation_fires_on_keyboard_tilde_and_tight_colon() {
        let findings = only_rule("기간: 2026. 4. 23.~2026. 6. 15.\n", "notation-punctuation");
        assert_eq!(findings.len(), 1, "{findings:?}");
        let findings = only_rule("원장:김갑동\n", "notation-punctuation");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn notation_punctuation_silent_on_canonical_forms() {
        for md in ["기간: 2026. 4. 23.∼2026. 6. 15.\n", "원장: 김갑동\n"] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn ai_style_marks_fires_on_decorative_leading_symbols() {
        for md in ["■ 사업 개요\n", "▶ 추진 방향\n", "※ 참고 사항\n"] {
            let findings = only_rule(md, "ai-style-marks");
            assert_eq!(findings.len(), 1, "{md:?} -> {findings:?}");
            assert_eq!(findings[0].severity, Severity::Warning);
        }
    }

    #[test]
    fn ai_style_marks_silent_on_sanctioned_marks() {
        for md in [
            "□ 회의 명칭\n\n○ 안건\n",
            "★ 발의자 홍길동\n",
            "- 목록 항목\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn fp_guard_url_bare() {
        // A bare URL is plain Text (pulldown-cmark 0.13.4 has no GFM
        // autolinking); the bounded mask keeps every rule silent on it.
        let md = "참고: https://example.go.kr/2026.8.20/notice 입니다\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        let md = "https://example.go.kr/2026.8.20/notice\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
    }

    #[test]
    fn fp_guard_url_inline() {
        // Inline-link destinations and `<…>` autolinks are structurally safe.
        for md in [
            "[공고문](https://example.go.kr/2026.8.20/notice)\n",
            "<https://example.go.kr/2026.8.20/notice>\n",
        ] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn fp_guard_image_alt_text() {
        // WR-2: image alt text is an attribute of an embedded resource, not
        // document prose — a date-like string in the alt produces no finding
        // (D-01 structural skip, consistent with autolink text).
        let md = "참고 ![2020.7.8 캡처](x.png) 입니다\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
    }

    // ---- Task 2: error-severity structure rules + the five FP-guard pins ----

    #[test]
    fn struct_item_mark_fires_on_typed_ladder_marks() {
        for md in [
            "가. 항목입니다\n",
            // `1)` at line start is an ordered-list marker in CommonMark;
            // the literal mark only reaches text when escaped.
            "1\\) 항목입니다\n",
            "가) 항목입니다\n",
            "(1) 항목입니다\n",
            "(가) 항목입니다\n",
            "① 항목입니다\n",
            "㉮ 항목입니다\n",
            "· 항목입니다\n",
            "ㆍ 항목입니다\n",
            "\\- 항목입니다\n",
        ] {
            let findings = only_rule(md, "struct-item-mark");
            assert_eq!(findings.len(), 1, "{md:?} -> {findings:?}");
            assert_eq!(findings[0].severity, Severity::Error);
        }
    }

    #[test]
    fn struct_item_mark_silent_on_sanctioned_forms() {
        // The literal □/○ ladder is the sanctioned authoring form (markdown
        // contract §1); minutes.md uses it and must stay silent (SC1). Nested
        // list markup carries the numbered path.
        let minutes_style = "{{회의명}} 회의록\n\n작성: {{작성자}}\n\n□ 회의 명칭\n\n○○ 회의\n\n□ 상정 안건\n\n1. 첫째 안건\n2. 둘째 안건\n\n□ 결정 사항\n\n○ 결정 1\n";
        assert!(lint(minutes_style).is_empty(), "{:?}", lint(minutes_style));
        for md in ["□ 회의 명칭\n", "○ 안건\n", "- 목록 항목\n"] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn struct_item_mark_fires_on_double_mark_in_list_item() {
        // A rung mark at the start of a list item's own text double-numbers
        // next to the engine-assigned mark.
        for md in [
            "- 가. 항목입니다\n",
            "1. (1) 항목입니다\n",
            "- · 항목입니다\n",
        ] {
            let findings = only_rule(md, "struct-item-mark");
            assert_eq!(findings.len(), 1, "{md:?} -> {findings:?}");
            assert_eq!(findings[0].severity, Severity::Error);
        }
    }

    #[test]
    fn struct_roman_heading_fires_on_ascii_numerals() {
        for md in ["# I. 사업 개요\n", "## III. 추진 계획\n", "I. 사업 개요\n"] {
            let findings = only_rule(md, "struct-roman-heading");
            assert_eq!(findings.len(), 1, "{md:?} -> {findings:?}");
            assert_eq!(findings[0].severity, Severity::Error);
        }
    }

    #[test]
    fn struct_roman_heading_silent_on_fullwidth_and_english() {
        for md in ["## Ⅰ. 사업 개요\n", "Ⅻ. 기타 사항\n", "I. Introduction\n"] {
            assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
        }
    }

    #[test]
    fn fp_guard_ho_citation() {
        let md = "제3.01.호를 참조하시기 바랍니다.\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
    }

    #[test]
    fn fp_guard_version_string() {
        let md = "버전 v2.05.1 기준으로 작성\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
    }

    #[test]
    fn fp_guard_table_rule_row() {
        let md = "| 구분 | 내용 |\n| --- | --- |\n| 날짜 | 2020.7.8 |\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
    }

    #[test]
    fn fp_guard_bold_wrapped_mark() {
        // Adopted research A3 (bold-x2 guard): a Strong-wrapped hand-typed
        // mark yields exactly one finding — not zero and not two.
        let findings = only_rule("**가. 항목입니다**\n", "struct-item-mark");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].severity, Severity::Error);
    }

    #[test]
    fn skips_code_and_tables() {
        let md = "```\n가. 항목\n오후 3시 20분\n345천원\n붙임: 계획서 1부\n2020.7.8\nI. 사업 개요\n■ 참고\n```\n\n| 규칙 |\n| --- |\n| 가. 항목 |\n\n평범한 문단입니다.\n";
        assert!(lint(md).is_empty(), "{md:?} -> {:?}", lint(md));
    }

    #[test]
    fn severity_split_matches_d05() {
        // D-05: exactly the two structure rules are error-severity; every
        // notation rule and ai-style-marks is a warning.
        let md = "시행: 2020.7.8\n\n일시: 오후 3시 20분\n\n비용: 345천원\n\n붙임: 계획서 1부.  끝.\n\n원장:김갑동\n\n기간: 10~20\n\n■ 참고\n\n가. 항목입니다\n\n# I. 개요\n\n붙임  1. 계획서 1부.  끝.\n\n끝\n";
        let findings = lint(md);
        let mut seen = std::collections::HashMap::new();
        for f in &findings {
            seen.insert(f.rule_id, f.severity);
        }
        assert_eq!(seen.len(), 10, "{findings:?}");
        for (rule, severity) in &seen {
            let expected = if matches!(*rule, "struct-item-mark" | "struct-roman-heading") {
                Severity::Error
            } else {
                Severity::Warning
            };
            assert_eq!(*severity, expected, "{rule}");
        }
    }
}
