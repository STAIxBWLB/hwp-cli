[한국어](official-documents.ko.md) · [English](official-documents.md)

# Korean official documents with hwp-cli

This sub-guide is the self-contained reference for writing Korean official documents (공문)
with `hwp`: the markdown contract, per-document recipes for the six document types, the
template slot tables, the Korean alias table, and the fill/validate workflows. English is
canonical; `official-documents.ko.md` is the full Korean mirror. Everything here works with
the released binary today unless a section explicitly names a later phase.

The six document types in scope: 기안문 (draft), 보고서 (report), 계획서 (plan),
회의록 (minutes), 공고문 (public notice), 보도자료 (press release).

## 1. The markdown contract

Body item marks come from **nested-list depth**, not from typed characters. Write plain
nested markdown ordered lists (`1.` at every depth, indented two or more spaces per level)
and the engine assigns the mark for that depth:

| List depth | Statutory mark | What every official profile renders |
|---|---|---|
| 1 | `1.` | `1.` |
| 2 | `가.` | `가.` |
| 3 | `1)` | `1)` |
| 4 | `가)` | `가)` |
| 5 | `(1)` | `(1)` |
| 6 | `(가)` | `(가)` |
| 7 | `①` (U+2460) | `①` |
| 8 | `㉮` (U+326E) | `㉮` |

Verified implementation boundary:

- Every official profile assigns the statutory eight-level ladder. HWPX writes its matching
  `CircledHangul` numbering definition directly. HWP5 uses the verified safe, direct encoding
  path for the same visible ladder; this is an encoding result, not a raw-byte equivalence claim
  for aliases or source documents.
- List depth 8 succeeds. Depth 9 or deeper, including embedded HTML lists, fails closed and
  publishes no output. At levels 2, 6 and 8, counting continues after `하` as observed in
  Hancom Office.
- **Never hand-type marks on the numbered path.** A typed `가.` inside an ordered list
  renders next to the engine-assigned mark and double-numbers.

The □ ○ ladder is **literal symbol text**, a separate axis from list numbering:

- Type `□ ` (U+25A1) at the start of a paragraph for a level-1 block and `○ ` (U+25CB) for
  level 2. The engine keeps the glyph and outdents the paragraph so wrapped lines align
  after the symbol — no automatic marker is drawn on top. Use only `○`; never `◦`, ASCII
  `o`, `ㅇ` or `❍`.
- Keep `□ `/`○ ` lines as standalone paragraphs with a blank line before and after; a
  single newline does not reliably separate them from surrounding text.
- `- ` markdown bullets render as `-` at depth 1 and `·` (U+00B7) at depth 2 and below —
  the bottom two rungs of the `□ → ○ → - → ·` ladder.

Heading numbers are **literal**: type `Ⅰ. 1.` in the heading text itself (`Ⅰ` is U+2160,
the full-width Roman numeral — ASCII `I.` is forbidden). Headings carry no automatic
numbering.

A **single item is a plain paragraph**: when a list would contain exactly one item, write
it as an unmarked paragraph instead.

## 2. Per-document recipes

All six types are authored the same way today: a markdown skeleton under `templates/`
(exported next to this file), `hwp new --from <skeleton> --preset <preset>`, then `hwp fill`.
Phase 2.4 adds the `hwp new --template <alias>` shortcut; the recipes below are the
present-day path.

| Document type | Preset today | Template skeleton | Notes |
|---|---|---|---|
| 기안문 (draft) | `official` (Malgun Gothic 12pt/160%; `gian` compatibility alias — still works, prints a one-time deprecation note) | `gian-internal.md` (내부결재), `gian-external.md` (대외시행) | `gongmun-basic.md` covers the plain external official letter (no 접수번호/접수일자) |
| 보고서 (report) | `report` (HCR Batang 15pt/160%) | `report.md` | 15 mm header/footer, `- N -` page number; 배경 → 내용 → 계획 → 행정사항 skeleton |
| 계획서 (plan) | `plan` (HCR Batang 15pt/160%) | `plan.md` | 15 mm header/footer, `- N -` page number; 9-section 사업계획서 skeleton |
| 회의록 (minutes) | `minutes` (HCR Batang 14pt/130%) | `minutes.md` | No header/footer margin or page number; 9 statutory elements of 공공기록물 관리에 관한 법률 시행령 제18조 (D-19) |
| 공고문 (public notice) | `notice` (Malgun Gothic 15pt/160%) | `notice.md` | 10 mm header/footer, `- N -` page number; 공고번호 `제2025-282호` form |
| 보도자료 (press release) | `press` (HCR Batang 14pt/160%) | `press.md` | 10 mm header/footer, `- N -` page number; 보도시점/배포일 slots and inverted-pyramid body |

