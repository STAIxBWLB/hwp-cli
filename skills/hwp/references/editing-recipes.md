[한국어](editing-recipes.ko.md) · [English](editing-recipes.md)

# Native editing recipes and old `hwpx` migration crosswalk

This is the canonical editing guide in the one bundled `hwp` skill. It replaces old `hwpx`
guidance without adding a wrapper, helper script, executable recipe, or binary template. Run the
commands with a binary whose release and capability record cover the selected recipe. The
original-form reference run below was made on a pre-release source build, not on an installed
release; the workflow first ships in v1.5.0, and older releases do not support its commands.

## Scope and release boundary

Minimum supported `hwp` version for the legacy migration crosswalk: **v0.12.1**. This release
carries the corrected Phase 2.5
label-form and skill-export contracts. A local build, an unversioned `PATH` binary, or an unmerged
branch is not retirement or replacement evidence.

The eight Markdown files under `templates/` remain the bundled template SSOT. The six old binary
templates are a read-only retirement compatibility corpus; do not copy, archive, or edit them.

## Original-form workflow

### Select the native source and evidence scope

When completing a supplied HWP/HWPX form, start from that native file. Use Markdown only when
reauthoring is intended. Do not extract a form by slicing JSON paragraph arrays or unpacking and
patching package XML. Use typed `split`, source-bound bindings and native edits.

The complete workflow below needs v1.5.0 or later, including `document.original_form`. The
[versioned capability record](original-form-capabilities-v1.json) records its reference run on a
**pre-release source build** at its recorded commit (which reported package version 1.4.0) and
checked that released v1.4.0 lacks the workflow. Check `hwp --version` and the record exported
by your binary before using it.

```sh
hwp --version
hwp skill export -o ./hwp-guide
```

Use an isolated working directory. Keep discovery text, reviewed plans, exact previews and local
address maps private. Receipts do not replace those reviewed inputs. Preserve every stage's
source/output identity; an ID or hash from an earlier output is not a selector for a later one.

### Prepare the bounded P0 example

For a real supplied form, skip fixture authoring and keep its requested fonts. For the public
reference run, use the owner-authored #416 generator from the repository:

```sh
python3 tools/generate_original_form_fixture.py ./target/original-form-workflow
```

The reference variant changes only `document.styles.body.font_family` in the generated authoring
spec to `NanumGothic`, before composing a native source. Save that authoring input as
`./target/original-form-workflow/p0-nanum.spec.json`; keep `expected.json`, `inside.png` and `outside.png`. Supply licensed
`NanumGothic.ttf` and `NanumGothicBold.ttf` under `./target/original-form-workflow/fonts/`. Their exact hashes are in the
capability record's certification policy. Fonts are not bundled. This fixture setup is not an
instruction to change the fonts of someone else's supplied native form.

```sh
cd ./target/original-form-workflow
hwp compose "$PWD/p0-nanum.spec.json" -o ./source.hwpx
```

The capability asset contains complete, reviewed JSON values under `reference_inputs`. Copy each
value into the file named by its key, using a JSON editor or your client's file-writing facility.
These are data inputs, not an extra editing script. They are bound to the recorded P0 source and
implementation: compare fresh hashes before using them. For another source, author and review a
new plan, not a modified native IR dump. No shipped command automatically derives a complete
binding or certification plan.

### Discover and extract the native range

```sh
hwp info ./source.hwpx --json
hwp slots ./source.hwpx --forms --occurrences --json > ./source-catalog.json
hwp cat ./source.hwpx --format json --with-segments --segments v2 > ./source-map.json
```

Review labels, old values, `bindable`, paragraph/table IDs and body coordinates. Repeated labels
are not unique addresses. The P0 source already has **four** profile tables. To demonstrate
repetition to four, select application + development + only profile 1. In this generator the
application heading is body paragraph 4 and profile 1's table is paragraph 11. Confirm those
coordinates in the actual map and generator manifest; they are not general document constants.

