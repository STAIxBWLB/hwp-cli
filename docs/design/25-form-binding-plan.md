[한국어](25-form-binding-plan.ko.md) · [English](25-form-binding-plan.md)

# Source-bound form binding plan

**Status: version 1 implementation.** This contract belongs to #418 under
#411; English is canonical. The compound `apply_form_bindings` API and initial plan contract
have been approved. This document specifies their design, not availability in a released CLI,
MCP tool or binary. Existing operations and #358 snapshot rules remain unchanged.

## Purpose and evidence boundary

Review exact occurrences in a supplied native form, then bind each literal semantic key to all
explicitly selected locations. Reuse existing discovery patterns, clone/splice/cell primitives
and the edit transaction's atomic writer. Do not regenerate the form or add a parallel editor.

This contract supplies structural and semantic edit budgets. Font resolution, pagination,
clipping, render quality and Hancom open/export remain separate evidence under #419 and
[24-original-form-contract.md](24-original-form-contract.md). Reflow permission is not a layout
pass. Pagination/style work belongs to #420 and the complete workflow to #421.

## Discovery contract

`form-discovery-v1.schema.json` matches `FormDiscovery` and `FormOccurrence` in
`crates/hwp-convert/src/form/binding.rs`. Version 1 is closed, requires every serialized field
(including nullable fields), allows at most 20000 occurrences and at most 32 path indices.

| Field | Meaning |
|---|---|
| `id` | `occ-` plus a lowercase 64-digit SHA-256, bound to source hash, model route, kind, claim and prior text digest; not authorization |
| `kind` | `cell_text`, `placeholder`, `inline_value` or `native_field` |
| `paragraph_id`, `section`, `indices`, `model_route` | Existing checksummed paragraph address where supported, plus exact source model coordinates; unsupported captions have no invented editable paragraph ID |
| `table_id`, `table_index`, `row`, `col` | Nullable containing-table identity, discovery table ordinal and owning cell origin; merged covered coordinates are not separate owners |
| `start`, `end` | Half-open `Paragraph::chars` indices, not UTF-8 offsets, WCHAR offsets or rendered Markdown ranges |
| `label`, `old_text`, `prior_sha256` | Local review evidence, exact prior text and its digest |
| `bindable`, `cause`, `next_step` | Target capability, bounded refusal cause and next action; unsupported occurrences retain IDs and evidence |

Every cell paragraph is an inventory target, not an inferred label-value assignment. Neighboring
cell text in `label` is evidence only. The caller chooses explicit occurrence IDs; repeated
labels and overlapping full-cell/placeholder claims remain individually reviewable. `bindable`
means a supported candidate, not uniqueness, automatic ownership or permission to overlap.
Placeholders and body inline values have exact text claims. Cell blanks, checkboxes and
annotations are reviewed through their explicit cell-paragraph targets; discovery does not
silently choose a semantic key or apply the legacy form fill's normalization/first-wins policy.

Caption, raw-preserved and rich targets are inventory-only with cause/next-step evidence. Native
named fields are listed individually as `native_field`, never bindable in this contract; callers
are directed to `hwp fields` and the existing name-based field workflow. A field name is evidence,
not an occurrence address. No caption/raw/native-field mutation is promised by this inventory.

## Source identity and literal keys

The plan carries exact lowercase 64-digit `source_sha256` and `structural_sha256` values. The
structural fingerprint uses the current parsed-IR JSON serialization, including values and
structure; serde-skipped binary payloads remain bound by the original byte hash. It supplements,
and never replaces, the source hash. It is not a universal cross-format semantic equality hash.

Semantic keys are literal caller strings of 1..128 characters, not normalized labels. Keep them
separate from `normalize_form_label` and `form::normalize_label`. Missing, stale, unsupported,
overlapping or multiply owned targets refuse before publication. One `(occurrence_id, copy)`
pair has one owner across the complete plan. Source-to-output addresses from #417 are provenance
for an extracted source, not permission to guess shifted table indices.

## Approved compound apply and repetition

`apply_form_bindings` is a standalone compound operation in the existing `edit-ops-v1` engine.
It is exclusive with other operations in its batch. It verifies one reviewed plan and literal
values, performs declared repeats, resolves explicit targets, applies existing primitives and
publishes once through the existing atomic transaction. This does not change older ops or their
selector semantics. CLI/MCP transport implementation and delivery checks are still in progress.

Plan version 1 contains `bindings`, optional `repeats` and optional `constraints`. Bounds are
256 bindings, 1..256 targets per binding, 16 repeat entries and 128 constraints. Each target
requires `occurrence_id`, `max_chars` (1..20000) and an explicit `allow_reflow` boolean; `copies`
defaults to `[0]`, has unique indices in 0..15 and cannot be empty. Values must have exactly the
binding keys, at most 256 entries and at most 20000 Unicode scalar values per string. LF and TAB
are allowed; CR and other C0 controls are refused. Every target's `max_chars` also applies.