There is one profile per document type and no more. 개조식 is a writing style — the noun-form
sentence ending used inside 보고서·계획서 and 내부결재 bodies (§6 어투 of
`references/korean-official-format.md`) — so it selects no profile; pick the profile by document
type and apply the style in the body text. All six profiles start with top/bottom/left/right
margins of 20/10/20/20 mm; use `--margin-top`, `--margin-bottom`, `--margin-left` or
`--margin-right` only for an explicit per-side override. Canonical names are `official`,
`report`, `plan`, `notice`, `minutes` and `press`; `gian`, `gongmun` and the documented Korean
names normalize to one of them. The `gian` alias still works but prints a one-time deprecation
note.

Conventions every recipe shares:

- **`끝.` ending.** Official documents close the body with `끝.` after the last item (and
  after the `붙임  … 1부.` attachment list when one exists; a single attachment carries no
  item number, two or more are numbered `1.`, `2.` — §6 of the regulation reference).
- **발신명의 until Phase 2.4.** There is no frame/header machinery yet, so the centered
  22pt bold 발신명의 line (e.g. `예시대학교총장`) is produced by splicing an HTML-fragment
  part file that uses the block-level alignment contract (design doc 18 §8). Put this in
  the part file you pass to `hwp fill --set "발신명의=@sender.html"` or splice it into the
  body part:

  ```html
  <style>
  .cs0 { font-size: 22pt; }
  .ps0 { text-align: center; }
  </style>
  <p class="ps0"><span class="cs0"><strong>예시대학교총장</strong></span></p>
  ```

  Fragments carry a leading `<style>` element; import reads only the `.csN`/`.psN` rules,
  class numbers are producer-local (consumers reconstruct shapes by property values, never
  by name), `text-align` on `.psN` sets paragraph alignment, and marks such as bold come
  from the tags (`<strong>`), not the classes. This recipe lives only in this sub-guide and
  in part files — never in SKILL.md.
- **관인(직인) is not stamped.** The e-approval system (온나라/K-Office) inserts the seal at
  dispatch; do not try to generate one.

## 3. Slot tables

Slot lists per template skeleton (verbatim from `hwp slots` run on the old `.hwpx`
templates — do not re-derive). **Slots a template does not contain are ignored**, so a
shared `--set` list is safe to reuse across templates.

| Template | Slots |
|---|---|
| `gian-internal.md` | `{{기관명}}` `{{제목}}` `{{본문}}` `{{붙임}}` `{{발신명의}}` `{{기안자}}` `{{기안자직위}}` `{{협조자}}` `{{시행번호}}` `{{시행일자}}` |
| `gian-external.md` | `{{기관명}}` `{{수신}}` `{{경유}}` `{{제목}}` `{{본문}}` `{{붙임}}` `{{발신명의}}` `{{기안자}}` `{{검토자}}` `{{결재자}}` `{{시행번호}}` `{{시행일자}}` `{{접수번호}}` `{{접수일자}}` `{{주소}}` `{{홈페이지}}` `{{전화}}` `{{팩스}}` `{{이메일}}` `{{공개구분}}` |
| `gongmun-basic.md` | same as `gian-external.md` minus `{{접수번호}}` `{{접수일자}}` |
| `report.md` | `{{제목}}` `{{작성자}}` `{{작성부서}}` `{{작성일자}}` `{{배경1}}` `{{배경2}}` `{{내용1}}` `{{내용2}}` `{{내용3}}` `{{계획1}}` `{{계획2}}` `{{행정사항}}` `{{붙임}}` |
| `plan.md` | `{{사업명}}` `{{주관기관}}` `{{책임자}}` `{{사업기간}}` `{{총사업비}}` `{{참여기관}}` `{{배경1}}` `{{배경2}}` `{{필요성1}}` `{{필요성2}}` `{{최종목표}}` `{{연차목표}}` `{{추진전략}}` `{{세부과제1}}` `{{세부과제2}}` `{{세부과제3}}` `{{추진일정}}` `{{예산내역}}` `{{정량효과}}` `{{정성효과}}` `{{성과지표}}` `{{붙임1}}` `{{붙임2}}` |
| `minutes.md` | 9 statutory elements: 회의 명칭 / 개최기관 / 일시·장소 / 참석자·배석자 명단 / 진행 순서 / 상정 안건 / 발언 요지 / 결정 사항 / 표결 내용 |
| `notice.md` | `{{공고번호}}` `{{공고일자}}` plus the shared slots it uses (`{{제목}}` `{{본문}}` …) |
| `press.md` | `{{보도시점}}` `{{배포일}}` `{{담당부서}}` `{{담당자}}` `{{연락처}}` plus the shared slots it uses |