Set `SOURCE_SHA256` to the actual `source_sha256` in `source-catalog.json`, then run:

```sh
hwp split ./source.hwpx --out-dir ./extracted \
  --from heading:FORM_START_APPLICATION --to at:0.11 \
  --source-sha256 "$SOURCE_SHA256" --strict \
  --range-report ./extraction.json --loss-report ./extraction-preservation.json
hwp validate ./extracted/source-001.hwpx
```

Keep the real range report and source-to-output map. The recorded run kept `BinData/image2.png`
(the inside image), deliberately excluded `BinData/image1.png` (the earlier outside image), and
had no dropped-by-format, kept-unverified or unreadable reference site. An excluded source asset
is different from a lost selected asset. A same-format HWP input follows the same native selection
contract; select fresh addresses and retain its own report.

### Review a source-bound four-profile plan

Prepare a unique insertion heading after the extracted profile's table, then apply the existing
`heading` named style in a separate edit. This common CLI/MCP sequence avoids relying on the CLI's
inline `insert_para.style`/`char` fields, which MCP does not expose.

```sh
hwp cat ./extracted/source-001.hwpx --format json --with-segments --segments v2 > ./extracted-map.json
hwp edit ./extracted/source-001.hwpx -o ./inserted.hwpx --ops "$PWD/prepare-headings.json" --verify
hwp styles ./inserted.hwpx --format json
hwp edit ./inserted.hwpx -o ./prepared.hwpx --ops "$PWD/prepare-style.json" --verify
hwp slots ./prepared.hwpx --forms --occurrences --json > ./prepared-catalog.json
hwp cat ./prepared.hwpx --format json --with-segments --segments v2 > ./prepared-map.json
```

For the P0 map the selected table moves from source paragraph 11 to extracted paragraph 8.
`prepare-headings.json` inserts `FORM_START_PROFILE_2` after paragraph 8;
`prepare-style.json` applies `heading` to the new paragraph 9 with `clear_direct: true`.
Refresh the new address between requests. These changes target a new heading, not the form's
existing labels or cell geometry.

Review every occurrence against `prepared-catalog.json`. The worked plan has 20 binding keys,
41 selected copy-targets and 6 numeric constraints. Each target has explicit `copies`, `max_chars`
and `allow_reflow`. Its `source_sha256` and `structural_sha256` come from this **prepared** source,
not the original whole document or the first extraction. The complete array is
`reference_inputs["binding.ops.json"]` in the capability asset.

| Keys | Reviewed target and value |
|---|---|
| `course.name`, `course.year` | Application, development and all four profiles: `Reviewed Methods`, `2031` |
| `total.hours` | Application hours, development grand total and every profile's hours: `90` |
| `profile.name.1` through `.4` | Copy indices 0 through 3: `Profile Alpha`, `Profile Beta`, `Profile Gamma`, `Profile Delta` |
| `profile.objective` | First objective paragraph in all four copies: `Reviewed objective.` |
| `profile.continuation` | Second objective paragraph in all four copies: three leading spaces followed by the unchanged `Nested item: original wording remains unchanged.`; indentation removes only those spaces later |
| `module.a.lecture`, `.practice`, `.subtotal` | Development row 3: `20`, `10`, `30` |
| `module.b.lecture`, `.practice`, `.subtotal` | Development row 4: `22`, `8`, `30` |
| `module.c.lecture`, `.practice`, `.subtotal` | Development row 5: `18`, `12`, `30` |
| `lecture.total`, `practice.total` | Development row 6: `60`, `30` |

The 6 `sum` constraints check each module, the lecture total, the practice total and the grand
total. Totals are supplied and reviewed values, not implicit calculations or an expression
language. A mismatch refuses before publication.

