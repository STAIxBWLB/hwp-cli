---
name: hwp
description: Read, create, edit, convert, render and validate Hancom HWP 5.0 / HWPX documents with the ahwp CLI or its MCP stdio server. Use whenever a task touches .hwp or .hwpx files — extracting text, searching, filling templates, editing content, converting to docx/pdf/html/md/json/odt/txt/csv, or rendering pages to images. Also use for Korean official documents (공문) — 기안문, 보고서, 계획서, 회의록, 공고문, 보도자료 — their markdown contract, templates and 표기법 (notation rules) are covered by the official-documents sub-guide.
---

[한국어](SKILL.ko.md) · [English](SKILL.md)

# ahwp — HWP/HWPX document toolkit

`ahwp` is a single-binary toolkit for Hancom HWP documents. It reads and writes the binary
HWP 5.0 format and the XML-based HWPX format, and exports to docx, pdf, html, markdown, json,
odt, txt and csv. No Hancom Office installation is required.

English is canonical; `SKILL.ko.md` is the full Korean mirror and both are always exported
together.

Install: `brew install staixbwlb/hwp/ahwp`, or
`curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh | sh`.
`ahwp skill export [-o DIR] [--install claude-code|codex|amazon-quick]` materializes this
skill from the binary. Amazon Quick Desktop profiles can be selected with
`--quick-profile ID_OR_ABSOLUTE_PATH`.

## Command quick reference

Full flag reference: `ahwp {command} --help` (generated docs: docs/manual/cli-reference.md).
Output formats are inferred from the output extension unless `--to`/`--format` is given.
In the usage lines below, single-brace tokens such as `{file}` and `{output}` are placeholders
to replace with your own values. Only the doubled form `{{name}}` is literal HWPX template-slot
syntax (used by `fill`, `slots` and the template tools).

- `ahwp info {file} [--json]` — format, version, properties, stream list.
- `ahwp cat {file} [--format plain|markdown|json|html|csv]` — extract text. Useful flags:
  `--preview` (PrvText only, no body parse), `--with-header-footer`, `--with-hidden`
  (hidden comments), `--with-segments` (markdown + source coordinates as a one-line JSON
  envelope). `--format json` exports the full IR (tables, images, formatting). For a supported
  password-protected HWP5/HWPX file, prefer `--password-stdin`; `--password {value}` is also
  available when process-argument visibility is acceptable.
- `ahwp grep {pattern} {file} [--ignore-case]` — paragraph substring search over body, table
  cells and text boxes. grep convention: exit code 1 when nothing matches.
- `ahwp convert {inputs...} -o {output}` — format conversion (`--to hwp|hwpx|md|json|html|pdf|
  odt|txt|csv|docx`). `-` reads stdin / writes stdout (stdout for text formats only). Multiple
  inputs require `--out-dir {dir}`. `--strict` fails without publishing when unpreservable
  (opaque) data is found. PDF output delegates to the render path and needs CJK fonts:
  `--font-dir {dir}` (repeatable; default `HWP_FONT_DIR` or `fonts/`). Markdown export flags:
  `--media-dir`, `--with-header-footer`, `--with-hidden`, `--embed-bin` (json). Protected inputs
  accept the same `--password` / `--password-stdin` pair as `cat`.
- `ahwp merge {inputs...} -o {output}` — combine two or more documents into one, one Section per
  input in argument order. The writer comes from the output extension (`.hwp`/`.hwpx`); standard
  input `-` is not accepted. `--strict` fails without publishing when unpreservable (opaque) data
  is found; `--loss-report {file.json}` writes the typed `hwp-preservation-report-v1` ledger even
  on a clean run. Page, footnote and outline numbering keep each input's own start/continue
  settings, so re-check them after merging. One `--password` / `--password-stdin` covers the
  whole batch.
  Later HWPX picture item references resolve in their own source. Reused ids must not bind
  missing targets to another input's image; the first reference namespace stays unchanged
  during grafting, including its declared missing targets.
  Removed images are reported as `BinaryAssetRemoved`; `--strict` refuses their publication.
- `ahwp split {input} --out-dir {dir}` — divide one document into fragments, one per Section by
  default (named `{stem}-NNN.{lowercased ext}`). `--pages "N"|"N-M"` (repeatable) splits on page
  ranges instead; those boundaries come from the layout cache Hancom saved, so they are an
  estimate that may not match Hancom's own pagination. `--from` with `heading:TEXT`,
  `bookmark:NAME` or `at:SECTION.PARAGRAPH` (0-based), plus optional `--to`, `--end-exclusive` and
  `--source-sha256 HEX`, instead extracts ONE typed range: top-level body paragraphs of one section (a table is taken
  whole), with the section setup and every binary the range uses. A heading must equal the
  paragraph's own text as `ahwp cat` prints it (tabs, line breaks and non-breaking spaces count),
  so cell text is not an anchor, and an anchor matching twice is refused. Nothing
  is written on a refusal. `--range-report FILE` publishes `hwp-range-extraction-v1` (source
  hash, source-to-output addresses, kept, kept-unverified, excluded and dropped-by-format
  binaries) together with the fragment. A binary is excluded only when the fragment provably does
  not use it, so a reference the tool cannot read keeps them all and is listed. The report names
  anchors by kind and address, never by their text.
  `--strict` and `--loss-report` behave as
  in `merge`.
