[한국어](editing-recipes.ko.md) · [English](editing-recipes.md)

# Native editing recipes and old `hwpx` migration crosswalk

This is the canonical editing guide in the one bundled `hwp` skill. It replaces old `hwpx`
guidance without adding a wrapper, helper script, executable recipe, or binary template. Run the
commands with a released `hwp` binary, not a local checkout.

## Scope and release boundary

Minimum supported `hwp` version: **v0.12.1**. This release carries the corrected Phase 2.5
label-form and skill-export contracts. A local build, an unversioned `PATH` binary, or an unmerged
branch is not retirement or replacement evidence.

The eight Markdown files under `templates/` remain the bundled template SSOT. The six old binary
templates are a read-only retirement compatibility corpus; do not copy, archive, or edit them.

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