The repeat entry identifies the selected profile table and the unique new heading paragraph,
sets `copies: 4` and supplies four distinct `profile_ids`. Four includes the original, so it adds
three clones. The original table remains first and the clones follow that insertion heading,
keeping table indices 2 through 5 contiguous for the certification repeat allowance. Binding a
copy requires its explicit copy index; no label-based guess chooses a person/profile for you.

### Dry-run, inspect and apply

Keep text values such as `- draft => reviewed: course` as literal JSON strings in `values`, not
interpolated delimiter-based flags. Binding keys are exact strings, not property paths. Numeric
constraint values still follow the unsigned-integer contract.

Copy `binding.ops.json` to `dry.ops.json` and change only `receipt` to `dry-receipt.json` and
`preview` to `dry-preview.json`. The plan and values remain identical. Use absolute or `./` paths
with existing parent directories for asset-bearing inputs and atomic report destinations.

```sh
hwp edit ./prepared.hwpx -o ./dry.hwpx --ops "$PWD/dry.ops.json" --dry-run
```

Review the exact local preview, selected claims, numeric values and source-to-copy maps. A dry-run
still writes/verifies a private candidate, but its receipt has `published: false` and it publishes
no native output. Apply against the same reviewed `prepared.hwpx` bytes:

```sh
hwp edit ./prepared.hwpx -o ./bound.hwpx --ops "$PWD/binding.ops.json" --verify
hwp validate ./bound.hwpx
```

`apply_form_bindings` is the only operation in its batch. Do not combine it with `--allow-partial`,
style operations or `--report`; retain its own receipt. `preview` is explicit and contains exact
content. The default receipt carries hashes, counts and occurrence IDs, not labels or values.
The reference apply reports 3 cloned profiles and the semantic budget
`selected_text_and_declared_clones_only`. It does not assert page fit or native-open success.

To revise allocations again, replay reviewed values against the same prepared source and update
all relevant numeric constraints/totals. Do not feed a prior bound output to the old source-bound
plan or clone the previous clones again. Any changed source requires fresh discovery and review.

### Add headings and apply pagination and indentation

Use fresh output addresses to add `FORM_START_PROFILE_3` and `_4` before the second and third
clone-host paragraphs. In the recorded `bound.hwpx`, these are body paragraphs 11 and 12 in the
request snapshot; after both insertions, the new headings are 11 and 13. Apply `heading` to those
new paragraphs in a second request. Keep the original form labels and signature areas unchanged.

```sh
hwp cat ./bound.hwpx --format json --with-segments --segments v2 > ./bound-map.json
hwp edit ./bound.hwpx -o ./headings-inserted.hwpx --ops "$PWD/remaining-headings.json" --verify
hwp edit ./headings-inserted.hwpx -o ./headed.hwpx --ops "$PWD/remaining-style.json" --verify
hwp slots ./headed.hwpx --forms --occurrences --json > ./headed-catalog.json
hwp edit ./headed.hwpx -o ./paged.hwpx --ops "$PWD/pagination.ops.json" --verify --report ./pagination-report.json
hwp slots ./paged.hwpx --forms --occurrences --json > ./paged-catalog.json
hwp edit ./paged.hwpx -o ./final.hwpx --ops "$PWD/indent-repeat.ops.json" --verify --report ./indent-repeat-report.json
hwp compare ./paged.hwpx ./final.hwpx --format json > ./indent-compare.json
```

`pagination.ops.json` sets break-before on the new profile headings (paragraphs 9, 11 and 13 in
`headed.hwpx`) and applies the following shape to each fresh occurrence paragraph ID for the
four continuation paragraphs:

```json
{"op":"set_indent_level","address":{"id":"CURRENT_PARAGRAPH_ID"},"level":2,"step":"5mm","hanging":"2mm"}
```