- `ahwp new -o {out.hwpx|out.hwp}` — create a document. `--from {file.md|file.json}` imports
  markdown or a JSON IR (empty document when omitted); `--set-meta key=value` (title/author/
  subject/keywords, repeatable); `--preset official|report|plan|notice|minutes|press`
  (Korean official-document profiles, markdown input only); `gian` and other documented
  compatibility aliases normalize to a canonical profile — the `gian` alias still works but
  prints a one-time deprecation note. Per-side overrides are
  `--margin-top`, `--margin-bottom`, `--margin-left` and `--margin-right` in millimetres.
  `--strict` fails when markdown import drops content.
- `ahwp edit {input} -o {output} [flags...]` — edit an existing document; images, formatting
  and unparsed records are preserved. String flags (all repeatable): `--replace "find=>repl"`,
  `--set-cell "t:r:c=value"` (0-based; a value split by blank lines writes one paragraph per
  block), `--set-field "name=value"`, `--set-meta "k=v"`,
  `--create-field "anchor=>name[=value]"`, `--create-bookmark "anchor=>name"`,
  `--create-hyperlink "anchor=>[text=>]URL"`, `--insert-image "anchor=>path[@WxH mm]"`,
  `--seal "anchor=>path[@size mm]"`, `--set-format "find:prop=value,..."`,
  `--set-align "find=left|right|center|justify|distribute"`. Structural flags (all
  repeatable): `--insert-para "anchor=>text"`, `--insert-para-before`, `--delete-para "text"`
  (the anchor is also searched inside table cells and captions),
  `--add-row "t[:at[:count[:template_row]]]"` / `--add-col "t[:at[:count]]"` (`at` omitted or
  `end` appends; a number inserts before that row/column; merged tables supported) /
  `--delete-row "t:r"` / `--delete-col "t:c"` /
  `--merge-cells "t:r1:c1:r2:c2"` / `--split-cell "t:r:c"` / `--add-table "anchor=>[[row],...]"` /
  `--clone-table "src=>anchor[=>blank|keep]"` (deep-copy table `src` after the anchor —
  blank keeps structure/styles with empty cells, keep also clones nested tables/images
  with remapped IDs) /
  `--delete-table "n|anchor"` / `--delete-image "anchor"` / `--delete-field "name"` /
  `--delete-bookmark "name"`, paragraph shape `--set-para "find=>key:value[,key:value]"`
  (line-spacing, indent, left/right/top/bottom mm, align),
  `--set-cell-para "t:r:c=>key:value[,key:value]"` (same keys, applied to every paragraph of
  the addressed cell with no text anchor; runs after `--set-cell` in one invocation)
  and page setup `--set-page "key:value"`
  (width/height/margin-*/orientation). `--verify` re-reads the output; `--allow-partial`
  relaxes the all-or-nothing rule (see Safety rules).
- `ahwp fill {template.hwpx} -o {output}` — fidelity-preserving `{{name}}` template fill
  (package preserved). `--set "name=value"` (repeatable); `--set "name=@part.md"` splices a
  part file (markdown + HTML table blocks) into the anchor paragraph — part-based composition
  for large documents (`@@` escapes a literal `@`; a name holding `=` goes through `--data`);
  `--data {file.json}` bulk fill
  (`"parts": {...}` splicing, `"tables": [...]` row fill); `--json` prints the summary;
  `--allow-partial` publishes the matched subset (nothing matched: the input unchanged, counts
  0). A slot name is any text without braces or control characters; `{{ name }}` padding
  and requested names are trimmed, and values are inserted literally. List slots first with
  `ahwp slots`.
  `--forms` also fills Korean form fields from the same values (hwpx): the cell next to or below
  a label cell, inline `라벨: 값`, `라벨(  )` blanks, `□옵션` checkboxes, `(라벨:  )` blanks;
  keys match with spaces, colons and parentheses ignored, and `--json` adds `unmatched`.
  A non-empty value cell and the text after `라벨:` are overwritten (each overwrite is warned);
  cells holding a field, picture or table are never written, and values are inserted literally.
- `ahwp compose {spec.json|yaml} -o {output}` — deterministic composition from DocumentSpec
  v1/v2. `--dry-run` validates without writing; `--report` prints the run report as JSON.
