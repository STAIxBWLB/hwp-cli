[한국어](korean-official-format.ko.md) · [English](korean-official-format.md)

# Korean official-document format — regulation reference

**Purpose.** This file is the rule source the `hwp` skill's official-document output is judged
against: the statutory item-mark sequence, indent and notation rules, and the per-document
skeletons for 기안문·보고서·계획서·회의록·공고문·보도자료. Phase 2.2 (numbering presets) and
Phase 2.3 (`hwp lint`) cite this file as their rule source, so every rule here is written to be
refutable: statute article + 2020 행정업무운영 편람 page where known + a per-section confidence
tag (D-12).

**How to read the confidence tags.**

- `confirmed` — stated in the statutes (「행정업무의 운영 및 혁신에 관한 규정」 및 동 시행규칙,
  「공공기록물 관리에 관한 법률 시행령」) or in the 2020 행정업무운영 편람; safe to enforce.
- `practice` — field convention (실무 관행) or a non-statutory guideline; follow it by default,
  but an agency's own convention wins and a linter must never hard-fail on it.
- `unverified` — no authoritative source found; recorded for honesty, never asserted as a rule.

**Citation convention.** Each section header line carries its confidence tag; the first line of
each section gives the primary basis (statute article and, where the 2020 편람 pins it down, the
page). Rules are restated from the statutes, not copied from any secondary compilation (see
§12 Sources).

**Regulation name.** The current name is 「행정업무의 운영 및 혁신에 관한 규정」 (대통령령
제33575호, 2023. 6. 27. 시행), 약칭 행정업무규정 — formerly 사무관리규정 → 행정업무의 효율적
운영에 관한 규정 (2011) → 행정 효율과 협업 촉진에 관한 규정 (2016) → the current name. The
article numbers cited here survived the renamings unchanged.

## 1. Item marks — the 8-level statutory sequence · confirmed

Source: 「행정업무의 운영 및 혁신에 관한 규정 시행규칙」 제2조제1항; 2020 행정업무운영 편람
(항목 표시). Confidence: confirmed (code points byte-verified).

When the body of an official document is divided into two or more items, the items are marked
in this exact top-to-bottom order (시행규칙 제2조제1항):

**`1.` → `가.` → `1)` → `가)` → `(1)` → `(가)` → `①` → `㉮`**

The same article's proviso allows special symbols such as `□ ○ - ㆍ` "필요한 경우" (when
needed) — see §3. The canonical sequence is exactly **8 levels**: summaries that stop at `①`
(7 levels) are incomplete — the regulation and the 편람 both include `㉮`.

### Code points and exact composition

| Level | Ordinal | First mark | Exact composition |
|-------|---------|-----------|-------------------|
| 1 | 첫째 | `1.` | ASCII digit `1` (U+0031) + period `.` (U+002E) |
| 2 | 둘째 | `가.` | Hangul syllable `가` (U+AC00) + period |
| 3 | 셋째 | `1)` | digit + closing parenthesis `)` (U+0029) |
| 4 | 넷째 | `가)` | Hangul syllable + closing parenthesis |
| 5 | 다섯째 | `(1)` | **three characters**: `(` + digit + `)` — NOT the single parenthesized digit ⑴ (U+2474) |
| 6 | 여섯째 | `(가)` | **three characters**: `(` + syllable + `)` — NOT the single ㈎ (U+3216) |
| 7 | 일곱째 | `①` | **single character** ① = **U+2460** (CIRCLED DIGIT ONE) |
| 8 | 여덟째 | `㉮` | **single character** ㉮ = **U+326E** (CIRCLED HANGUL KIYEOK A) |

Generators must emit levels 5-6 as the 3-character parenthesis combinations and levels 7-8 as
the single Unicode code points above — mixing the two forms (e.g. a single ⑴ at level 5) is a
format error.

## 2. Indent ladder and the single-item rule · confirmed

Source: 2020 행정업무운영 편람 (항목 정렬); 시행규칙 제2조. Confidence: confirmed.

- The first level (`1.`) starts at the left base line with **no indent**.
- From the second level down, each level indents **2타 cumulatively** from its parent level
  (둘째 = 2타, 셋째 = 4타, 넷째 = 6타, …).
- Between the item mark and the item content: **1타** (e.g. `가.∨○○○`, `(1)∨○○○`).
- **2타 = one Hangul glyph = two half-width columns** (ASCII letters/digits); 1타 = one
  half-width column.