A repeat identifies its source `table_id`, insertion `after_paragraph_id`, total `copies` in
1..16 and exactly that many unique, nonempty caller-assigned `profile_ids` (each at most 128
characters). Profile membership is explicit and never inferred from repeated labels. The count
includes the original: one to four means three clones, not four additions.

Repeat sources and insertion anchors are restricted to top-level body tables/paragraphs;
nested-table repetition is unsupported in this initial contract.

Clone first using existing `clone_table` and ID remapping, then explicitly recatalogue and produce
source-to-copy occurrence/table/profile maps. Plan targets select copies explicitly, with copy 0
being the original. Keep-mode refusal for unsupported Generic content still applies. Do not
assume discovery ordinals are editable table indices for unsupported caption contents.

This compound recatalogue boundary leaves #358 unchanged. Existing batches cannot reuse a label
coordinate after a table-changing op. Post-clone bindings use verified maps, not arithmetic
index shifts, textual anchors guessed from duplicates or fabricated editable IDs.

## Numeric constraints and replay

`equal` compares the numeric values of `left` and `right`. `sum` adds an explicit list of 1..256
key values and compares it with the caller-supplied `total`. All referenced keys must have values.
The grammar is unsigned ASCII digits, optionally comma-grouped in thousands, within `i64::MAX`.
For example, `1,000` and `1000` compare equal. Signs, units, decimal fractions, whitespace and
non-ASCII digits are refused. Arithmetic uses checked addition; overflow or mismatch refuses
publication. There are no expression strings, implicit formulas, inferred totals or evaluator.
Diagnostics identify the rule index and cause without echoing semantic keys or value strings.

Revised values may replay against the same original source hash and reviewed target/clone
topology. Recompute constraints and the exact preview without rediscovering targets by their new
values. Every replay starts from that original source, so it does not clone earlier output again.
Previous output or any changed source requires fresh discovery, a new source-bound plan and
review. A matching structural fingerprint alone cannot authorize changed source bytes.

## Local preview, receipts and semantic budget

An explicitly requested local discovery/preview may contain labels, exact cells, prior/new text
and clone maps. Public logs and default receipts contain hashes, occurrence/operation IDs,
counts and bounded cause/next-action codes. Do not leak discovery text, semantic values or legacy
warnings containing original cell contents. Receipt identity binds source, reviewed plan, values,
output and actual outcome; it does not attest layout or native compatibility.
The receipt records `native_warning_count`; original writer/preview-render messages appear
only in the explicit local preview's `native_warnings`. Font substitution during preview is
not semantic loss and does not supply the separate #419 render/font acceptance.

Every target requires reviewed reflow permission. `max_chars` bounds replacement Unicode scalar
count. With `allow_reflow=false`, replacement WCHAR length cannot exceed the original claim's
WCHAR length. A true value permits that length growth within `max_chars`; it does not permit
font shrinking, grid changes, arbitrary formatting changes or unreviewed paragraph creation.
Neither length check establishes fit, wrapping, page count or clipping correctness.

Preserve non-target text, indentation, paragraph/character properties, grid, merge spans, widths,
borders, signature areas and resources except declared repeats and reviewed value replacements.
Precise paragraph slices reuse existing splice logic. Existing `set_cell` rebuilds a cell from
its first paragraph/character shape; accepting a value there does not prove preservation of
mixed styles, control-rich content or paragraph counts. Unsupported targets refuse rather than
invoke a broader rewriting path. #419 remains responsible for separate font/render/anchor and
native-open acceptance evidence.

## Acceptance fixtures and delivery

Reuse the owner-authored P0 generator and `expected.json` for exact cells, grids, resource roles,
repeated labels, merged signature areas and four authored profiles. That source already has four
profiles; it does not prove actual repetition. Derive a supplementary one-profile source from
the same authored blueprint through shipped #417 heading-range extraction, retaining its real
source-to-output map. Do not manually slice IR paragraph arrays. Exercise three actual clones to
reach four and verify post-clone mappings.

P0 `FormProjection`, `ExpectedCell`, `expect_cell_values` and comparisons remain test oracles,
not production imports. Their bounded paragraph/style rules remain intact; deliberate broader
changes require an explicit expected document/projection. Verify both formats, revised allocation
consistency, stale/ambiguous atomic refusal, unsupported target inventory, CLI/MCP parity and the
complete repository gate. Do not derive fixtures from private forms, proprietary fonts or
specification extracts, or promote synthetic semantic checks into Hancom open/export evidence.