- `ahwp template {template} --data {data} -o {output}` — typed native HWP/HWPX generation from
  TemplateSpec/Data v1. `--dry-run` runs expansion + writer + validation without publishing;
  `--report` prints the preservation/expansion report as JSON.
- `ahwp render {input} -o {output.png|svg|pdf}` — render pages. `--pages "1"|"1-3"|"all"`,
  `--dpi 36..=600` (default 96), `--format png|svg|pdf`, `--font-dir {dir}` (repeatable).
  PNG/SVG write one file per page; PDF writes a single multi-page file. Needs CJK fonts.
  Protected inputs accept `--password` or `--password-stdin`.
- `ahwp fields {file} [--json]` / `ahwp bookmarks {file} [--json]` / `ahwp slots {file} [--json]`
  — list fields (name/kind/value), bookmarks (bokm), `{{name}}` template slots. `ahwp slots
  --forms` adds Korean form fields (label cells, inline `라벨: 값`) with key, label, source,
  confidence, occurrences and required.
- `ahwp styles {file} [--format text|json]` — list named styles: id, name, kind (`null` when the
  document omits it or gives an unsupported value), resolved char/para attrs and `used_by`, the `para`
  segment ids `ahwp cat --with-segments --segments v2` publishes. `--format json` is `hwp-styles-v1`.
- `ahwp validate {file} [--json]` — structural validation (mimetype, required entries, XML
  parsing); exit code 0 when valid.
- `ahwp dump {file} [--stream DocInfo|BodyText/Section0|Contents/header.xml] [--raw] [--json]`:
  low-level record and stream diagnostics for a single file. Reach for it only when `ahwp info`
  and `ahwp validate` do not explain a defect; it prints format internals, not document content.
- `ahwp lint {file.md|file.hwp|file.hwpx} [--json] [--strict] [--profile gongmun|report]` — ten
  official-document notation and structure rules (`-` reads stdin as markdown). Advisory: it
  always exits 0 unless `--strict` finds an error-severity finding. `--json` prints the
  `hwp-lint-report-v1` contract (`rule_id`/`severity`/`line`/`col`/`message`).
- `ahwp certify {input} --policy {policy.json|yaml} --report {dir}` — certify package,
  semantics, native render and independent import under a versioned policy; publishes the
  report directory atomically.
- `ahwp diff {input} --ref {hancom.png} [--page N] [--dpi N] [--tolerance N] [-o diff.png]` —
  compare a render against a Hancom reference PNG (offset, pixel difference).
- `ahwp compare {a} {b} [--format text|json]` — report paragraph and structural differences
  between two documents, leaving both untouched. This is not `ahwp diff`, which compares a render
  against a Hancom reference PNG. `--format json` prints the `hwp-compare-report-v1` contract.
  Its exit codes follow diff(1) — see Exit codes below.

## Official documents

`ahwp` ships the Korean official-document (공문) surface natively. Six profiles are available,
one per document type: `official` (기안문), `report` (보고서), `plan` (계획서), `notice`
(공고문), `minutes` (회의록) and `press` (보도자료). The canonical official profile accepts
`gian` and `gongmun` as semantic compatibility aliases; aliases select the same profile, not a
raw-byte identity promise, and the `gian` alias still works but prints a one-time deprecation
note. Korean aliases are also accepted. Each type is authored as a markdown
skeleton, created with `ahwp new --from ... --preset official` (or its matching profile), and
filled with `ahwp fill`.

개조식 is a writing style, not a profile: it is the noun-form sentence ending used inside
보고서·계획서 and 내부결재 bodies. Choose the profile by document type and apply the style in the
body text — see `references/korean-official-format.md` §6 어투.

Every profile uses top/bottom/left/right margins of 20/10/20/20 mm before an explicit per-side
override. Body defaults are: official Malgun Gothic 12pt/160%; report and plan HCR Batang
15pt/160%; notice Malgun Gothic 15pt/160%; minutes HCR Batang 14pt/130%; and press HCR Batang
14pt/160%. Report and plan use 15 mm header/footer margins; notice and press use 10 mm; official
and minutes use 0 mm. Report, plan, notice and press include `- N -` page numbers; official and
minutes do not.

Item marks come from nested-list depth: ordered lists render as `1.` → `가.` → `1)` → `가)` →
`(1)` → `(가)` → `①` → `㉮`. All eight depths are supported; depth 9 or deeper is rejected
without publishing. HWPX emits the matching numbering directly. HWP5 uses the verified safe,
direct encoding path for this official ladder, including the evidenced post-`하` continuation.

The □ ○ ladder is literal: type `□ ` and `○ ` as paragraph-leading symbols and the engine
indents them — never substitute ASCII lookalikes. `- ` list bullets render as `-` at depth 1
and `·` below.