Replace the selector with the current catalogue's `paragraph_id`; complete reference IDs are in
the asset. `level` is 0 through 7, `step`/`hanging` are unit strings. It removes only leading blank
characters and uses paragraph margins/indent, without changing inner words or the table grid.
`set_break_before` is body-only and refuses cell paragraphs and a section's first paragraph.
`keep_together` currently refuses because its HWP5 flag encoding lacks genuine saved-file proof.
Do not silently shrink fonts or claim that a profile is kept together when it is not supported.

The repeat indentation array uses **refreshed** paragraph IDs from `paged-catalog.json`. The
recorded second pass touches 0 pieces for each of the four operations; comparison is identical,
and native bytes also match. Do not reuse a stale run address after text length has changed.

### Render and certify the exact delivered artifacts

```sh
hwp validate ./final.hwpx
hwp convert ./final.hwpx -o ./final.hwp --strict --loss-report ./conversion-preservation.json
hwp validate ./final.hwp
hwp slots ./final.hwpx --forms --occurrences --json > ./final-hwpx-catalog.json
hwp slots ./final.hwp --forms --occurrences --json > ./final-hwp-catalog.json
hwp render ./final.hwpx -o ./hwpx.pdf --font-dir ./fonts --report ./hwpx-render.json --layout-json ./hwpx-layout.json
hwp render ./final.hwp -o ./hwp.pdf --font-dir ./fonts --report ./hwp-render.json --layout-json ./hwp-layout.json
```

This reference run binds/edits HWPX, then converts the delivered artifact to HWP. Same-format HWP
binding is supported separately; a format change needs fresh discovery and its own identities,
not reuse of a binding plan from the other format. Conversion validity and an empty preservation
ledger do not establish Hancom rendering fidelity.

Copy `expect-hwpx.json` and `certify-hwpx.json` from `reference_inputs`. The reviewed expectation
plan contains the prepared source hash, exact output hash, 134 expected cells, 6 unique form-start
anchors with pages 1 through 6, dimensions for all six tables, and the allowance
`repeated_tables: [{"id":"profiles.repeat","table":2,"extra_copies":3}]` over the selected
three-table source. All those values are specific to this fixture, not portable page goldens.

For HWP, copy that expectation to `expect-hwp.json` and set `output_sha256` to the actual HWP
hash from fresh discovery (the reference value is in `reference_input_variants.hwp_certificate`).
Copy the policy to `certify-hwp.json` and change its `document.original_form.plan` accordingly.
Both policies pin the exact regular and bold font files, forbid substitution, check every page,
and keep the outside-page-bounds and potential-collision failure gates enabled.

```sh
hwp certify ./final.hwpx --policy "$PWD/certify-hwpx.json" --report "$PWD/certificate-hwpx"
hwp certify ./final.hwp --policy "$PWD/certify-hwp.json" --report "$PWD/certificate-hwp"
```

Use a new report directory for each run: publication is atomic and existing report directories
are refused. Read `overall`, `checks.original_form`, the policy/plan/input hashes, font identities,
render results and the manifest. The source hash inside the expectation plan is declared/echoed;
it is not an independent load-and-compare check of the supplied source. Retain the extraction,
heading-edit, binding and final-discovery records to establish the actual chain.

Certification checks expected cell values and table row/column dimensions, not a complete
source-to-output width/border/style/merge projection. The binding semantic budget and native
re-read verification are separate evidence. `render-report.complete` means diagnostic collection
was complete, not that a page is visually correct. Without pinned fonts, the original-form result
is explicitly limited and cannot certify successfully.

The recorded CLI run passes both selected policies with 134 cells, 6 form starts and 6 pages per
format; no outside-page bounds or potential collisions were detected under that policy. HWP
retains one `table_cell_content_overflow` warning while expected-text coverage passes. Preserve
that warning for review. This is bounded native-backend PDF generation, not universal native-table
PDF coverage, portable pixel equality or Hancom parity.

### Use the corresponding MCP calls

Start the stdio server in an isolated workspace, or let a configured MCP host start it:

