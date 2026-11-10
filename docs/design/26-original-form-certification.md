[한국어](26-original-form-certification.ko.md) · [English](26-original-form-certification.md)

# Original-form certification

`ahwp certify` can check a delivered HWP/HWPX artifact against a reviewed expectation plan for a
supplied native form (#419, part of #411). It binds the source, plan, output, font and render
identities in one report, and every refusal names a cause and a next step without carrying
document text.

## Inputs

The certification policy gains one optional section:

```json
"document": {
  "fonts": {"manifest": [{"path": "fonts/NanumGothic.ttf", "sha256": "..."}]},
  "original_form": {"plan": "plan.json", "address_map": false}
}
```

`plan` is an `original-form-certification-plan-v1` file next to the policy. It holds the expected
text, so it stays local: certify reads it once, hashes it, and never copies it into the report
directory, a refusal or a log. The plan can declare:

| Field | Meaning |
|---|---|
| `source_sha256` | Identity of the supplied source form, echoed into the report |
| `output_sha256` | When present, the certified artifact must have exactly this hash |
| `table_count` | Tables in the source form, in the recursive order `--set-cell` indexes |
| `form_starts` | Required form-start anchors: the exact text of the top-level body paragraph that opens each form, with `page_start` (default true) and an optional `page` |
| `tables` | The stable grid projection: rows and columns of a table by index |
| `expected_cells` | Expected values by `table`/`row`/`col`; whitespace runs are normalized, every visible character must match |
| `allowances.repeated_tables` | Profile tables the delivered form repeats: `extra_copies` copies follow the table directly and keep its grid; `table_count` grows by them |
| `allow_year_change` (per cell) | Standalone four-digit years 1900-2099 may differ |

Every plan entry carries a stable ID (`[A-Za-z0-9._-]`, at most 64 characters). Findings use those
IDs; the report never echoes the text they stand for.

## What is checked, and from what evidence

| Check | Evidence | Finding code |
|---|---|---|
| Output identity | SHA-256 of the certified input | `output_identity_mismatch` |
| Table count and grid | Parsed document | `table_count_changed`, `table_missing`, `repeated_table_missing`, `table_grid_changed` |
| Expected cell value | Parsed document | `expected_cell_missing`, `expected_cell_value_missing`, `expected_cell_value_mismatch` |
| Expected cell drawn | Segment rows of the certification layout | `expected_cell_not_rendered`, `expected_cell_clipped` |
| Form start present and unique | Parsed document | `form_start_missing`, `form_start_ambiguous` |
| Form start placement | Segment rows of the certification layout | `form_start_not_rendered`, `form_start_on_wrong_page`, `form_start_not_at_page_start` |
| Clipped row, incomplete fragmentation | Typed render issues of the same layout | `table_row_clipped`, `cell_fragmentation_incomplete` |
| Disallowed font substitution | Font resolution of the render, when the policy sets `fonts.forbid_substitution` | `disallowed_font_substitution` |

Placement and fit are read from the layout's own typed issues and segment rows
(`hwp_render::layout_segments_isolated_bounded`: the same isolated font set and layout budget as
the certification render). They are never inferred from text length. A repeated table header does
not mask a missing body row: expected cells are checked one by one, and clipping is reported by
the layout whatever the header does.

Each finding also has a `next_step` code, for example `fill_the_expected_cell`,
`restore_the_page_break_before_the_form` or `pin_the_requested_fonts_and_rerender`.

## Limited results

Page placement depends on line breaking, which depends on fonts. Placement, clipping and
fragmentation are therefore findings only when the font environment is pinned: a font manifest is
supplied, substitution is forbidden, and every requested font matched. Otherwise the check reports
`limited: true` with `pinned_font_environment` in `missing_evidence`, and certification is never
`passed`: a passing but limited check makes the overall result `partial`. Other missing-evidence
codes are `render_layout_unavailable`, `segment_map_truncated`,
`cell_render_location_unavailable` (a table inside a text box) and `parsed_document`. A
substitution the policy allows (`forbid_substitution: false`) is not a finding: the environment
is simply not pinned, so the result is limited.

A plan that cannot be used is refused with one content-free cause class:
`original_form_plan_unreadable` (path, missing file or size), `original_form_plan_not_json`
(not the plan's closed JSON shape), `original_form_plan_contract` (contract or version),
`original_form_plan_bounds` (counts, indices, pages, text length, hashes, or nothing planned)
and `original_form_plan_ids` (malformed or repeated IDs, two cells at one address).

## What stays separate

`checks.original_form` sits beside the existing checks and does not change them. Structural
validity (`package`), semantic preservation (`repeat_import_consistency`, `preservation`), render
quality (`render`) and native-open proof (`hancom_open`) keep their own results.
`render-report.complete` keeps its meaning of diagnostic completeness; nothing here reads it as
visual correctness. No result from this check implies Hancom rendering parity.

## Privacy and the local address map

Reports, refusals and logs carry stable IDs, cause codes and next-step codes only, in every mode.
With `address_map: true` the report directory also gets `original-form/address-map.json`
(`original-form-address-map-v1`): each finding ID mapped to its section/paragraph,
table/row/column and page. It carries no text either, but it locates content, so keep it local
with the plan. It is listed in `manifest.json` and is not one of `report.artifacts`. It shares the
published 258-file manifest cap: a report already at that cap (256 pages plus the oracle PDF)
leaves it out. `address_map` says it was requested and `address_map_published` whether the file
exists. Over MCP, `hwp_certify` writes the same directory and never returns the map
inline.

## Schemas

- `schemas/certification-policy-v1.schema.json` (`document.original_form`)
- `schemas/certification-report-v1.schema.json` (`checks.original_form`)
- `schemas/original-form-certification-plan-v1.schema.json`
- `schemas/original-form-address-map-v1.schema.json`

## Evidence and limits

Coverage is the owner-authored P0 form set (`tools/generate_original_form_fixture.py`, #416):
`crates/hwp-cli/tests/original_form_certify.rs` certifies a correct fill and fails a clipped row,
a missing cell value, a moved form start and a substituted font, each with its own cause; an
unpinned run, and a substitution the policy allows, are limited and name the missing evidence. The pinned cases need one single-face
TrueType font on the host (fonts-nanum on Linux CI, AppleGothic on macOS) and are counted as a
missing resource elsewhere. These are local renderer results, not Hancom output.

A drawn cell border does not prove its text was rendered. Each expected cell paragraph must have drawn WCHAR ranges covering every non-whitespace text character; partial or complete clipping fails with `expected_cell_clipped`. Any earlier top-level body paragraph with a bounding box on the same page, including one containing only a table or object, prevents a page-start anchor. Empty rows without a bounding box do not.