Heading numbers are literal: type `Ⅰ. 1.` (Ⅰ = U+2160 full-width; ASCII `I.` is forbidden)
in the heading text — headings carry no automatic numbering.

A single item is a plain paragraph: when a list would contain exactly one item, write it as
an unmarked paragraph instead.

Never hand-type marks on the numbered path: write plain nested lists and let the engine
assign marks; a hand-typed `가.` inside an ordered list double-numbers.

Per-document recipes, slot tables, the Korean alias table, fill/validate workflows and the
Hancom final check live in `official-documents.md` (exported next to this file); regulation
background lives in `references/korean-official-format.md`.

## Editing recipes

The bilingual native editing migration crosswalk, including honest limits for retired `hwpx`
workflows, lives in [references/editing-recipes.md](references/editing-recipes.md). Use it for
analysis, anchor-based paragraph edits, data-driven table fill, label-value forms, and the
validate-plus-render guard sequence.

The analyze, edit-one-section and guard workflows below were run verbatim against `fixtures/samples/report-tables.hwpx`
(hwp 0.12.1, 2026-08-29) before being written down.

### Original-form workflow

When a native HWP/HWPX form is supplied, edit that form; use Markdown when reauthoring is
intended. Before starting, check `ahwp --version` against the
[versioned capability evidence](https://github.com/STAIxBWLB/agent-hwp/blob/main/skills/hwp/references/original-form-capabilities-v1.json).
The complete #417-#420 workflow first ships in v1.5.0; v1.4.0 and earlier releases do not support
`document.original_form` certification.

Follow the [canonical original-form workflow](https://github.com/STAIxBWLB/agent-hwp/blob/main/skills/hwp/references/editing-recipes.md#original-form-workflow):

1. Discover the source identity and exact occurrences; extract the native range with its resource
   and address report, then discover the extracted source again.
2. Review explicit bindings, copies, numeric constraints and growth budgets; dry-run, inspect the
   local preview, then fill/repeat against the same reviewed source bytes.
3. Refresh addresses, apply style and pagination, and repeat indentation to check idempotence.
4. Validate and certify the delivered artifact with pinned fonts and reviewed expectations;
   inspect rendering, then obtain separate person-run Hancom reopen/export evidence before any
   native-compatibility claim.

Use JSON files with `--ops OPS.json` (or `--ops -`) for literal values containing delimiters or
leading hyphens. Keep discovery text, plans and exact previews local; retain extraction, binding
and certification receipts. An empty loss ledger or `render-report.complete` does not establish
visual correctness. Read the canonical recipe before manipulating IR character arrays or
exporting resources; self-contained JSON requires `convert --to json --embed-bin`.

For MCP, `hwp_read` has legacy markdown `with_segments` only, not CLI segments v2. Use advertised
addresses; send one `apply_form_bindings` object, not an array, and no `verify` property.
MCP `insert_para` has no inline `style`/`char`: refresh the inserted heading's address, then use
an advertised `apply_style` request. MCP `hwp_render` has no `report`/`layout_json` arguments;
export typed layout/report artifacts through the CLI. MCP `hwp_certify` still publishes its own
policy/render evidence.

If this installation contains only `SKILL.md` (Quick/web), use the absolute recipe link above,
or run `ahwp skill export -o ./hwp-guide` to materialize the bundled references and capability JSON.
Full exports also provide the [local recipe](references/editing-recipes.md#original-form-workflow).

### Analyze a document

One "what is in this document" pass: package inventory, then text with source coordinates,
then the programmatic handles (fields and template slots):

```bash
ahwp info doc.hwpx --json
ahwp cat doc.hwpx --format markdown --with-segments > doc.segments.json
ahwp fields doc.hwpx --json
ahwp slots doc.hwpx --json
```

`--with-segments` prints a one-line JSON envelope `{"markdown": ..., "segments": [...]}` where
each segment is `{"kind": "para", "section": N, "para": N, "start": N, "end": N}` and
start/end are character offsets into the markdown string. `fields` and `slots` legitimately
return `[]` when the document has no fields or `{{slot}}` placeholders — empty is an answer,
not an error.

### Edit one section

Segments locate the paragraph; the edit itself anchors on the visible text of that range —
there is no raw `sec` index contract. Work on a copy until the result passes validation:

```bash
ahwp cat doc.hwpx --format markdown --with-segments > doc.segments.json
ahwp edit doc.hwpx -o edited.hwpx --insert-para "anchor text=>new paragraph" --verify
ahwp edit edited.hwpx -o reverted.hwpx --delete-para "new paragraph" --verify
ahwp validate edited.hwpx
```

Read the `(section, para)` coordinate of the segment whose markdown range contains the target
text, then use that range's plain text (without markdown markers) as the anchor.
`--insert-para "anchor=>text"` inserts after the anchor paragraph (`--insert-para-before`
inserts before it); `--delete-para "text"` removes a paragraph by its text. Both are
all-or-nothing by default and re-read the output under `--verify`.

### Guard an edit

Before/after structural-drift check: structural validation plus renderer page counts.

```bash
ahwp validate edited.hwpx
ahwp render doc.hwpx -o before.png --report before.render.json
ahwp render edited.hwpx -o after.png --report after.render.json
```

Compare `total_pages` in the two render reports (the report also carries `font_coverage` and
`font_resolution_complete`; rendering needs CJK fonts, and page counts depend on the fonts
available). A changed page count is a review signal, not proof of content drift: an IR
round-trip alone can reflow layout — verified on the fixture, where a plain
`ahwp convert --to hwpx` round-trip rendered 6 pages before and 5 after while the
byte-preserving `--replace` fast path kept the original count. When layout must stay
untouched, prefer `--replace`-only edits (the package-preserving path) and treat any
page-count change as a reason to inspect the rendered pages, not to weaken validation.

### Restyle named paragraphs

List the current styles and paragraph addresses before editing:

```bash
ahwp styles doc.hwpx --format json
ahwp cat doc.hwpx --format json --with-segments --segments v2 > doc.segments.json
```

Select a style with `kind: "paragraph"`. In this example, `body` is an existing primary style
name and `at: {section: 0, paragraph: 1}` is the chosen paragraph; replace both with values from
this document. An address can instead be `{"id":"CURRENT_PARA_SEGMENT_ID"}` from v2 or a
style's `used_by`. IDs belong to the source snapshot: obtain fresh ones after editing.

Save this array as `restyle.json`:

```json
[
  {"op":"create_style","name":"Body restyled","based_on":"body","kind":"paragraph","size":"13pt"},
  {"op":"apply_style","address":{"at":{"section":0,"paragraph":1}},"style":"Body restyled","clear_direct":false}
]
```

```bash
ahwp edit doc.hwpx -o styled.hwpx --ops restyle.json
```

The same array can be piped to `ahwp edit doc.hwpx -o styled.hwpx --ops -`; named-style
operations have no dedicated flags. MCP executes named-style arrays in create/update/apply
order, before the other edit arrays. Use the ordered `--ops` array for a different interleaving.

To update the new style's users together, save
`[{"op":"update_style","style":"Body restyled","size":"14pt"}]` as `update-style.json`, then run
`ahwp edit styled.hwpx -o updated.hwpx --ops update-style.json`. Style selectors accept an exact
primary name or an integer ID. `update_style` allocates changed definitions with copy-on-write;
other styles and direct formatting keep their own shapes. `apply_style` preserves direct
character and paragraph formatting by default; `clear_direct: true` explicitly resets both to
the target style. These operations currently edit paragraph styles, not character styles.

Equivalent `hwp_edit` MCP arguments for the two calls are:

```json
{"input":"doc.hwpx","output":"styled.hwpx","create_style":[{"name":"Body restyled","based_on":"body","kind":"paragraph","size":"13pt"}],"apply_style":[{"address":{"at":{"section":0,"paragraph":1}},"style":"Body restyled","clear_direct":false}]}
```

```json
{"input":"styled.hwpx","output":"updated.hwpx","update_style":[{"style":"Body restyled","size":"14pt"}]}
```

Repeated identical apply/update requests succeed without a semantic change. Repeating creation
refuses the duplicate name atomically. Stale IDs also refuse before publication. If an affected
user is inside raw-preserved records or XML that cannot be regenerated, the edit refuses with
locations; use `ahwp styles` to select another style or repair the source first.

```bash
ahwp validate updated.hwpx
ahwp certify updated.hwpx --policy policy.json --report ./style-certification
```

Supply a policy appropriate for the document and a report directory that does not exist.
`certify` currently requires `--policy`; there is no implicit default-policy command to copy.
Read the report and inspect rendering when appearance matters. A `native_only` pass checks the
native implementation under that policy; it does not attest that Hancom opened or rendered the
file. Hancom-open evidence is a separate receipt. The same workflow accepts `.hwp` paths; output
format constraints and refusal diagnostics still apply.

### Insert a component

Take the paragraph address from `ahwp cat doc.hwpx --format json --with-segments --segments v2`.
Save the blocks (DocumentSpec v1: paragraph, table, image, field, equation, page or column
break) as `component.json` and run it with `--report`:

```json
[{"op":"insert_blocks","address":{"at":{"section":0,"paragraph":2}},"position":"after",
  "blocks":[{"type":"paragraph","runs":[{"type":"text","text":"Approval:"}]},
            {"type":"image","path":"logo.png","width_mm":20}]}]
```

```bash
ahwp edit doc.hwpx -o with-component.hwpx --ops component.json --report report.json
```

- Style names resolve against the document first; a missing one is created from `styles`
  (`missing_style: "create"`, the default) or refused (`"refuse"`). Lists need a `lists` entry.
- Image paths are relative to the `--ops` file's directory; `--ops -` refuses images. Over MCP
  pass `base_dir` with the `insert_blocks` array of `hwp_edit` (items are the op without `op`).
- Blocks are measured against the column or table cell they land in (landscape pages and
  multi-column sections included) and an oversized one is refused before anything is written.
- Equations insert into HWPX only. Section breaks are not supported. Any refusal writes nothing.
- The report's `changed` for this op lists the inserted paragraph ids, each followed by its run
  ids (and table, cell and image ids). Pass a run id to `set_format` without guessing indices:
  `[{"op":"set_format","address":{"id":"RUN_ID_FROM_THE_REPORT"},"bold":"on"}]`. Ids describe the
  document right after that op.

### Review and apply native form bindings

Before binding supplied forms, read the [canonical binding recipe](https://github.com/STAIxBWLB/agent-hwp/blob/main/docs/manual/agent-conventions.md#review-and-apply-native-form-bindings) for the closed plan schema, copy maps and refusal boundaries.

```bash
ahwp slots source.hwpx --json --forms --occurrences > local-catalog.json
ahwp edit source.hwpx -o bound.hwpx --ops OPS.json --verify --dry-run
ahwp edit source.hwpx -o bound.hwpx --ops OPS.json --verify
ahwp validate bound.hwpx
```

- Build and review one exclusive `apply_form_bindings` request in `OPS.json` from that exact source catalogue. Select explicit nonoverlapping occurrence IDs and per-target budgets; do not infer ownership from repeated labels.
- Keep the catalogue and optional exact preview local. Use the mandatory content-free receipt; do not combine this operation with `--report` or `--allow-partial`.
- Inspect the dry-run preview and copy maps before applying. Replay revisions against the same original bytes; discover and review again after source edits or conversion.
- Use `ahwp fields` for inventoried native fields. Do not bypass raw/rich/caption refusals with broad text replacement.
- For MCP, use `hwp_slots` with `forms`/`occurrences` true and `hwp_edit` with one `apply_form_bindings` object, following the same recipe and advertised schemas. Do not send a `verify` property.
- Collect render/font and person-run Hancom evidence separately under #419; the semantic receipt does not establish it.

### Page breaks and typed indentation

Use `set_break_before` (`value` is `on` or `off`) to start one body paragraph on a new page and
`set_indent_level` (`level` 0 to 7, `step`, optional `hanging`, such as `5mm`) to replace
leading spaces with real indentation. Both take a paragraph `address` from
`ahwp cat doc.hwpx --format json --with-segments --segments v2`:

```json
[{"op":"set_break_before","address":{"at":{"section":0,"paragraph":5}},"value":"on"},
 {"op":"set_indent_level","address":{"at":{"section":0,"paragraph":7}},
  "level":2,"step":"5mm","hanging":"3mm"}]
```

- A paragraph in a table cell and a section's first paragraph are refused for the page break,
  with the cause. Repeating either op is a successful no-op. A real change drops the line
  layout of that paragraph and the ones after it up to the next break so they are re-measured.
- Put `set_format` run edits BEFORE `set_indent_level` or `replace` on the same paragraph in a
  batch: a run address that follows a length change is refused with nothing written.
- `set_indent_level` strips only the leading blanks (also those after a section, field or
  bookmark control, which stays), never inner spacing or a numbering label. The first remaining
  character keeps its bold or other character shape. A blank-only paragraph is left as is. A paragraph with highlight or proofreading marks is
  refused when blanks would go: remove the marks first, or use `set_para` with `left_mm`.
- `keep_together` (keep with next, keep lines) is always refused: no Hangul-saved sample shows
  those bits. Do not retry it. Use `set_break_before`, or ask for a sample saved from Hangul.
- Over MCP use the `set_break_before` and `set_indent_level` arrays of `hwp_edit`.

## MCP server

`ahwp mcp` speaks synchronous JSON-RPC 2.0 over stdio (line-delimited; stdout carries the
protocol, logs go to stderr). Protocol version is negotiated at `initialize`: a client
`protocolVersion` of `2025-06-18`, `2025-03-26` or `2024-11-05` is echoed back, anything else
gets the latest supported version.

Always start it with at least one sandbox root:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "ahwp",
      "args": ["mcp", "--root", "/path/to/workspace", "--font-dir", "/path/to/fonts"]
    }
  }
}
```

`--root {dir}` (repeatable) restricts every file path the tools touch — inputs, outputs,
nested image/part paths, spec `base_dir`s, per-call `font_dir`s and the certify report
directory — to the given directories. Roots are canonicalized at startup; a missing or
unreadable root fails fast. Without any `--root` the server is unrestricted and prints a
one-line warning to stderr at startup — prefer `--root` whenever the client allows it.

Amazon Quick Desktop on Windows starts local MCP children at Low mandatory integrity
(`S-1-16-4096`). Use a dedicated child of the Windows `LocalLow` directory and
`C:\Windows\Fonts`, with separate JSON arguments:
`["mcp", "--font-dir", "C:\\Windows\\Fonts", "--root",
"C:\\Users\\YOUR_NAME\\AppData\\LocalLow\\hwp-quick-workspace"]`. Replace `YOUR_NAME`
with the actual account folder before configuring Quick or calling a tool; neither the argument
list nor MCP paths expand `%USERPROFILE%`. Create the root under `AppData\LocalLow` so it inherits
the Low mandatory label. Do not use ordinary `C:\TEMP` or `%LOCALAPPDATA%\Temp` as a write root:
discovery may work there while `hwp_new` fails to create its private staging directory with
`Access is denied (os error 5)`.

A folder added to Quick's local-folder permissions is available to its built-in read/search tools
but does not become writable by the Low-integrity MCP child. Keep all MCP inputs and outputs under
the configured `LocalLow` root, use Quick's file tools or Explorer to copy artifacts into and out
of it, and re-enable the connector if repeated startup failures caused Quick to auto-disable it.

When helping someone configure Quick, prefer JSON import and never put shell quote characters
inside an argument. Verify three layers in order: the exact absolute binary returns a version;
Quick reports 23 tools and stays enabled after refresh; then `hwp_new` followed by `hwp_validate`
succeeds on an absolute path under the configured LocalLow root (for example
`C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\quick-hwp-smoke.hwpx`). Do not claim
that discovery alone proves file access.
After a connector edit, refresh or start a new chat instead of reusing an old generated tool prefix.
The copy-paste operator and AI runbook is:
`https://github.com/STAIxBWLB/agent-hwp/blob/main/docs/manual/amazon-quick-desktop.md`.

Tools (23):

| Tool | Required arguments | Purpose |
|---|---|---|
| `hwp_info` | `path` | Format, version, properties and stream diagnostics |
| `hwp_read` | `path` | Extract text (`format`: plain/markdown/json/html/csv; optional per-call `password`; `with_header_footer`, `with_hidden`, `with_segments`); UTF-8 byte pagination |
| `hwp_grep` | `path`, `pattern` | Paragraph substring search; `{matches, count, truncated}`; zero matches is a normal result |
| `hwp_list_fields` | `path` | List fields |
| `hwp_list_bookmarks` | `path` | List bookmarks (bokm) |
| `hwp_styles` | `path` | List named styles with resolved attributes and `used_by` segment ids, as an `hwp-styles-v1` listing |
| `hwp_slots` | `path` | List `{{name}}` placeholders |
| `hwp_render` | `path` | Render pages (optional per-call `password`; `format`: png/svg/pdf, `pages` range); single-page PNG returns base64, larger results write files via `output_path` |
| `hwp_edit` | `input`, `output` | Strict atomic editing through typed JSON operations (mirrors every `ahwp edit` flag, incl. `add_table`, `clone_table`, `set_para`, `set_page`, `delete_*`) |
| `hwp_convert` | `input`, `output` | Format conversion (optional per-call `password`; `strict` defaults to true over MCP) |
| `hwp_new` | `output` | Create a document from markdown or JSON IR plus metadata, official profile and `margin_top`/`margin_bottom`/`margin_left`/`margin_right` overrides |
| `hwp_compose` | `output`, `spec`/`spec_path` | Compose DocumentSpec v1/v2 through the same path as the CLI |
| `hwp_template` | `output`, `template` (+`data`) | Bounded expansion of TemplateSpec/Data v1 |
| `hwp_fill` | `input`, `output`, `values` | Fill `{{name}}` in an hwpx template (package preserved) |
| `hwp_validate` | `path` | Structural validation, `{valid, errors, warnings}` |
| `hwp_lint` | `path` | Advisory official-document notation/structure lint on a markdown file (path only, `--root` sandboxed); returns the hwp-lint-report-v1 findings JSON (`rule_id`/`severity`/`line`/`col`/`message`) |
| `hwp_certify` | `input`, `policy`, `report` | Run certification and publish the report atomically |
| `hwp_diff` | `input`, `ref` | Render one page and compare it to a reference PNG |
| `hwp_merge` | `inputs`, `output` | Combine two or more documents, one Section per input; returns the preservation ledger |
| `hwp_split` | `input`, `out_dir` | Split into per-Section (or `pages`) fragments, or extract one typed range with `from`/`to`; returns the published fragment paths and, for a range, the `hwp-range-extraction-v1` report |
| `hwp_compare` | `a`, `b` | Read-only paragraph/structure diff returning `hwp-compare-report-v1`. Differences are a normal result and never set `isError` — read `identical` |
| `hwp_put_file` | `name`, `content` | Write base64 content into the session workspace. The only way to hand an existing document to a remote deployment: tool arguments take paths, so upload first and pass the name. 512 KiB decoded |
| `hwp_get_file` | `path` | Return a workspace file as base64 (a receipt block plus an embedded resource). Refuses above 512 KiB rather than truncating. Keep intermediates in the workspace and fetch only the final artifact - the base64 lands in the client's message stream |

## HTTP mode for containers

`ahwp serve` speaks the same protocol as `ahwp mcp`, over HTTP instead of stdio, so the same
23 tools are reachable from a container. It is meant to sit behind a trusted edge that has already
terminated TLS, authenticated the caller and capped the request body — not to face the internet
directly.

```bash
ahwp serve --addr 0.0.0.0:8080 --root /work --font-dir /usr/share/fonts/truetype/nanum
```

| Route | Behavior |
|---|---|
| `POST /mcp` | One JSON-RPC message, capped at 1 MiB. A request answers `200 application/json`; a notification answers `202` with no body |
| `GET /mcp` | `405` — the server never pushes, so no event stream is offered |
| `GET /healthz` | `200` once the listener is bound; container platforms probe this |
| `POST\|GET /files/{name}` | Only with `--files`. Names must match `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`; 64 MiB per file, 256 MiB per workspace |

`--root` is mandatory here, unlike `ahwp mcp` where omitting it only warns: a remote deployment must
never run with unrestricted filesystem access. An inbound `Mcp-Session-Id` is accepted and ignored,
because session affinity belongs to the platform in front.

`ahwp serve` stops on SIGTERM and SIGINT, finishing the request in flight first, so a platform that
stops idle containers that way can stop it even when it runs as PID 1 — where the kernel delivers
no signal that still has its default disposition. A second signal exits immediately. If you pin a
release older than this behavior, run it under an init such as `tini` or with `docker run --init`.

## Safety rules (must follow)

1. **Validate after every write.** After `new` / `edit` / `fill` / `compose` / `template` /
   `convert` to hwp/hwpx (or the MCP equivalents), run `ahwp validate {output}` (or
   `hwp_validate`) and check exit code 0 before handing the file on.
2. **Mutations are fail-closed.** Authoring commands (`new`, `edit`, `fill`) treat `DROP:`
   warnings — content that cannot be preserved in the output — as hard failures and do not
   publish. A failed command leaves any existing output file untouched.
3. **Atomic publish.** Outputs are written to a private staging workspace next to the
   destination, verified, then swapped in. There is no partial or half-written output state;
   on failure the previous file is preserved (and restored on rollback).
4. **All-or-nothing by default.** `edit` and `fill` fail the whole command when any requested
   change found no target (unapplied edit / unreplaced placeholder). `--allow-partial`
   (or the MCP `allow_partial` argument) publishes only the matched subset — use it
   deliberately, and re-check the result.
5. **Prefer `--root` for MCP.** Scope the server to the working directory (repeat the flag
   for every directory the tools may legitimately touch) instead of running unrestricted.
6. **Keep passwords invocation-local.** Prefer CLI `--password-stdin` over `--password`, and pass
   MCP passwords only on the individual call that needs one — `hwp_read`, `hwp_convert`,
   `hwp_render`, `hwp_merge`, `hwp_split` or `hwp_compare`, the only six tools that accept the
   argument. Never put a credential in a report, receipt, generated file, command transcript or
   persistent environment.
7. **Read exit codes by their command's convention.** They deliberately differ, so a single
   "non-zero means failure" reading is wrong:

   | Command | Convention |
   |---|---|
   | `compare` | diff(1): 0 identical, 1 differences found, 2 the run itself failed |
   | `lint` | always 0; `--strict` exits 1 only on an error-severity finding |
   | `grep` | 1 when nothing matched (a normal result, not an error) |
   | `validate`, `new --strict`, `convert --strict`, `merge --strict`, `split --strict` | 0 on success, non-zero on failure |

   MCP has no exit codes: `hwp_compare` returns `identical` and `hwp_grep` returns `count`, both
   with `isError` false.
8. **Read the preservation ledger after a document-level write.** `convert`, `merge` and `split`
   record every unpreservable item in the typed `hwp-preservation-report-v1` ledger — written to
   `--loss-report {file.json}` on the CLI, and returned in the `preservation` field of every
   `hwp_merge` / `hwp_split` response. A merge always drops the package passthrough of every
   input after the first, so `--strict` is opt-in on both commands rather than the default:
   check the ledger instead of assuming a clean run.
