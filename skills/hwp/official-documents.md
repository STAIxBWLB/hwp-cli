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

Body item marks come from **nested-list depth**, not from typed characters. Write plain nested
markdown ordered lists (`1.` at every depth) and indent each nested line by at least its marker
width. Three spaces after `1.` is the canonical form; the engine then assigns the mark for that
depth:

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

All six types share the same markdown skeleton under `templates/` (exported next to this
file). Two authoring paths reach it: `hwp new --template <slug-or-alias> -o out.hwpx` loads
the skeleton directly (§4 has the full alias table), or `hwp new --from <skeleton> --preset
<preset>` followed by `hwp fill` fills in slots one at a time. The recipes below use the
`--from` + `hwp fill` path since that is what needs the per-document notes; `--template` is
the shortcut when the defaults suit.

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
- **발신명의 via `--doc-foot`.** The centered 22pt bold 발신명의 line (e.g. `예시대학교총장`)
  is a native document-footer frame, built by `hwp new` itself — no HTML-fragment splicing
  needed. Pass it as a repeatable `key=value` frame argument at generation time:

  ```bash
  hwp new --from templates/gian-external.md --preset official \
    --doc-foot "발신명의=예시대학교총장" -o draft.hwpx
  ```

  `--doc-foot` also carries 기안자/검토자/결재자/협조자/시행번호/시행일자/접수번호/접수일자/
  주소/홈페이지/전화/팩스/이메일/공개구분/수신자 (repeat the flag once per key). 수신자 is the
  recipient list of a document whose 두문 reads `수신자 참조`, and is the one 결문 row emitted
  only when supplied. `--doc-head` carries
  기관명/수신/경유 the same way. `--notice-head`/`--notice-foot`/`--press-head` are the
  matching frames for 공고문/보도자료. Every frame is a table, wired directly into the
  document — nothing to splice, no HTML block-level alignment workaround.
- **관인(직인) is not stamped.** The e-approval system (온나라/K-Office) inserts the seal at
  dispatch; do not try to generate one.

## 3. Slot tables

Slot lists per template, verbatim from `hwp slots` run on a `hwp new --template <slug>` output
— do not re-derive. A slot lives either in the body markdown or in one of the template's native
frames, and `hwp slots`/`hwp fill` reach both, so the list below is the whole fillable surface. **A slot the template does not contain is an error**: `hwp fill`
fails closed and publishes nothing unless `--allow-partial` is given, so a shared `--set` list
needs `--allow-partial` to be reused across templates. Failing closed is deliberate — a typo'd
slot name that was silently ignored would dispatch a document with an empty field.

| Template | Slots |
|---|---|
| `gian-internal.md` | `{{기관명}}` `{{제목}}` `{{본문}}` `{{붙임}}` `{{발신명의}}` `{{기안자}}` `{{기안자직위}}` `{{협조자}}` `{{시행번호}}` `{{시행일자}}` |
| `gian-external.md` | `{{기관명}}` `{{수신}}` `{{경유}}` `{{제목}}` `{{본문}}` `{{붙임}}` `{{발신명의}}` `{{기안자}}` `{{검토자}}` `{{결재자}}` `{{협조자}}` `{{시행번호}}` `{{시행일자}}` `{{접수번호}}` `{{접수일자}}` `{{주소}}` `{{홈페이지}}` `{{전화}}` `{{팩스}}` `{{이메일}}` `{{공개구분}}` |
| `gongmun-basic.md` | the multi-recipient form: same as `gian-external.md` but with no `{{수신}}` (두문 reads the fixed `수신자 참조`) and a `{{수신자}}` list in 결문 |
| `report.md` | `{{제목}}` `{{작성자}}` `{{작성부서}}` `{{작성일자}}` `{{배경1}}` `{{배경2}}` `{{내용1}}` `{{내용2}}` `{{내용3}}` `{{계획1}}` `{{계획2}}` `{{행정사항}}` `{{붙임}}` |
| `plan.md` | `{{사업명}}` `{{주관기관}}` `{{책임자}}` `{{사업기간}}` `{{총사업비}}` `{{참여기관}}` `{{배경1}}` `{{배경2}}` `{{필요성1}}` `{{필요성2}}` `{{최종목표}}` `{{연차목표}}` `{{추진전략}}` `{{세부과제1}}` `{{세부과제2}}` `{{세부과제3}}` `{{추진일정}}` `{{예산내역}}` `{{정량효과}}` `{{정성효과}}` `{{성과지표}}` `{{붙임1}}` `{{붙임2}}` |
| `minutes.md` | 9 statutory elements: `{{회의명}}` `{{작성자}}` `{{주관}}` `{{일시}}` `{{장소}}` `{{참석자}}` `{{진행순서}}` `{{안건1}}` `{{안건2}}` `{{논의1}}` `{{논의2}}` `{{결정1}}` `{{결정2}}` `{{표결내용}}` |
| `notice.md` | `{{기관명}}` `{{공고번호}}` `{{제목}}` `{{본문}}` `{{공고일자}}` `{{발신명의}}` |
| `press.md` | `{{기관명}}` `{{보도시점}}` `{{배포일}}` `{{담당부서}}` `{{담당자}}` `{{연락처}}` `{{제목}}` `{{본문}}` `{{붙임}}` |

Authoring rules that keep slots fillable:

- **`{{본문}}` sits alone in its own paragraph.** Part splicing
  (`--set "본문=@body.md"`) block-replaces only when the anchor paragraph contains exactly
  `{{본문}}` and nothing else.
- **Keep a slot out of bold/italic spans anyway.** Formatting inside a slot name splits it
  across text runs. `hwp fill` handles that — it pulls the pieces back into the first run
  before replacing — but the value then inherits that first run's shape, which is rarely the
  shape you meant. Write `*{{name}}*`, never `{{na*me*}}`.
- **A slot may not cross a line break or a paragraph boundary.** Those genuinely end it:
  `{{` on one side and `}}` on the other is not a slot, and neither `hwp slots` nor
  `hwp fill` treats it as one.

## 4. Korean alias table

Template files use English slugs; `hwp new --template` accepts either the slug or the
Korean alias below (D-11), e.g. `hwp new --template 기안문-내부결재 -o draft.hwpx` or
`hwp new --template gian-internal -o draft.hwpx`. `--template` and `--from` are mutually
exclusive because both name the document content. The frame flags are a different axis and
do combine with `--template`: a skeleton is plain markdown carrying no native 두문/결문
table, so `--doc-head`/`--doc-foot` are what add them.

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

`hwp slots` and `hwp fill` agree about what a document contains: every name `slots` lists is
one `fill` can fill, formatting-split names included. A slot split across text runs is
coalesced into its first run before replacement, so the value takes that run's character shape.

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