- **Single item = no mark.** If a level has only one item, do not give it a mark — write it as
  a plain paragraph.

### Cumulative indent ladder

| Level | Mark | Indent (타) | Half-width spaces |
|-------|------|------------|-------------------|
| 1 첫째 | `1.` | 0 | 0 |
| 2 둘째 | `가.` | 2 | 2 |
| 3 셋째 | `1)` | 4 | 4 |
| 4 넷째 | `가)` | 6 | 6 |
| 5 다섯째 | `(1)` | 8 | 8 |
| 6 여섯째 | `(가)` | 10 | 10 |
| 7 일곱째 | `①` | 12 | 12 |
| 8 여덟째 | `㉮` | 14 | 14 |

Standard indent diagram (∨ = 1타):

```
1.∨○○○○○○
∨∨가.∨○○○○○○
∨∨∨∨1)∨○○○○○○
∨∨∨∨∨∨가)∨○○○○○○
∨∨∨∨∨∨∨∨(1)∨○○○○○○
∨∨∨∨∨∨∨∨∨∨(가)∨○○○○○○
2.∨○○○○○○
```

### Hanging indent for continuation lines

- **Recommended:** from the second line of a multi-line item, align with the first content
  character (the position after mark + 1타) — 내어쓰기 / hanging indent.
- **Tolerated:** starting continuation lines at the left base line. Pick one form and keep it
  for the whole document — never mix the two in one document.

## 3. The □ ○ - ㆍ symbol ladder · practice

Source: 시행규칙 제2조제1항 단서 (special symbols permitted when needed) + 실무 관행; 2020
편람. Confidence: practice — the ladder order is **not** statutory.

- The proviso of 제2조제1항 permits special symbols such as `□ ○ - ㆍ` instead of the regular
  8-level marks "필요한 경우" (exceptional, optional).
- **No statute and no 편람 page fixes a per-level mapping of these symbols.** The de-facto
  ladder `□`(대) → `○`(중) → `-`(소) → `ㆍ`(세) is field convention (실무 관행), common in
  보고서 — it is not a 법정 순서.
- Code point of the fourth symbol: the 시행규칙 정본 encodes it as **ㆍ (U+318D, HANGUL LETTER
  ARAEA, 아래아)**. Other materials render the same slot as `·` (U+00B7) or `∙` (U+2219). All
  three encode the same 가운뎇점 (middle dot) intent; use U+318D for 정본 fidelity, U+00B7
  when readability comes first.
- 편람: do not use special symbols that risk breaking in electronic input/processing.
- Cross-reference: `official-documents.md`'s markdown contract maps literal `□ `/`○ ` runs and
  `-`/`·` bullets to this ladder — the two files must agree exactly.

## 4. Exhausting a level — 단모음 continuation · confirmed

Source: 2020 행정업무운영 편람 본문 **p.43** (primary); 시행규칙 제2조 states only the 8-level
sequence and the 가나다-order principle — the 거/너/더 expansion is the 편람's supplement.
Confidence: confirmed (편람 p.43).

When a Hangul-letter level runs past 하 (`하.` / `하)` / `(하)` / ㉻), continue in **단모음
(single-vowel) order**:

```
가→나→다→…→파→하 → 거→너→더→…→퍼→허 → 고→노→도→…
```

Applied per level: 둘째 `가.`…`하.` → `거.` → `너.` … · 넷째 `가)`…`하)` → `거)` … · 여섯째
`(가)`…`(하)` → `(거)` … · 여덟째 ㉮…㉻ → circled 거, circled 너, …

## 5. Document structure — 두문/본문/결문 and the 10 parts of a 기안문 · confirmed

Source: 시행규칙 제4조; 별지 제1호서식 (일반기안문); 2020 편람. Confidence: confirmed.

A 기안문 (which becomes the 시행문 when dispatched externally — same 별지 제1호서식 layout) is
divided into three blocks: **두문(頭文) · 본문(本文) · 결문(結文)**.

### 두문 — heading block

1. **행정기관명** — top of the document, **centered** (letter-spaced as in '행 정 기 관 명').
   When the name collides with another agency's, the superior agency's name is noted alongside.
2. **수신** — `수신∨∨○○○장관(○○○과장)`: recipient's title, with the 처리 보조/보좌기관 직위 in
   parentheses. Internal documents show **"내부결재"** (internal approval).
   With multiple recipients, 두문 shows "수신자 참조" and the recipient list moves to 결문.