```sh
hwp mcp --root "$PWD" --font-dir "$PWD/fonts"
```

Use the same reviewed data and stage boundaries:

| CLI stage | MCP tool and arguments |
|---|---|
| Discovery | `hwp_slots`: `path`, `forms: true`, `occurrences: true` |
| Inspect body coordinates | `hwp_read`: `path`, `format: "markdown"`, `with_segments: true` |
| Native range | `hwp_split`: `input`, `out_dir`, `from`, `to`, `source_sha256`, `range_report`, `loss_report`, `strict` |
| Insert new headings | `hwp_edit`: `input`, `output`, `insert_para` array with `address`, `text`, `before` |
| Style inserted headings | Separate `hwp_edit` with `apply_style` array, the fresh address, `style: "heading"`, `clear_direct: true` |
| Dry-run / bind and repeat | Exclusive `hwp_edit.apply_form_bindings` **object** with `plan`, `values`, `receipt`, optional `preview`; top-level `dry_run: true` for the review call |
| Pagination / indent | `hwp_edit` with `set_break_before` and `set_indent_level` arrays; refresh occurrence paragraph IDs first |
| Compare / validate | `hwp_compare` with `a`/`b`; `hwp_validate` with `path` |
| Convert output | `hwp_convert` with `input`, `output`, `strict: true` |
| PDF generation | `hwp_render` with `path`, `format: "pdf"`, `output_path`, `pages: "all"`, `font_dir` |
| Certification | `hwp_certify` with `input`, `policy`, `report` |

For binding, take the one object in `reference_inputs["binding.ops.json"]`, remove only `op`,
choose fresh local receipt/preview paths, and place it under `apply_form_bindings`. Do not send
an array, `verify`, `base_dir`, `report`, `allow_partial` or a style operation with it. Native
verification is intrinsic to this operation. A dry-run does not publish the native output.

MCP `insert_para` does not expose CLI inline `style`/`char`; use the separate style request above.
MCP `hwp_read` has legacy markdown coordinates only, not `segments: "v2"` or JSON with segments.
For nested edits, use `paragraph_id` from fresh occurrences or styles' `used_by`; for new body
headings, inspect fresh body coordinates. MCP `hwp_render` exposes no `report` or `layout_json`
argument: export those two artifacts through CLI when needed. MCP certification still publishes
its own policy/render evidence. Do not invent tool arguments to imitate a CLI flag.

The reference MCP run used 22 actual stdio tool calls through these interfaces. All passed; the
final HWPX and HWP bytes matched the CLI artifacts, and both MCP certification calls passed.
This is a fixture-specific observation, not a universal byte-parity guarantee.

### Keep JSON IR and embedded resources separate from the workflow

Inspection JSON is not a plain-text authoring format. `Paragraph.chars` is a typed `HwpChar`
array, for example `{"chars":[{"Text":"가"},{"Text":"나"},{"CharCtrl":13}]}`, not the string
`"가나"`. Native controls have their own widths and payloads; do not use string indexing or raw
array slicing as an alternative editing engine.

`hwp cat --format json` (including the document beside CLI v2 segments) is an inspection export,
not a self-contained binary-resource package. If JSON must make a complete round trip, use:

```sh
hwp convert ./final.hwpx --to json --embed-bin -o ./final.self-contained.json
hwp new --from ./final.self-contained.json -o ./json-roundtrip.hwpx
hwp validate ./json-roundtrip.hwpx
hwp compare ./final.hwpx ./json-roundtrip.hwpx --format json
```

For export, MCP uses `hwp_convert` with `input`, `output`, `to: "json"`, `embed_bin: true`.
For re-entry, use `hwp_new` with `output` and `json` containing the complete IR JSON **string**,
not its filename. `hwp convert` does not accept that JSON as a native input. The recorded JSON
re-entry retains six tables and the image control and compares semantically identical.
This is the recorded semantic/resource check, not byte identity of every native format or
opaque record.
An export without payload bytes is incomplete for image round-tripping; a missing-asset refusal
is a guard, not evidence that arbitrary native images were lost. Native editing remains the
preferred form-completion path.