Authoring rules that keep slots fillable:

- **`{{본문}}` sits alone in its own paragraph.** Part splicing
  (`--set "본문=@body.md"`) block-replaces only when the anchor paragraph contains exactly
  `{{본문}}` and nothing else.
- **Slots are plain, unformatted, single-run text.** Never place a slot inside bold/italic
  spans or across a line break: `hwp fill` matches `{{name}}` by raw-XML string replace, so
  a slot split across text runs does not match. Symptom: `hwp slots` lists the name but
  `fill --set name=…` reports 0 replacements.

## 4. Korean alias table

Template files use English slugs; the Korean aliases below are what the Phase 2.4
`hwp new --template` flag will accept (D-11). Until then, reference the skeleton file
directly with `--from templates/<slug>.md`.

| Korean alias | Template slug |
|---|---|
| 기안문-내부결재 | `gian-internal` |
| 기안문-대외시행 | `gian-external` |
| 공문서-기본 | `gongmun-basic` |
| 보고서 | `report` |
| 사업계획서 | `plan` |
| 회의록 | `minutes` |
| 공고문 | `notice` |
| 보도자료 | `press` |

## 5. Fill and validate workflows

The canonical recipe — create, inspect, fill, validate — self-contained (no other guide
needed):

```bash
# 1. Create the document from a template skeleton
hwp new --from templates/gian-internal.md --preset official -o draft.hwpx

# 2. List the slots the document actually contains
hwp slots draft.hwpx

# 3. Fill plain values and splice the body part
hwp fill draft.hwpx -o out.hwpx \
  --set "기관명=예시대학교" \
  --set "제목=AI 교육센터 운영계획(안)" \
  --set "본문=@body.md" \
  --set "붙임=운영계획 상세(안) 1부" \
  --set "발신명의=AI학과장"

# 4. Validate before handing the file on (exit code 0 required)
hwp validate out.hwpx

# Optional: render pages to eyeball layout (needs CJK fonts)
hwp render out.hwpx -o page1.png --pages 1 --font-dir fonts/
```

`body.md` is markdown under the §1 contract (plus HTML table blocks when needed); `@@`
escapes a literal `@` in a value. Bulk filling from one JSON file uses
`hwp fill --data data.json` (`"parts": {...}` for splicing, `"tables": [...]` for row
fill). Remember: mutations are fail-closed — a `DROP:` warning means nothing is published,
and an unmatched slot fails the whole command unless `--allow-partial` is given.

One honest gap: fill matches slots by raw-XML string replace, so a `{{slot}}` split across
text runs is **not** replaced today (run-spanning fill is the EDIT-01 gap, scheduled for
Phase 2.5). Templates created through `hwp new --from` keep each slot inside a single run,
so the recipe above is unaffected.

## 6. MCP

Every recipe above maps one-to-one onto the MCP tools when `hwp` runs as an MCP stdio
server: `hwp new` → `hwp_new`, `hwp fill` → `hwp_fill`, `hwp render` → `hwp_render`,
`hwp validate` → `hwp_validate`, and `hwp lint` → `hwp_lint`. Always start the server
with at least one `--root` so the tools can only touch the sandboxed workspace; tool
details, sandbox semantics and client setup stay in SKILL.md's MCP section.

### Lint before you generate

`hwp_lint` takes a markdown file `path` only (no inline text), honors the server's
`--root` sandbox, and returns the advisory hwp-lint-report-v1 findings JSON —
`rule_id`/`severity`/`line`/`col`/`message` per finding, with no source-text excerpts.
The CLI twin is `hwp lint <file.md> [--profile gongmun|report] [--json] [--strict]`.

Lint the markdown skeleton before `hwp new --from`. Fix error-severity structure
findings (`struct-item-mark`, `struct-roman-heading`) first — under `--strict` they
fail the command — and treat the notation warnings as advisory guidance to apply by
hand; lint never rewrites the file.

## 7. Final check in Hancom Office

Self-verification (`hwp validate`, `hwp render`) catches regressions, but the final verdict
is whether **Hancom Office opens the file** without a corruption, tampering or repair
dialog — and whether what it shows matches the intent. Open the output in Hancom Office
before delivering it whenever the document leaves the tooling loop.