3. **(경유)** — filled only for 경유문서 (the 경유 agency's 장 and the final 수신기관의 장);
   otherwise left blank.

### 본문 — body block

1. **제목** — first line of the body: `제목∨∨○○○`.
2. **내용** — uses the item-mark system of §1-§4. Field convention starts the body with
   `1. 관련:` (근거 문서번호+제목); the usual detail order is 일시-장소-대상-내용-방법.
3. **붙임 / 끝.** — see §6.

### 결문 — closing block (top → bottom)

1. **발신 명의** — below the body, **centered**; in principle the head of the agency. Not shown
   on 내부결재 documents. The 관인 is stamped so that the last character of the 발신명의 sits in
   the center of the seal impression (red).
2. **결재란** — two-line structure: `결재  (직위/직급) 기안자 서명  (직위/직급) 검토자 서명
   (직위/직급) 결재권자 서명`, then a **separate `협조` line** for the 협조자 — never mixed into
   the 기안/검토/결재 line.
3. **발의자·보고자 표기** (시행규칙 제6조제1항): 발의자 = `★`, 보고자 = `⊙`, placed before or
   above the 직위/직급.
4. **시행/접수 칸** — `시행∨∨처리과명-연도별일련번호(시행일)` /
   `접수∨∨처리과명-연도별일련번호(접수일)`, e.g. `시행  행정제도과-1234(2024. 5. 3.)`.
5. **기관 정보** — fixed two-column block: left column 우편번호+도로명주소, then 전화·팩스 on
   one line; right column 홈페이지 → 전자우편 → 공개 구분, separated by `/`.
6. **공개 구분** — one of 공개 / 부분공개 / 비공개; 부분공개·비공개 cite the 정보공개법
   제9조제1항 호.

### The 10 parts of a 기안문

Rewritten from the old skill's summary, in document order:

1. **행정기관명 (머리말)** — agency name at the top; electronic approval systems insert it.
2. **수신** — "내부결재" for internal routing; the specific agency head's title for external
   dispatch; "수신자 참조" for many recipients.
3. **(경유)** — only when a 경유 agency exists.
4. **제목** — one-line summary: "…계획(안)", "…보고", "…협조 요청".
5. **본문** — 개조식, item-mark system of §1-§4.
6. **붙임 + 끝.** — attachment list per §6, closed by the `끝.` mark.
7. **발신명의** — external dispatch only; the 관인 sits at its last character.
8. **결재란** — 기안자/검토자/협조자/결재자; the approval system renders it, templates only
   hold the placeholders.
9. **시행·접수 정보** — 문서번호·일자; 시행 filled by the drafting agency, 접수 by the
   receiving agency.
10. **기관 정보** — 주소, 홈페이지, 전화, 팩스, 전자우편, 공개구분.

```
┌─────────────────────────────────────────┐
│              행 정 기 관 명                │  ← 두문: centered
│ 수신  ○○○장관(○○○과장)                    │
│ (경유)                                    │
│ 제목  ○○○○○○                             │  ← 본문
│                                          │
│ 1. ……                                    │
│   가. ……                                 │
│ 붙임  1. ○○○ 1부.  끝.                    │
│                                          │
│              발 신 명 의   (관인)          │  ← 결문: centered
│ 결재 (직위) 기안자 서명 (직위) 검토자 서명 (직위) 결재권자 서명 │
│ 협조 (직위) 협조자 서명                     │  ← separate 협조 line
│ 시행 처리과명-번호(시행일)  접수 처리과명-번호(접수일)  │
│ 우00000 도로명주소 / 홈페이지주소           │
│ 전화( )  팩스( )  / 전자우편 / 공개구분     │
└─────────────────────────────────────────┘
```

### Document classes (공문서 종류)

Source: 규정 제4조 (6 classes). Confidence: confirmed.

| Class | Definition | Examples |
|-------|-----------|----------|
| 법규문서 | documents with legal force | 법률, 대통령령, 부령, 조례, 규칙 |
| 지시문서 | directives from a superior agency | 훈령, 지시, 예규, 일일명령 |
| 공고문서 | public notices | 고시, 공고 |
| 비치문서 | registers kept at the agency | 비치대장, 비치카드 |
| 민원문서 | civil applications and their answers | 민원신청서, 민원회신 |
| 일반문서 | everything else | **기안문**, **보고서**, **사업계획서**, 회의록, 보도자료 |

The skill's main targets are 일반문서 — especially 기안문 (결재용) and 보고서·사업계획서.

## 6. Notation rules — 표기법 · confirmed

Source: 2020 행정업무운영 편람 표기법. Confidence: confirmed (except where a sub-rule is
tagged otherwise).

### 날짜 — dates

- **`YYYY. M. D.`** — the 연·월·일 characters are dropped and replaced by periods; a **final
  period after the day is mandatory**; one space after each period; leading zeros in month and
  day are removed. Example: `2026. 6. 19.`
- Weekday in parentheses, no space before the parenthesis: `2023. 11. 11.(토)`.
- Ranges use the 물결표: `4. 23.∼6. 15.`.
- Correct: `2020. 7. 8.` — wrong: `2020.7.8` (no spaces, no final period), `1985.09.06.`
  (leading zeros).

### 시각 — times

- **`HH:MM`** — 24-hour clock, colon between hour and minute, **leading zeros kept** (the
  opposite of dates): `오후 3시 20분` → `15:20`, `오전 8시 9분` → `08:09`.
- Date and time together: `일시: 2023. 12. 1.(금) 15:00∼17:00`.

### 금액 — amounts

- **`금NNN,NNN원(금<한글 수>원)`** — to prevent alteration, 금 + digits + 원 are written
  together and the Korean reading is repeated in parentheses:
  `금113,560원(금일십일만삼천오백육십원)`.
- Avoid abstract thousand-units: `345천원` → `34만 5천 원` or `345,000원`.

### 숫자와 단위 — numbers and units

- Arabic numerals. A dependent-noun unit may be spaced or attached — `50 명` and `50명` are
  both valid.
- The suffixes `-여/-쯤/-가량/-당/-바` attach: `50여 명`, `20명당 1명`.

### 붙임 — attachments

- `붙임` goes on the **line after the body ends**, **without a colon** (`붙임:` ✗ → `붙임∨∨` ○).
- 2타 after `붙임`, then the attachment name + quantity; quantity ends with a period: `1부.`
- **One attachment: omit the item number `1.`** Two or more: number them `1. 2.` and align the
  second line under the first character of the first name.

```
붙임∨∨2023학년도 ○○ 명단 1부.∨∨끝.        (one attachment)

붙임∨∨1. ○○○계획서 1부.                  (two or more)
∨∨∨∨∨∨2. ○○○서류 1부.∨∨끝.
```

### 끝. — the end mark

Core formula: **2타 (one Hangul glyph) + `끝` + period**.

| Case | Handling |
|------|----------|
| Body ends with text | 2타 after the last character: `…바랍니다.∨∨끝.` |
| Body/붙임 ends at the right margin | next line, 2타 from the left margin, then `끝.` |
| With attachments | 2타 after the last attachment line, then `끝.` |
| Ends with a full table | below the table, 2타 from the left margin, then `끝.` |
| Table ends mid-way (cells left empty) | no `끝.` — write **"이하 빈칸"** in the next cell/line |

### 문장부호 — punctuation

- **가운뎇점 (·)**: for 대등·밀접한 열거 (`융·복합`); not used inside dictionary words
  (`시도`, not `시·도`).
- **쌍점 (:)**: attached to the preceding word, 1타 after (`원장:∨김갑동`); never after `붙임`.
- **물결표 (∼)**: attached on both sides; prefer the character `∼` over the keyboard `~`.
- **낫표 (「 」)**: for statute names (「행정업무의 운영 및 혁신에 관한 규정」); single quotation
  marks are tolerated.
- 띄어쓰기: 성+이름 together, 호칭 spaced (`홍길동 씨`, `교육부 장관`).

### 어투 — sentence endings

Confidence: practice (the document-type ↔ ending mapping has no single authoritative text).

- **시행문 (external)**: polite endings — `~하시기 바랍니다`, `~을 알려드립니다` (하십시오체).
- **보고서·계획서 (internal)**: **개조식** — short clauses, noun-form endings `~함/~임/~음/~바람`.
- Never mix 서술식 and 개조식 in one document; prefer plain words (`금일`→`오늘`,
  `향후`→`앞으로`).

**Adjacency note.** Where two notation rules touch (a date immediately followed by a
punctuation mark, 붙임 adjacent to a heading), each rule is stated separately above and no
combined behavior is defined — no specification exists to resolve such merges; flagged for
verify-work.