### Obtain separate person-run Hancom evidence

Before claiming native compatibility, a person opens the exact delivered HWP/HWPX file in
Hancom Office, checks fonts, all populated/continued cells, signatures, page starts and retained
images, and exports the PDF. Record the product/build and artifact/export hashes with the
applicable verification receipt. A CLI-rendered PDF or a passing native-only certificate is not
that receipt. Follow the published Hancom receipt and PDF-parity contracts for the evidence type
being claimed; do not fill in a person-run `pass` from automated rendering.

When a matching `hancom-verification-receipt-v1` is available, the certification policy can add
`document.hancom_open` with `receipt`, `require_pass: true`, and
`require_artifact_sha256: true`, then certify the exact artifact into a fresh report directory.
The reference run has **no** Hancom open/export evidence; that sign-off remains unverified.

## Native migration crosswalk

### Status words

- **exact**: the native command is the old operation itself.
- **composed**: more than one native command provides the useful workflow.
- **limited**: native commands cover the stated safe path but do not reproduce the old semantic.
- **intentionally retired**: raw package surgery or a non-native fallback has no replacement.

| Old inferred operation | Native command sequence | Status | Known limitation | Runnable verification |
|---|---|---|---|---|
| `summary` | `hwp info form.hwpx --json` then `hwp cat form.hwpx --format json > form.json` | composed | No legacy pretty summary or XML section-index map. | `test -s form.json && hwp validate form.hwpx` |
| `unpack` | Do not unpack; use `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify`. | intentionally retired | No supported raw-ZIP extraction workflow. | `hwp validate edited.hwpx` |
| `repack` | Do not repack; use `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify`. | intentionally retired | Native writers own package layout; manual manifest changes are unsupported. | `hwp validate edited.hwpx` |
| `edit` run-spanning find/replace | `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | limited | Does not promise matching a find string split across arbitrary XML text runs. | `hwp validate edited.hwpx && hwp cat edited.hwpx --format plain` |
| `fill` run-spanning `{{slot}}` | `hwp slots template.hwpx --json` then `hwp fill template.hwpx -o filled.hwpx --set "name=value" --json` | limited | A slot may span formatting runs, but never a line break or paragraph boundary. | `hwp slots template.hwpx --json && hwp validate filled.hwpx` |
| `fill-table` | `hwp fill template.hwpx -o filled.hwpx --data tables.json --json` | exact | `tables.json` must follow the native `tables` data contract and target an existing table. | `hwp validate filled.hwpx` |
| `fill-form` | `hwp edit form.hwpx -o filled.hwpx --set-cell-by-label "성명=홍길동" --set-cell-by-label "소속=AI센터" --verify` | limited | Each normalized label must resolve to exactly one adjacent or header-row value cell; ambiguity and duplicate targets refuse publication. Use `--label-table N` to scope recursive tables. | `hwp validate filled.hwpx && hwp cat filled.hwpx --format plain` |
| `analyze` | `hwp info form.hwpx --json` then `hwp cat form.hwpx --format json > form.json` | composed | JSON IR is useful for text, tables, and anchors, but exposes no old raw `sec` child indices or XML style IDs. | `test -s form.json && hwp validate form.hwpx` |
| `edit-section` | `hwp cat form.hwpx --format json > form.json`, then `hwp edit form.hwpx -o edited.hwpx --delete-para "old paragraph" --insert-para "anchor=>replacement paragraph" --verify` | limited | Anchor and paragraph operations are not raw `[start:end)` section-index surgery and do not clone an XML paragraph style by index. | `hwp validate edited.hwpx && hwp cat edited.hwpx --format json > edited.json` |
| `linesegarray` clearing on edit | `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | composed | The writer manages layout records internally; there is no user-visible `linesegarray` switch or XML-level report. | `hwp validate edited.hwpx` |
| sec-index section edits | `hwp cat form.hwpx --format json > form.json`, then `hwp edit ... --delete-para ... --insert-para ... --verify` | limited | The native surface deliberately has no stable `sec` child index contract. | `hwp validate edited.hwpx` |
| mimetype-first STORED repack | `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | intentionally retired | The writer preserves valid package construction but exposes no general raw-ZIP repack API. | `hwp validate edited.hwpx` |
| `guard` / `page_guard` structural drift metric | `hwp validate edited.hwpx`, then `hwp render reference.hwpx -o reference.png --report reference.render.json` and `hwp render edited.hwpx -o edited.png --report edited.render.json` | composed | Validation and render reports do not reproduce XML paragraph/table/text-delta thresholds. Compare report page counts and review rendered output when layout matters. | `hwp validate edited.hwpx && test -s reference.render.json && test -s edited.render.json` |

## Inspect before editing

### Build an editable map

Start with native metadata and IR output. Search the JSON for stable visible text, table content,
or a field/bookmark name that can serve as an anchor; do not infer an old XML index from it.

```bash
hwp info form.hwpx --json
hwp cat form.hwpx --format json > form.json
hwp fields form.hwpx --json
hwp bookmarks form.hwpx --json
```

### Verify the inspection

```bash
test -s form.json
hwp validate form.hwpx
```

## Fill native placeholders and forms

### Fill a template or table

Use `hwp slots` before `hwp fill` for placeholders. Use `--data` for data-driven table rows; it
is the native `fill-table` replacement.

```bash
hwp slots template.hwpx --json
hwp fill template.hwpx -o filled.hwpx --set "title=2026 Plan" --json
hwp fill table-template.hwpx -o table-filled.hwpx --data tables.json --json
hwp validate filled.hwpx
hwp validate table-filled.hwpx
```

### Fill a label-value form

`--set-cell-by-label` accepts `label=value`, finds adjacent `label | value` and header/data-row
layouts, and is atomic by default. Do not use `--allow-partial` to bypass an ambiguous or duplicate
label.

```bash
hwp edit form.hwpx -o form-filled.hwpx \
  --set-cell-by-label "성명=홍길동" \
  --set-cell-by-label "소속=AI센터" \
  --verify
