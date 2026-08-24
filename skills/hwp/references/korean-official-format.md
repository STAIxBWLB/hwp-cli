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

**Conflicting secondary source, superseded.** 한국공공언어진흥원's 「공문서 작성법 길라잡이」
(2025-01-08) prints levels 5-6 as the precomposed `⑴ ⑵ ⑶ ⑷` and `㈎ ㈏ ㈐ ㈑`. 시행규칙
제2조제1항 is the higher authority and gives the parenthesis combinations, which is also what
14/14 genuine Hancom artifacts showed under GN-9's verification pass. The engine emits the
3-character form; do not file this as a defect.

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
- **No statute and no 편람 page fixes a per-level mapping of these symbols**, so the ladder
  `□`(대) → `○`(중) → `-`(소) → `ㆍ`(세) is field convention (실무 관행), common in 보고서 —
  not a 법정 순서. It does have a published source: 한국공공언어진흥원's 「공문서 작성법
  길라잡이」(2025-01-08) fixes the same four rungs twice, as `□ ○ - •` in its 8-level 원칙
  table (제1장 다) and as `□>○>->∙` in its 유의 사항 table (제1장 2). Cite that for the order;
  it raises the convention above hearsay without making it statutory.
- Code point of the fourth symbol: the 시행규칙 정본 encodes it as **ㆍ (U+318D, HANGUL LETTER
  ARAEA, 아래아)**. Other materials render the same slot as `·` (U+00B7), `∙` (U+2219) or
  `•` (U+2022) — the 길라잡이 uses the last two in its two tables. All four encode the same
  가운뎇점 (middle dot) intent; use U+318D for 정본 fidelity, U+00B7 when readability comes
  first. The engine emits U+00B7.
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

## 7. 회의록 — the 9 statutory elements · confirmed

Source: 「공공기록물 관리에 관한 법률 시행령」 **제18조** — note the source is the 공공기록물
시행령, NOT the 행정업무규정. Confidence: confirmed (법령).

Minutes of a meeting (회의록) must record these 9 statutory elements:

1. 회의 명칭
2. 개최기관
3. 일시·장소
4. 참석자·배석자 명단
5. 진행 순서
6. 상정 안건
7. 발언 요지
8. 결정 사항
9. 표결 내용

For 지정회의 (designated meetings), a 속기록 (stenographic record) or 녹음기록 (audio
recording) accompanies the minutes — with a 녹취록 when recorded.

## 8. 공고문 — public notices · confirmed / practice

Source: 규정 제6조제3항 (효력) — confirmed; 번호 형식 — practice. Confidence: confirmed
(5일 효력, 법령) / practice (numbering format).

- 고시 and 공고 are both 공고문서: 고시 carries continuing force and binding effect; 공고 is
  one-off and non-binding.
- **Effect (5일 효력):** when the effective date is not stated, a 고시·공고 takes effect
  **5 days after its publication date** (규정 제6조제3항).
- **Numbering:** year-prefixed serial number with a hyphen — `제2025-282호`.
- Structure: header (기관 + 번호) → 제목 → 본문 (법령 근거) → 발신 (기관장) · 시행일.

## 9. 보도자료 — press releases · practice

Source: 국립국어원 「보도자료 작성 길잡이」 (an authoritative guideline, not a statute).
Confidence: practice.

A 보도자료 is built in three parts:

1. **머리** — 기관, 보도/배포 일시, 작성자·연락처.
2. **내용** — 표제 + 부제, 리드문, 본문:
   - 표제: 종결어미형 (for vividness) or 명사문형 (for compression).
   - 부제: noun phrase in dashes `- … -`, at most two.
   - 리드문: 육하원칙 — 누가·언제·무엇 are mandatory.
   - 본문: 두괄식 역피라미드 (conclusion first), items on the `□`(대) `ㅇ`(중) `-`(소) ladder.
3. **부가** — 붙임, 공공누리 license note.

## 10. Report and plan skeletons — 보고서·사업계획서 · practice

Source: 관행 골격 (no statutory form; restated from the field-standard structures).
Confidence: practice (medium).

### 보고서 skeleton

- Short status report — 4 sections:
  1. 추진 배경
  2. 주요 내용
  3. 향후 계획
  4. 행정 사항
- Longer reports extend the middle: 추진배경 → 현황 → 문제점 → 개선방안 → 기대효과 →
  추진계획 → [행정사항]. Title shows the direction (`○○ 개선방안 보고`, `○○ 검토보고`,
  `○○ 개최결과 보고`); **1건 1매** (one topic per page) is the field principle.
- Endings: 개조식 (see §6 어투).

### 사업계획서 skeleton

The common 9-section backbone (individual 사업단 forms vary):

```
Ⅰ.   사업 개요          — 사업명, 기간, 총사업비, 주관·참여기관
Ⅱ.   추진 배경 및 필요성 — 대내외 환경, 정책·기술 트렌드
Ⅲ.   추진 목표 및 전략   — 최종목표, 연차별 목표, 추진전략
Ⅳ.   세부 추진 내용      — 세부과제 1/2/3 …
Ⅴ.   추진 일정          — 연차별·월별 일정
Ⅵ.   소요 예산          — 인건비/사업비/관리비 내역
Ⅶ.   기대 효과          — 정량/정성 효과
Ⅷ.   성과 지표          — KPI, 측정 방법
Ⅸ.   붙임              — 참여진 이력, 예산 상세, 추진 체계도
```

A shorter field sequence shares the report skeleton: 추진배경/목적 → 추진방향 →
추진계획(일시·장소·대상·내용·방법) → 소요예산 → 추진일정 → 행정사항 → [기대효과].

