[한국어](agent-conventions.ko.md) · [English](agent-conventions.md)

# Agent conventions

The conventions every client follows when it drives `ahwp`, over the CLI or the MCP server: workflows, refusals, the preservation ledger, exit codes, passwords and environment variables. For agents and the people writing their instructions. Client setup is in [ai-integrations.md](ai-integrations.md).

## Conventions every client needs

These hold for the CLI and the MCP server alike, whichever client drives them.

### Original-form workflow

Prefer the supplied native HWP/HWPX form when completing an existing form. Choose Markdown
when reauthoring is intended. The complete #417-#420 workflow described here first ships in
v1.5.0; v1.4.0 and earlier releases do not include `document.original_form`
certification. Check the binary version and
[versioned capability/limitation evidence](../../skills/hwp/references/original-form-capabilities-v1.json)
before selecting the workflow.

Use the [canonical original-form recipe](../../skills/hwp/references/editing-recipes.md#original-form-workflow)
for the concrete commands, reviewed JSON inputs and bounded evidence:

1. Discover the source hash, occurrences and addresses; extract the chosen native range with
   resource closure and retain the source-to-output map.
2. Rediscover the extracted source, review explicit claims and copy targets, dry-run the binding
   plan and inspect its local preview. Apply fill/repeat atomically to those same source bytes.
3. Replay revised allocations against that reviewed source with explicit numeric constraints and
   caller-supplied totals. Refresh addresses before styling/pagination; apply indentation twice
   and inspect the no-op comparison.
4. Validate, render under pinned fonts and certify the delivered artifact against reviewed cell
   values, table dimensions and form starts. Retain source/plan/output/font/render identities.
5. Reopen and export the exact delivered file in Hancom through a person-run check; retain its
   separate receipt before claiming native compatibility.

Use structured `--ops` JSON rather than delimiter-based flags for arbitrary text, including
leading hyphens. Discovery catalogues, expectation plans and exact previews contain document
content and stay local; default receipts identify results without exposing those values. The
canonical recipe also distinguishes the IR character array from a plain string and explains why
`convert --to json --embed-bin` is required for self-contained resource export.

For MCP, use `hwp_split`, `hwp_slots`, `hwp_edit`, `hwp_render` and `hwp_certify` in the same staged
order. `hwp_read` offers only legacy markdown `with_segments`, not CLI segments v2 or JSON with
segments. Obtain fresh advertised addresses; `apply_form_bindings` is one object, not an array,
and has no `verify` property. Do not combine that exclusive binding operation with style edits.
MCP `insert_para` does not expose the CLI's inline `style`/`char` fields. Refresh the inserted
heading's address, then apply a named style through a separate advertised `apply_style` request.
MCP `hwp_render` has no `report` or `layout_json` arguments: typed layout/report artifact export
is CLI-only. MCP `hwp_certify` still publishes its own policy/render evidence in the report directory.

Read each policy result and its limitations. Structural validity, preservation, complete render
diagnostics, visual acceptance and person-run Hancom evidence remain separate. A bounded native
render or PDF success is evidence only for the tested fixture, font environment and selected
policy. Quick/web installations that start with only `SKILL.md` can materialize the full bundled
recipe and capability JSON with `ahwp skill export -o ./hwp-guide`.

### Document-level workflows

Since v0.13.0 the toolkit works on whole documents, not just one at a time:

- `ahwp merge {inputs...} -o {out}` / `hwp_merge` — concatenate two or more documents, one Section
  per input in argument order. Page, footnote and outline numbering keep each input's own
  start/continue settings, so re-check them afterwards.
  Later HWPX picture item references keep their own source meaning; reused ids do
  not make a missing target resolve to another input's image. The merge retains the first
  manifest's reference namespace; later inputs cannot supply its declared missing targets.
  Native output reports removed images as `BinaryAssetRemoved`;
  use `--strict` to refuse publication when that loss is unacceptable.
- `ahwp split {in} --out-dir {dir}` / `hwp_split` — one fragment per Section by default; `--pages`
  splits on page ranges instead, estimated from the layout cache Hancom saved rather than
  recomputed, so it may not match Hancom's own pagination. `--from`/`--to` (`from`/`to` in
  `hwp_split`) extract one typed range instead, anchored on a heading (the paragraph's own text as `ahwp cat` prints it), a bookmark or a
  `section.paragraph` address, with the binaries the range uses. The `hwp-range-extraction-v1`
  report of where each paragraph and table went and which binaries were kept, kept unverified,
  excluded or dropped by the output format (HWP5 writes only the binaries its BIN_DATA table
  names) is written only when you pass `--range-report {file.json}`; `hwp_split` always returns
  it in its `range` field. A binary is excluded only when the whole fragment provably does not use
  it: when the fragment holds a reference the tool cannot read (an image fill, a bullet that may be an
  image bullet, a raw shape, OLE or video object, a carried package part that is not readable XML), every binary travels and the report
  lists those sites. This includes settings/version XML, unreadable XML attributes, truncated
  reference records and unknown raw tags. Keep the input stable during extraction: the source
  hash checks before and after parsing cannot detect an A-B-A file replacement.
  The report is content-free: anchors appear as a kind and a resolved address,
  never as the heading or bookmark text. The fragment carries the section's page, column, header,
  footer, page-number and new-number controls. When the range starts after the section's first
  paragraph, a hide or odd/even adjust control on that paragraph acts on its own source page, so
  it stays behind; a range that starts at the first paragraph takes it whole.
- `ahwp compare {a} {b}` / `hwp_compare` — paragraph and structural differences between two
  documents, leaving both untouched. This is not `ahwp diff`, which compares a render against a
  Hancom reference PNG.

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

### Replace one run or one range instead of every match

A `replace` with a paragraph address rewrites every match of `from` inside that paragraph, so it
only serves a caller that knows `from` occurs exactly once there. To rewrite one selected run, or
one span inside it, address the run instead. A `run` segment of the v2 envelope carries the
`source_chars` window it occupies in the source paragraph, in absolute UTF-16 code units — use
those numbers directly as the address's `chars`:

```bash
ahwp cat doc.hwpx --format json --with-segments --segments v2 > doc.segments.json
```

```json
[{"op":"replace","address":{"id":"RUN_ID_FROM_THE_ENVELOPE","chars":[12,17]},"from":"선택 구간","to":"{{slot}}"}]
```

`at.run` with no `chars` rewrites the named run's whole text:

```json
[{"op":"replace","address":{"at":{"section":0,"paragraph":3,"run":1}},"from":"기존 텍스트","to":"새 텍스트"}]
```

Either form touches only the named run or window: the same phrase elsewhere in the paragraph,
across a run boundary or inside a field value is left alone. `from` is a drift check, not a
selector — when the addressed text is not exactly `from`, nothing is rewritten, the request is
reported unapplied with the address and its window, and the run fails unless `--allow-partial`.
A run id and a nested-paragraph id share one `<checksum>.<path>` shape, so a run named by `id`
carries `chars`; `at.run` names a run without a window.

In a batch the address resolves against the document as loaded, so a run reading follows its own
paragraph when an earlier op inserts or deletes one above it. It does not survive a change to that
paragraph's own length: an earlier `replace` whose `from` and `to` differ in width is rejected
during preflight, and an earlier `set_indent_level` stripping blanks is refused with nothing
written. Put the run reading first in the batch, or run the two edits separately.

### Insert a component into an existing document

`insert_blocks` places DocumentSpec v1 blocks (paragraphs, lists, tables, images, fields,
equations, page and column breaks) before or after an addressed paragraph, styled like the
document. Take the paragraph address from `ahwp cat doc.hwpx --format json --with-segments
--segments v2`, then save the component as `component.json`:

```json
[
  {"op":"insert_blocks","address":{"at":{"section":0,"paragraph":2}},"position":"after",
   "blocks":[
     {"type":"paragraph","runs":[{"type":"text","text":"Approval:"}]},
     {"type":"table","columns":[{"width_mm":40},{"width_mm":40}],
      "rows":[{"cells":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"Drafted"}]}]},
                       {"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"Approved"}]}]}]}]},
     {"type":"image","path":"logo.png","width_mm":20}
   ]}
]
```

```bash
ahwp edit doc.hwpx -o with-component.hwpx --ops component.json --report report.json
```

Rules that matter:

- A style name in a block resolves against the document's own styles first. A name the document
  lacks is created from the op's `styles` (`missing_style: "create"`, the default) or refused
  (`"refuse"`); the report states the policy. A list name needs a definition in `lists`.
- Image paths are relative to the directory of the `--ops` file and may not leave it. Reading
  `--ops -` from stdin has no directory, so image blocks are refused there. Over MCP, pass
  `base_dir` (a sandbox-checked directory) and the server's `--root` sandbox binds every asset.
- Tables and images are measured against the text column or table cell they land in: the rotated
  page of a landscape section, one column of a multi-column section, and inside a cell the cell
  width less its margins. `position: "before"` uses the layout preceding the address. An oversized
  block is refused before anything is written.
- Equations insert into HWPX only. The HWP writer has no equation record, so HWP output refuses
  them with a message naming the HWPX remedy. Section breaks are not supported.
- The edit is atomic: any refusal leaves the output unwritten, even with `--allow-partial`.

The report's entry for the op lists the ids of what was inserted, so the next call does not have
to guess indices. `pieces_touched` is the number of inserted paragraphs; `changed` holds one
creation pair (`before` is null) for every segment `ahwp cat` would publish inside them: each
paragraph id, then its run ids, and for a table or image block the table, cell and image ids.
If a very large component publishes more than the report's 10,000-pair cap, `changed` holds
the first 10,000 in document order and `changed_total` gives the real count; the omitted ids are
the ones `ahwp cat` shows. A run id is what `set_format` takes:

```json
[{"op":"set_format","address":{"id":"RUN_ID_FROM_THE_REPORT"},"bold":"on"}]
```

Ids describe the document right after that op. Obtain fresh ones after any later edit.
`ahwp validate` and `ahwp certify` apply to the result as to any document; a field block makes
the native render report an omitted control, which is a renderer limit, not an insertion fault.

Equivalent `hwp_edit` MCP arguments (the item is the op without its `op` tag):

```json
{"input":"doc.hwpx","output":"with-component.hwpx","base_dir":"/work/assets","insert_blocks":[{"address":{"at":{"section":0,"paragraph":2}},"position":"after","blocks":[{"type":"image","path":"logo.png","width_mm":20}]}]}
```

### Review and apply native form bindings

Discover exact occurrences on the source that will actually be edited:

```bash
ahwp slots source.hwpx --json --forms --occurrences > local-catalog.json
```

The version 1 catalogue contains `source_sha256`, the parsed-IR `structural_sha256` and
source-bound occurrence IDs. It contains document text: keep it local for explicit review.
Cell paragraphs are inventory targets; neighboring labels are evidence, not automatic value
ownership. Select explicit, nonoverlapping bindable claims. Captions, raw-preserved/rich targets
and native named fields remain visible but unsupported; native fields direct callers to
`ahwp fields`. Do not replace that refusal with a broad text replacement.
`label_occurrences` counts exact same-kind label matches and `label_ambiguous` marks repeated
labels. These expose ambiguity for review; they never select one occurrence automatically.

Save the following request shape as `OPS.json`, replacing the example hashes and occurrence ID
with the exact catalogue values, selecting the intended claim and reviewing its value/budget:

```json
[
  {
    "op": "apply_form_bindings",
    "plan": {
      "version": 1,
      "source_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "structural_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
      "bindings": [
        {
          "key": "course.name",
          "targets": [
            {
              "occurrence_id": "occ-2222222222222222222222222222222222222222222222222222222222222222",
              "copies": [
                0
              ],
              "max_chars": 100,
              "allow_reflow": true
            }
          ]
        }
      ],
      "repeats": [],
      "constraints": []
    },
    "values": {
      "course.name": "Reviewed course title"
    },
    "receipt": "./binding-receipt.json",
    "preview": "./binding-preview.json"
  }
]
```

```bash
ahwp edit source.hwpx -o bound.hwpx --ops OPS.json --verify
ahwp validate bound.hwpx
```

`apply_form_bindings` must be the only operation in its batch. The receipt is mandatory and
publishes atomically with the document and optional local preview. Do not combine this operation
with `--allow-partial` or `--report`; use its receipt instead. The preview contains exact values
and addresses. Default receipts/logs contain identities, hashes, counts and occurrence IDs,
not labels or values; read their outcome and retain the source/plan/output identities.
Native/preview warning counts are included in the receipt; original messages appear only in
the explicitly requested preview (`native_warnings`). Preview font substitution is not semantic
loss, and render/font acceptance remains separate.

As an alternative, add `--dry-run` to the edit invocation. It still writes and verifies a private
native candidate, then publishes a receipt with `published: false` and the requested preview;
it never publishes the native output. This is a review step, not a promise that apply is safe
for changed source bytes.

Both HWP and HWPX are supported only for same-format apply. To use another native format,
convert first and discover that converted source afresh. Revised values may replay against the
same original bytes and reviewed plan; do not feed earlier output to an old plan or repeat clones
on earlier output. Changed source requires fresh discovery and review.

Each target requires `max_chars` and an explicit `allow_reflow`; `copies` defaults to `[0]`
(the original), and additional copies must be selected explicitly. `max_chars` is a Unicode
scalar budget. Without reflow permission, replacement WCHAR length must not exceed the prior
claim, and LF/TAB are refused. Reviewed permission allows bounded text growth, not font shrinking,
grid/style changes or proof that the result fits a page. Render/font/Hancom evidence remains a
separate #419 acceptance step; a semantic receipt does not supply it.

For repetition, use a supported top-level table ID and an existing body paragraph ID whose text
is a unique insertion anchor. `copies` includes the original, so four means three clones. Supply
exactly that many unique caller-assigned `profile_ids`; select each target copy explicitly (0 is
the original). Read the returned source-to-copy maps. If the form was extracted, retain the real
#417 range report and its source-to-output map; do not infer shifted table indices or manually
slice IR. Existing #358 snapshot rules for ordinary batches remain unchanged.

Constraints are numeric `equal` or `sum` over explicit keys and caller-supplied totals. Numbers
are unsigned ASCII digits, optionally correctly grouped comma thousands, within `i64::MAX`.
`1,000` and `1000` compare equal; signs, units, decimals and whitespace refuse. Addition is
checked, mismatches refuse, and there is no expression evaluator or implicit total calculation.

Over MCP, `hwp_slots` with `path`, `forms: true`, `occurrences: true` returns the same catalogue.
Call `hwp_edit` with `input`, `output` and one `apply_form_bindings` **object**, not an array.
That object contains the same `plan`, `values`, `receipt` and optional `preview` shown above,
without `op`. It uses schema validation, checked write paths and the same ops normalization and
engine. There is no MCP `verify` flag; native verification is part of the operation.

See [the binding design](../design/25-form-binding-plan.md) and the versioned `schemas/form-discovery-v1.schema.json` and
`schemas/form-binding-plan-v1.schema.json` contracts for fields, bounds and evidence limits.

### Start a paragraph on a new page, and replace typed indentation

Three paragraph-level ops handle form pagination. Take the paragraph address from `ahwp cat
doc.hwpx --format json --with-segments --segments v2`; each op is a flat `--ops` entry, or an
array of `hwp_edit` of the same name.

```json
[
  {"op":"set_break_before","address":{"at":{"section":0,"paragraph":5}},"value":"on"},
  {"op":"set_indent_level","address":{"at":{"section":0,"paragraph":7}},
   "level":2,"step":"5mm","hanging":"3mm"}
]
```

- `set_break_before` sets (`on`) or clears (`off`) the page-break-before bit of one body
  paragraph: the paragraph-header break type in HWP and `pageBreak` in HWPX. That header bit is
  the only placement that genuine Hangul-saved files show, so a paragraph inside a table cell
  and the first paragraph of a section are refused with the cause and the remedy. Column breaks
  are not offered. Line caches are page-relative, so a change also drops the stored line layout of
  that paragraph and of the paragraphs after it up to the next page break, and Hangul or the
  writer re-measures them from the new page top (an unchanged repeat touches nothing).
  Repeating a request that is already in effect succeeds and changes nothing.
- `set_indent_level` removes only the blank characters at the start of one paragraph (ASCII,
  ideographic and no-break spaces) and puts the indentation on the paragraph shape instead:
  `margin_left` is `level` times `step` and the first line hangs by `hanging` (default `0mm`).
  `level` is 0 to 7. Words, inner spacing and labels stay as typed. Invisible controls at the
  start of the paragraph (the section and column definitions of a section's first paragraph, a
  field start, a bookmark, a page control) stay in place and the blanks after them are removed;
  the first remaining character keeps the character shape it had, so a plain blank before a bold
  label does not unbold the label. Blanks after a visible leading item (an auto number, a tab,
  an object) are a separator and stay, and a paragraph that holds only blanks keeps them. A
  paragraph that carries HWP highlight or proofreading marks (range tags) is refused when blanks
  would be removed, because the marks hold text offsets that are not moved (#470): remove the
  marks first, or use `set_para` with `left_mm` and `indent_mm`, which strips nothing. Running
  it a second time strips nothing, adds no paragraph shape, keeps the paragraph's native line
  layout and reports `pieces_touched` 0.
- `keep_together` sets or clears only supplied `keep_with_next`, `keep_lines`, `break_before_shape`
  fields (attr1 bits 17-19, Hangul 12.30.0 G4). At least one field is required. Like `set_align`,
  it accepts one body or nested paragraph address, including a cell paragraph. It preserves
  unspecified bits and header `break_type`; repeating it is a no-op. Renderer pagination is outside this contract.

These ops address one paragraph at a time, so a stale `id` is refused as with any addressed op.
A `set_format` run address resolves against the document before the batch, so a batch that
strips blanks (`set_indent_level`) or replaces text in the same paragraph before it is refused
with nothing written, naming the cause: put `set_format` first, or run the edits separately.
Results are structural; this manual makes no claim about the page count Hangul will show.
Evidence and limits are recorded in `docs/design/07-hangul-compat-rules.md` (J1 to J3).

### Classify a refused edit

A refused `ahwp edit` still writes the `edit-report-v1` file that `--report` names, and
`--dry-run` still prints it. The report (schema version 1.4) carries a top-level `refusal`
object, and when one op refused, that op's entry carries the same object with `status:
"failed"`. MCP `hwp_edit` returns the same object as `structuredContent.refusal` of its error
result, and writes the same report to its `report` argument for any refusal raised after the
arguments are read. Stderr and the MCP error text keep the full Korean sentence for people;
`refusal.message` is a fixed, content-free sentence per code that never quotes request content
(#348). Argument errors raised before an edit plan exists (such as a missing `keep_together` field
or an out-of-root path) may return without `structuredContent.refusal` or a report file. An `apply_form_bindings` batch publishes
only its receipt, refused or not.

```json
{"code":"insert_blocks.unknown_style","values":{"style":"Heading 9","missing_style":"refuse"},"message":"..."}
```

Branch on `code`, never on `message`. Values the user typed (anchors, style names) appear in
`values` as given; an issue path is a list of segments, so a style named
`a; layout_overflow at $.z: b` stays one segment and cannot pose as a second issue.

| `code` | `values` |
| --- | --- |
| `range_tag_shift` | none |
| `section_owner_insert`, `section_owner_move` | none |
| `add_table.anchor_not_found` | `anchor` |
| `insert_blocks.unknown_style` | `style`, `missing_style` |
| `insert_blocks.invalid_definition` | `style` and `detail`, or `issues` and `container_width_mm` |
| `insert_blocks.style_not_paragraph`, `insert_blocks.duplicate_style`, `insert_blocks.style_cycle` | `style` |
| `insert_blocks.style_id_overflow` | `style`, `style_id`, `max_style_id` |
| `layout_overflow`, `insert_blocks.invalid_spec` | `issues` (`code`, `path` as a list, `message`, and for an overflow `measured_width_mm`), `container_width_mm` |
| `preservation_loss` | `events` (`code`, `resource`, `disposition`, `count`) |
| `unapplied_requests`, `no_applicable_edits` | none; failed `ops` entries name the misses |
| `document_protected` | `protection`: `password`, `certificate_encryption`, `certificate_drm`, `drm` or `signature` |
| `verify_mismatch` | `counts`: the differing count invariants, each `{name, expected, actual}`; an empty list means only the canonical digest differs |
| `unclassified` | none; any refusal not listed above |

`document_protected` (1.2) is an open-time refusal: the input is protected, so the run has no `ops`. `password` covers an absent and a wrong password alike, for HWP5 and HWPX; the engine does not tell them apart. A distribution document is readable, so it is not refused. Consumers that accept only 1.1 must also accept 1.2; every 1.1 field and code is unchanged.

`verify_mismatch` (1.3) is what `--verify` reports when the re-read output differs from the edited document. Nothing is published. `counts` names only count invariants (`sections`, `paragraphs`, `tables`, `pictures`, `generic_controls`, `fields`, `bookmarks`, `hyperlinks`, `bin_streams`, `char_shapes`, `para_shapes`, `styles`, `text_chars`) and never document content. Before 1.3 this refusal was `unclassified`.

### Losses of a cross-format edit

An `ahwp edit` from an HWP5 input to a `.hwpx` output runs the same loss accounting as `ahwp
convert` for that pair, with or without `--verify`. The events (`{code, resource, disposition,
count}`, the `hwp-preservation-report-v1` shape) go to stderr, to the MCP `preservation` field,
and, since schema 1.4, to the `preservation` list of the `--report` file. The list is absent when
nothing was lost. Like `convert` without `--strict`, such an edit publishes with its events
reported: `metadata_value_removed` alone fires on every HWP5 to HWPX save, because HWPX stores
timestamps to the second. Two events still refuse it, `paragraph_range_tag_unrepresentable` and
`font_type_info_unrepresentable`. An edit to a `.hwp` output keeps its refuse-on-any-event policy.

`--verify` in this direction compares the HWPX re-read with the edited document after projecting
the fields the two formats encode differently (trailing bytes of header records, the numeric note
and page-border records, table and cell attribute words, raw drawing-object trees and similar), each
named and commented in `project_hwp5_to_hwpx_representation`. Text, structure, geometry and
every field not on that list are still compared, the output must still carry the section's
`hp:footNotePr`, `hp:endNotePr` and `hp:pageBorderFill` elements when the source had those records,
and re-typed drawing objects are accepted only when `control_removed` was reported. Any other
difference refuses as `verify_mismatch`. The cost: for the byte-encoded fields on that list the
digest cannot see a real loss inside those bytes.

### The preservation ledger

`convert`, `merge` and `split` record every item they could not preserve in the typed
`hwp-preservation-report-v1` ledger. On the CLI the ledger is written only when you pass
`--loss-report {file.json}` (the command then prints a one-line summary on stderr when something
was lost); over MCP it comes back in the `preservation` field of every `hwp_merge` and `hwp_split`
response. `--strict`
refuses to publish when the ledger is non-empty, but it is opt-in on both commands rather than the
default: a merge always drops the package passthrough of every input after the first, so a
fail-closed default would refuse even the most ordinary merge. Read the ledger instead of assuming
a clean run.

### Linting official documents

`ahwp lint {file}` / `hwp_lint` runs ten Korean official-document notation and structure rules over
a `.md`, `.hwp` or `.hwpx` source and emits the `hwp-lint-report-v1` contract
(`rule_id`/`severity`/`line`/`col`/`message`) under `--json`. Use it as the gate before an
official document leaves the agent, and `--strict` when a finding should stop the pipeline.

### Exit codes

The conventions differ on purpose, so "non-zero means failure" is the wrong reading:

| Command | Convention |
|---|---|
| `compare` | diff(1): 0 identical, 1 differences found, 2 the run itself failed |
| `lint` | always 0; `--strict` exits 1 only on an error-severity finding |
| `grep` | 1 when nothing matched — a normal result, not an error |
| `validate`, `new --strict`, `convert --strict`, `merge --strict`, `split --strict` | 0 on success, non-zero on failure |

MCP has no exit codes. `hwp_compare` returns `identical` and `hwp_grep` returns `count`, both with
`isError` false, so an agent must read the field rather than treat the call as failed.

### Passwords

Six MCP tools accept a per-call `password` — `hwp_read`, `hwp_convert`, `hwp_render`, `hwp_merge`,
`hwp_split` and `hwp_compare`. It is never cached across calls, and it is scrubbed from
notifications. On the CLI prefer `--password-stdin` over `--password`, which is visible in the
process arguments. Never let a credential reach a report, a receipt, a generated file, a command
transcript or a persistent environment variable.

### Environment variables

- `HWP_FONT_DIR` — default font directory when `--font-dir` is not given. Rendering and PDF output
  need CJK fonts, and this is often easier than threading `--font-dir` through a client's argument
  array.
- `HWP_LANG` — help and message language (`en` / `ko`); `--lang` overrides it.
- `HWP_BIN_DIR` — install location for the claude.ai bundle's `bootstrap.sh` (default
  `~/.local/bin`).