hwp validate form-filled.hwpx
```

## Edit content through anchors

### Replace or reshape a paragraph

Use `--replace` for a supported in-place match. For a section-shaped change, use an existing text
anchor with paragraph operations. Keep the source unchanged and write to a new output until the
result has passed validation and review.

```bash
hwp edit form.hwpx -o revised.hwpx \
  --replace "2025 Plan=>2026 Plan" \
  --delete-para "old body paragraph" \
  --insert-para "body heading=>replacement paragraph" \
  --verify
hwp validate revised.hwpx
```

### Understand the boundary

There is no native raw `sec` index editor, XML paragraph-style cloning operation, unpack/repack
route, or Python helper. If the intended edit depends on those semantics, stop and keep the source
document intact rather than treating this recipe as an exact migration.

## Guard a completed output

### Validate and render

Validation confirms package structure. Render reports give page counts and renderer diagnostics;
they are not a replacement for the old structural XML-drift thresholds.

```bash
hwp validate revised.hwpx
hwp render form.hwpx -o reference.png --report reference.render.json
hwp render revised.hwpx -o revised.png --report revised.render.json
```

### Review the evidence

Check both render report files and inspect the page images when layout is material. A changed page
count or visible overflow is a reason to revise the content, not to weaken validation.

## Safety and non-goals

Every writing command above ends with `hwp validate`. Native edits and fills publish atomically;
failed required changes leave the existing output untouched. These recipes intentionally do not
restore the retired wrapper, a Python XML helper, executable recipe files, or old binary templates.