## 11. Margins and fonts — verified engine behavior, not regulation · practice

Source: [docs/design/23-hwpx-skill-absorption.md](../../../docs/design/23-hwpx-skill-absorption.md)
§3 (margin check, D-14) and [docs/design/12-feature-gaps.md](../../../docs/design/12-feature-gaps.md)
GN-9; 2020 편람 서식 설계기준. Confidence: verified implementation behavior (margins and
profiles) / practice (fonts and line spacing).

### Margins (여백)

**These are verified engine behavior — never cite them as regulation.** Every canonical profile
starts with **top 20 / bottom 10 / left 20 / right 20 mm (20/10/20/20)**. The old top-30 mm value
had no authoritative source; the current tuple agrees with the 2020 편람 reference. A caller may
override one side explicitly with `--margin-top`, `--margin-bottom`, `--margin-left` or
`--margin-right`; that override is a caller choice, not a statutory claim.

| Canonical profile | Body default | Header/footer | Page number |
|---|---|---|---|
| `official` | Malgun Gothic 12pt / 160% | 0 mm | none |
| `report` | HCR Batang 15pt / 160% | 15 mm | `- N -` |
| `plan` | HCR Batang 15pt / 160% | 15 mm | `- N -` |
| `notice` | Malgun Gothic 15pt / 160% | 10 mm | `- N -` |
| `minutes` | HCR Batang 14pt / 130% | 0 mm | none |
| `press` | HCR Batang 14pt / 160% | 10 mm | `- N -` |

`official` is canonical; `gian` and `gongmun` are semantic compatibility aliases, not raw-byte
promises. Korean aliases normalize to the same six profiles. There is exactly one profile per
document type. 개조식 is a writing style (§6 어투), not a document class, so it names no profile;
the `gaejosik` profile was retired in 0.9.0 and `--preset gaejosik` now fails with a message
pointing here. The verified HWPX path writes the
eight-level definition directly, and the HWP5 path uses the safe, direct encoding observed in
Hancom Office. At levels 2, 6 and 8, the count continues after `하` as observed; no approximation
is used or implied.

### Fonts and line spacing (글꼴·줄간격)

**No statute fixes the body font, size, or line spacing of a general 기안문** — only 서식
(별지·민원 서식) typography is statutory. Everything below is 관행 (practice):

- **맑은고딕 12pt** — `official` profile default; a current official-document practice,
  not a statutory body-style requirement.
- **명조 계열 (휴먼명조/함초롬바탕) 14-15pt** — the traditional 보고서 convention.
- Line spacing: **123%** (한글 기본값) / **160%** (보고서 가독성 관행) / **130% 이상**
  (큰글자 서식 — the only statutory value, 시행규칙 별표5, and it applies to 큰글자 서식
  only).
- 자간 0 / 장평 100 are the field defaults.

## 12. Sources and the pre-send checklist

### Sources (D-13)

- **kordoc** (`~/workspace/references/ai-tools/kordoc`, **MIT**) — credited as the **secondary
  source** for the statute compilation this reference's section map follows
  (`docs/gongmunseo-reference.md`: §1 marks/indent/단모음, §4 문서 구조, §5 표기법, §6.4
  회의록, §6.5 보도자료, §6.6 공고문, §3.2 margin refutation, §2 typography). Rules are
  restated from the statutes, not copied.
- **jkf87/hwpx-skill** — rule ancestry of parts of the old skill's rule set is acknowledged
  by name. The repository carries **no license** (verified 2026-08-20 via the GitHub API), so
  **no rule text is copied** from it; this acknowledgment is the full extent of the reuse.
- **Primary basis** — 「행정업무의 운영 및 혁신에 관한 규정」 및 동 시행규칙, 「공공기록물
  관리에 관한 법률 시행령」 제18조, 2020 행정업무운영 편람 (p.43 for 단모음).
- **한국공공언어진흥원, 「공문서 작성법 길라잡이」 (2025-01-08)** — secondary source, read
  2026-08-22. Cited in §1 for a recorded conflict on levels 5-6 and in §3 for the `□ ○ - ㆍ`
  rung order. Its 제2장-제5장 ○/× notation tables are the labelled corpus behind the `hwp lint`
  rule set (GN-3). It ranks below 시행규칙 and the 편람 and
  above field convention; where it conflicts with either, the conflict is recorded, not resolved
  in its favour. Not committed to this repository — cited by 장·절 only.
- **Old `hwpx` skill** (STAIxBWLB/skills, read-only) — the 기안문 10-part box, document
  classes and skeleton content were rewritten from it.

### Pre-send checklist

Before dispatching an official document:

- [ ] A4 portrait; item marks follow the §1 sequence (engine-assigned, never hand-typed)
- [ ] Indent ladder and single-item rule per §2; `□ ○ - ㆍ` used consistently per §3
- [ ] 날짜 `YYYY. M. D.`, 시각 `HH:MM`, 금액 `금NNN,NNN원(금…원)` per §6
- [ ] 붙임 without a colon, closed by `끝.` per §6
- [ ] 수신, 시행번호·시행일자, 공개구분 filled (기안문)
- [ ] 회의록 carries all 9 statutory elements (§7)
- [ ] Sections tagged practice above are conventions — check the agency's own convention
- [ ] `hwp validate` passes
- [ ] Opened once in Hancom Office with no layout breakage (manual — cannot be automated)

Phase 2.3 adds **`hwp lint`**, which automates the mechanical half of this checklist —
until it lands, run the checklist by hand.
