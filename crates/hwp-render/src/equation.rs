//! HWP 수식 스크립트 조판 (mini-TeX).
//!
//! 한글 수식 개체는 글립 배치가 아니라 **텍스트 스크립트**(EQN 호환)를 저장한다(deep-research
//! 확정). 이 모듈은 스크립트를 math 트리로 파싱해 box model로 배치하고, 글리프 런 + 선(분수선·
//! 근호 vinculum)을 **baseline 상대 좌표**로 방출한다. `layout.rs`의 수식 arm이 사용.
//!
//! 문법 근거(한컴 수식 spec rev1.2): `over`/`atop`(분수, atop=선없음), `sqrt`(근호), `^`/`_`
//! (첨자), `{ }`(그룹), `~`·`` ` ``(공백), `#`(줄바꿈), `&`(열정렬). 함수어(sin/cos/log/lim…)는
//! 로만체, 그 외 단일 라틴 문자는 변수로 이탤릭. v1: 분수·첨자·근호·기호·함수어. 범위 밖:
//! 행렬(matrix)·큰연산자 극한·복잡 구분자.

use crate::display::PageList;
use crate::fonts::FontStore;
use crate::shape::{ShapedRun, shape_plain};
use hwp_model::Document;

// ── 1. 토큰 ──
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    LBrace,
    RBrace,
    Sup,        // ^
    Sub,        // _
    Hash,       // # — 줄바꿈(행 나눔)
    Space(f32), // ~ (1) 또는 ` (0.25) — em 배수
}

fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<Tok>| {
        if !word.is_empty() {
            out.push(Tok::Word(std::mem::take(word)));
        }
    };
    for c in s.chars() {
        match c {
            '{' => {
                flush(&mut word, &mut out);
                out.push(Tok::LBrace);
            }
            '}' => {
                flush(&mut word, &mut out);
                out.push(Tok::RBrace);
            }
            '^' => {
                flush(&mut word, &mut out);
                out.push(Tok::Sup);
            }
            '_' => {
                flush(&mut word, &mut out);
                out.push(Tok::Sub);
            }
            '#' => {
                flush(&mut word, &mut out);
                out.push(Tok::Hash);
            }
            '&' => flush(&mut word, &mut out), // 열 정렬(matrix) — v1은 공백 취급
            '~' => {
                flush(&mut word, &mut out);
                out.push(Tok::Space(1.0));
            }
            '`' => {
                flush(&mut word, &mut out);
                out.push(Tok::Space(0.25));
            }
            c if c.is_whitespace() => flush(&mut word, &mut out),
            _ => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

// ── 2. AST ──
#[derive(Debug, Clone)]
enum Node {
    Row(Vec<Node>),
    /// 여러 행을 세로로 쌓음(`#` 줄바꿈).
    Stack(Vec<Node>),
    /// 기호/텍스트 (roman=참이면 로만체, 거짓이면 변수 이탤릭 — v1은 스타일 미분화, 표시만).
    Sym(String),
    /// 분수: (분자, 분모, bar=분수선 유무).
    Frac(Box<Node>, Box<Node>, bool),
    /// 첨자: (base, sup, sub).
    Script(Box<Node>, Option<Box<Node>>, Option<Box<Node>>),
    /// 근호.
    Sqrt(Box<Node>),
    /// 가로 공백(em 배수).
    Space(f32),
}

struct Parser {
    toks: Vec<Tok>,
    i: usize,
}

impl Parser {
    /// 한 원자(atom): 그룹 {}·sqrt·단어·공백. 첨자(^/_)는 상위에서 후위로 붙인다.
    fn atom(&mut self) -> Option<Node> {
        match self.toks.get(self.i)?.clone() {
            Tok::LBrace => {
                self.i += 1;
                let inner = self.row_until_brace();
                if self.toks.get(self.i) == Some(&Tok::RBrace) {
                    self.i += 1;
                }
                Some(inner)
            }
            Tok::Space(w) => {
                self.i += 1;
                Some(Node::Space(w))
            }
            Tok::Word(w) => {
                self.i += 1;
                match w.as_str() {
                    "sqrt" | "root" => Some(Node::Sqrt(Box::new(self.atom_scripted()))),
                    _ => Some(Node::Sym(w)),
                }
            }
            _ => None, // RBrace/Sup/Sub는 원자 아님
        }
    }

    /// 원자 + 후위 첨자(^{..} _{..}). base^sup_sub 순서 무관 수집.
    fn atom_scripted(&mut self) -> Node {
        let base = self.atom().unwrap_or(Node::Row(vec![]));
        let (mut sup, mut sub) = (None, None);
        loop {
            match self.toks.get(self.i) {
                Some(Tok::Sup) => {
                    self.i += 1;
                    sup = Some(Box::new(self.atom().unwrap_or(Node::Row(vec![]))));
                }
                Some(Tok::Sub) => {
                    self.i += 1;
                    sub = Some(Box::new(self.atom().unwrap_or(Node::Row(vec![]))));
                }
                _ => break,
            }
        }
        if sup.is_some() || sub.is_some() {
            Node::Script(Box::new(base), sup, sub)
        } else {
            base
        }
    }

    /// `}` 또는 끝까지 한 행. `over`/`atop`은 중위 분수로 분할(행 전체 = 분자, 이후 = 분모).
    fn row_until_brace(&mut self) -> Node {
        let mut items = Vec::new();
        while let Some(t) = self.toks.get(self.i) {
            if *t == Tok::RBrace || *t == Tok::Hash {
                break;
            }
            if let Tok::Word(w) = t
                && (w == "over" || w == "atop")
            {
                // `over`/`atop`은 중위 연산자: 분자=바로 앞 원자, 분모=바로 뒤 원자(HWP 실측).
                // 전체 행 분할이 아니다 — `a + b over c` = a + (b/c), `{a+b} over c`로 그룹.
                let bar = w == "over";
                self.i += 1;
                let num = items.pop().unwrap_or(Node::Row(vec![]));
                let den = self.atom_scripted();
                items.push(Node::Frac(Box::new(num), Box::new(den), bar));
                continue;
            }
            match self.atom_scripted() {
                Node::Row(v) if v.is_empty() => break, // 진행 불가 방어
                n => items.push(n),
            }
        }
        if items.len() == 1 {
            items.pop().unwrap()
        } else {
            Node::Row(items)
        }
    }
}

fn parse(script: &str) -> Node {
    let mut p = Parser {
        toks: tokenize(script),
        i: 0,
    };
    // `#`로 나뉜 행들을 세로로 쌓는다.
    let mut rows = Vec::new();
    loop {
        rows.push(p.row_until_brace());
        if p.toks.get(p.i) == Some(&Tok::Hash) {
            p.i += 1;
        } else {
            break;
        }
    }
    if rows.len() == 1 {
        rows.pop().unwrap()
    } else {
        Node::Stack(rows)
    }
}

// ── 3. 기호 매핑 ──
// 함수어(sin/cos/log/lim…)는 원문 그대로 로만체로 표시된다(sym_text의 기본 분기). 이탤릭/로만
// 스타일 분화는 후속 — v1은 표시 문자열만 매핑한다.

/// 수식 토큰 → 표시 문자열(기호). 함수어/숫자/미지정은 원문.
fn sym_text(tok: &str) -> String {
    let m = match tok {
        "times" => "×",
        "div" => "÷",
        "cdot" | "dot" => "·",
        "pm" => "±",
        "mp" => "∓",
        "sum" | "SUM" => "∑",
        "int" | "INT" => "∫",
        "prod" | "PROD" => "∏",
        "inf" | "infinity" => "∞",
        "partial" => "∂",
        "nabla" => "∇",
        "leq" | "<=" => "≤",
        "geq" | ">=" => "≥",
        "neq" | "!=" => "≠",
        "approx" => "≈",
        "to" | "rightarrow" | "->" => "→",
        "leftarrow" | "<-" => "←",
        "in" => "∈",
        "notin" => "∉",
        "cdots" => "⋯",
        "degree" => "°",
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "rho" => "ρ",
        "sigma" => "σ",
        "tau" => "τ",
        "phi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "ALPHA" => "Α",
        "BETA" => "Β",
        "EPSILON" => "Ε",
        "ZETA" => "Ζ",
        "ETA" => "Η",
        "IOTA" => "Ι",
        "KAPPA" => "Κ",
        "MU" => "Μ",
        "NU" => "Ν",
        "RHO" => "Ρ",
        "TAU" => "Τ",
        "CHI" => "Χ",
        "GAMMA" => "Γ",
        "DELTA" => "Δ",
        "THETA" => "Θ",
        "LAMBDA" => "Λ",
        "PI" => "Π",
        "PROD_U" => "Π",
        "SIGMA" => "Σ",
        "PHI" => "Φ",
        "PSI" => "Ψ",
        "OMEGA" => "Ω",
        // 글꼴/그룹 명령은 버린다.
        "LEFT" | "RIGHT" | "rm" | "it" | "bold" | "ITALIC" | "roman" => "",
        other => return other.to_string(),
    };
    m.to_string()
}

// ── 4. 조판 상자 (baseline 상대: y=0 baseline, 음수=위, 양수=아래) ──
/// baseline 상대 배치 결과. 원점 x=0(좌), y=0(baseline).
pub struct EqBox {
    pub width: f32,
    pub ascent: f32,                       // baseline 위(양수 크기)
    pub descent: f32,                      // baseline 아래(양수 크기)
    runs: Vec<(ShapedRun, f32, f32)>,      // (run, dx, dy_baseline)
    lines: Vec<(f32, f32, f32, f32, f32)>, // (x1,y1,x2,y2, 두께)
}

impl EqBox {
    fn empty() -> Self {
        EqBox {
            width: 0.0,
            ascent: 0.0,
            descent: 0.0,
            runs: vec![],
            lines: vec![],
        }
    }
    /// dx, dy 이동(합성용) — 소비.
    fn shift(mut self, dx: f32, dy: f32) -> Self {
        for r in &mut self.runs {
            r.1 += dx;
            r.2 += dy;
        }
        for l in &mut self.lines {
            l.0 += dx;
            l.1 += dy;
            l.2 += dx;
            l.3 += dy;
        }
        self.ascent -= dy; // dy>0(아래로) → ascent 감소
        self.descent += dy;
        self
    }
    fn merge(&mut self, other: EqBox) {
        self.runs.extend(other.runs);
        self.lines.extend(other.lines);
        self.width = self.width.max(other.width);
        self.ascent = self.ascent.max(other.ascent);
        self.descent = self.descent.max(other.descent);
    }
}

/// 한 기호/텍스트를 셰이핑해 상자로. italic=변수 기울임.
fn box_sym(store: &mut FontStore, doc: &Document, text: &str, size: f32, italic: bool) -> EqBox {
    if text.is_empty() {
        return EqBox::empty();
    }
    let Some(run) = shape_plain(store, doc, text, size, 0, italic) else {
        return EqBox::empty();
    };
    let width = run.width_pt;
    EqBox {
        width,
        ascent: size * 0.72,
        descent: size * 0.22,
        runs: vec![(run, 0.0, 0.0)],
        lines: vec![],
    }
}

/// 트리를 조판해 EqBox 생성(baseline 상대).
fn layout(store: &mut FontStore, doc: &Document, node: &Node, size: f32) -> EqBox {
    match node {
        Node::Space(w) => EqBox {
            width: size * w,
            ..EqBox::empty()
        },
        Node::Sym(w) => {
            let t = sym_text(w);
            // 변수(매핑 안 된 단일 라틴 문자)는 이탤릭, 함수어·숫자·그리스·기호는 로만(실측 정합).
            let italic = t == *w
                && w.chars().count() == 1
                && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
            box_sym(store, doc, &t, size, italic)
        }
        Node::Row(items) => {
            let mut acc = EqBox::empty();
            let mut x = 0.0;
            for it in items {
                let b = layout(store, doc, it, size).shift(x, 0.0);
                x += bwidth(&b, x);
                acc.merge(b);
            }
            acc.width = x;
            acc
        }
        Node::Stack(rows) => {
            let boxes: Vec<EqBox> = rows.iter().map(|r| layout(store, doc, r, size)).collect();
            let width = boxes.iter().map(|b| b.width).fold(0.0, f32::max);
            let gap = size * 0.35; // 행 사이 여백
            // 각 행의 실제 높이(ascent+descent)로 배치 — 분수 행은 더 높으므로 겹침 방지.
            let total_h: f32 = boxes.iter().map(|b| b.ascent + b.descent).sum::<f32>()
                + gap * boxes.len().saturating_sub(1) as f32;
            let mut acc = EqBox::empty();
            let mut top = -total_h * 0.5; // 스택 상단(baseline 중심 기준)
            for b in boxes {
                let dx = (width - b.width) * 0.5; // 가로 중앙정렬
                let baseline = top + b.ascent; // 이 행의 baseline
                let (a, d) = (b.ascent, b.descent);
                acc.merge(b.shift(dx, baseline));
                top += a + d + gap;
            }
            acc.width = width;
            acc
        }
        Node::Script(base, sup, sub) => {
            let bb = layout(store, doc, base, size);
            let mut acc = EqBox::empty();
            let bw = bb.width;
            acc.merge(bb);
            let ss = size * 0.72;
            if let Some(s) = sup {
                let raised = layout(store, doc, s, ss).shift(bw, -size * 0.5);
                acc.merge(raised);
            }
            if let Some(s) = sub {
                let lowered = layout(store, doc, s, ss).shift(bw, size * 0.28);
                acc.merge(lowered);
            }
            // 폭 = base + 첨자 폭(대략).
            let sw = sup
                .as_ref()
                .map(|s| layout(store, doc, s, ss).width)
                .unwrap_or(0.0)
                .max(
                    sub.as_ref()
                        .map(|s| layout(store, doc, s, ss).width)
                        .unwrap_or(0.0),
                );
            acc.width = bw + sw;
            acc
        }
        Node::Frac(num, den, bar) => {
            let n = layout(store, doc, num, size * 0.95);
            let d = layout(store, doc, den, size * 0.95);
            let width = n.width.max(d.width);
            let axis = size * 0.28; // 분수선 위치(baseline 위)
            let gap = size * 0.15;
            // 분자: 하단(baseline+descent)이 분수선 위 gap.
            let n_dy = -(axis + gap + n.descent);
            let nb = n.shift((width - nbw(num, store, doc, size)) * 0.5, n_dy);
            // 분모: 상단(baseline-ascent)이 분수선 아래 gap.
            let d_dy = axis + gap + layout(store, doc, den, size * 0.95).ascent;
            let db = d.shift((width - nbw(den, store, doc, size)) * 0.5, d_dy);
            let mut acc = EqBox {
                width,
                ascent: axis,
                descent: 0.0,
                runs: vec![],
                lines: vec![],
            };
            let _ = nb.width;
            acc.merge(nb);
            acc.merge(db);
            acc.width = width;
            if *bar {
                acc.lines
                    .push((0.0, -axis, width, -axis, (size * 0.05).max(0.6)));
            }
            acc
        }
        Node::Sqrt(inner) => {
            let ib = layout(store, doc, inner, size);
            let radical = box_sym(
                store,
                doc,
                "√",
                size * (1.0 + ib.ascent / size * 0.3),
                false,
            );
            let mut acc = EqBox::empty();
            let rw = radical.width;
            acc.merge(radical);
            let inner_shifted = ib.shift(rw, 0.0);
            let iw = inner_shifted.width - rw;
            let top = -(acc.ascent.max(inner_shifted.ascent));
            acc.merge(inner_shifted);
            // vinculum: 근호 위 가로선.
            acc.lines
                .push((rw, top, rw + iw, top, (size * 0.05).max(0.6)));
            acc.width = rw + iw;
            acc.ascent = acc.ascent.max(-top);
            acc
        }
    }
}

/// Row 배치 시 다음 x 진행량(폭).
fn bwidth(b: &EqBox, _x: f32) -> f32 {
    b.width
}

/// 노드 폭만 측정(중앙정렬 계산용). 재조판이지만 v1은 단순성 우선.
fn nbw(node: &Node, store: &mut FontStore, doc: &Document, size: f32) -> f32 {
    layout(store, doc, node, size * 0.95).width
}

/// 스크립트를 조판해 EqBox 반환(baseline 상대). size=기준 글자 크기(pt).
pub fn typeset(store: &mut FontStore, doc: &Document, script: &str, size: f32) -> EqBox {
    layout(store, doc, &parse(script), size)
}

/// 조판 상자를 페이지에 그린다. (ox, baseline_y) = 수식 원점 x + baseline의 페이지 y.
pub fn render_into(
    page: &mut PageList,
    eq: EqBox,
    ox: f32,
    baseline_y: f32,
    issues: &mut crate::issues::RenderIssueAccumulator,
) {
    use crate::display::Item;
    for (run, dx, dy) in eq.runs {
        crate::layout::push_run(page, ox + dx, baseline_y + dy, run, issues);
    }
    for (x1, y1, x2, y2, wdt) in eq.lines {
        if !issues.charge_display_items(1) {
            return;
        }
        page.items.push(Item::Path {
            commands: vec![
                crate::display::PathCmd::MoveTo(ox + x1, baseline_y + y1),
                crate::display::PathCmd::LineTo(ox + x2, baseline_y + y2),
            ],
            fill: None,
            stroke: Some(crate::display::Stroke::solid(0x0000_0000, wdt)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 토크나이즈_기본() {
        let t = tokenize("a over b");
        assert_eq!(
            t,
            vec![
                Tok::Word("a".into()),
                Tok::Word("over".into()),
                Tok::Word("b".into())
            ]
        );
        let t2 = tokenize("x^2 _i");
        assert!(t2.contains(&Tok::Sup) && t2.contains(&Tok::Sub));
    }

    #[test]
    fn 파싱_분수() {
        match parse("a over b") {
            Node::Frac(_, _, true) => {}
            other => panic!("분수 아님: {other:?}"),
        }
        match parse("x atop y") {
            Node::Frac(_, _, false) => {}
            other => panic!("atop 아님: {other:?}"),
        }
    }

    #[test]
    fn 파싱_첨자() {
        match parse("x^2") {
            Node::Script(_, Some(_), None) => {}
            other => panic!("첨자 아님: {other:?}"),
        }
    }

    #[test]
    fn 파싱_근호_그룹() {
        match parse("sqrt {a+b}") {
            Node::Sqrt(_) => {}
            other => panic!("근호 아님: {other:?}"),
        }
    }

    #[test]
    fn 기호_매핑() {
        assert_eq!(sym_text("alpha"), "α");
        assert_eq!(sym_text("sum"), "∑");
        assert_eq!(sym_text("x"), "x");
        assert_eq!(sym_text("rm"), "");
    }
}
