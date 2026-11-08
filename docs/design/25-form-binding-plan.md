[한국어](25-form-binding-plan.ko.md) · [English](25-form-binding-plan.md)

# Proposed source-bound form binding plan

**Status: proposal, not implemented.** This draft describes #418 under #411. English is canonical.
`ApplyFormBindings` below is a proposed compound operation, not an available CLI flag, edit op,
MCP tool or approved schema. API and schema approval remain open. Existing command behavior,
including #358 snapshot rules, is unchanged.

## Purpose and boundaries

Discover each fillable occurrence in a supplied native form, let a person review exact targets,
then bind one literal semantic key to every intended location. Reuse existing discovery, clone,
text-splice, cell-edit and atomic publication machinery. Do not regenerate the supplied form or
introduce another editing engine.

This proposal covers structural and semantic edit budgets. It does not prove font resolution,
pagination, clipping, native render quality or Hancom open/export compatibility. Those require
separate evidence under #419 and the P0 contract in [24-original-form-contract.md](24-original-form-contract.md).
Pagination/style work remains under #420; the complete user workflow belongs to #421.

## Occurrence catalogue

The catalogue must keep repeated labels as individual occurrences, including rejected or ambiguous
candidates. The current `scan_form_fields` key/count listing is not an exact-target catalogue.

| Proposed field | Meaning |
|---|---|
| Occurrence ID | Deterministic identity bound to the source hash, model path, kind and source claim; not authorization or a reusable ID for another source |
| Kind | Placeholder, inline value, adjacent/below value cell, parenthesized blank, checkbox or annotation; native named fields await a scope ruling |
| Target | An owning cell coordinate or exact paragraph/text claim, with existing v2 IDs/`SegmentPath` where addressable |
| Container | Table identity, recursive table index, cell origin/spans and enclosing model route; a covered coordinate in a merged cell is not a new owner |
| Profile identity | Caller-assigned logical identity and explicit membership, never inferred from a repeated label alone |
| Evidence | Label location, pattern/target policy, expected prior-value digest and applicable source-coordinate convention |
| Resolution | Resolved, ambiguous or unsupported, with candidate IDs and bounded cause/next-action codes |

Semantic binding keys are literal caller keys. Keep them separate from `normalize_form_label`
and `form::normalize_label`, which implement different legacy matching contracts. Evidence must
state which discovery policy supplied a suggestion; fuzzy matches and overlap precedence are
not permission to select a target automatically. One occurrence cannot receive conflicting keys.

Text locations must distinguish UTF-8 match offsets, `para.chars` indices and WCHAR positions.
Rendered Markdown ranges are not interchangeable with any of them. Convert through the existing
text/splice machinery at one defined boundary and verify the original claim before applying it.

Table/profile IDs identify occurrences in this source. They do not claim a native persistent
identifier. Where #417 extraction was used, retain its source-to-output address map as provenance;
do not guess shifted table indices.

## Source and review binding

A plan carries the exact source SHA-256 plus a structural fingerprint covering selected table
membership, grid/merge/width information and the supported semantic formatting projection.
Fingerprint fields and canonicalization require schema review. A fingerprint supplements the
source hash; it cannot authorize applying to different source bytes.

Review selects explicit occurrence IDs for each key, declared clones and each target's edit
budget. Missing, ambiguous, overlapping, stale or unsupported bindings refuse before publication.
Do not turn the legacy form fill's first-wins warnings into an ambiguity policy for this plan.

## Proposed compound apply

The proposed standalone `ApplyFormBindings` operation would enter the existing `edit-ops-v1`
execution/publication path. It would not replace existing ops or change their selector semantics.
Its bounded request would reference one reviewed plan, literal values and explicit allowances.
Exact wire spelling, limits, CLI/MCP forms and schema approval remain unresolved.

The compound operation would:

- Verify source identity, structural fingerprint, review selections and bounded constraints.
- Perform only declared profile repetition with existing `clone_table` and its ID remapping.
- Recatalogue after cloning, producing explicit old-to-new occurrence/table/profile maps.
- Resolve the reviewed post-clone target set, preview expected changes, and reject ambiguity.
- Apply text splices or cell edits through existing primitives and publish once through the
  existing atomic writer, retaining its refusal and preservation reporting behavior.

The desired profile copy count includes the original: a target of four from one source profile
means three clones. It does not mean append four copies. Clone mode and permitted content are
explicit; existing keep-mode refusal for unsupported Generic objects remains meaningful.

Clone-first recataloguing is an internal, explicit boundary of this proposed compound operation.
It does not relax #358 for existing batches: a previously resolved label coordinate is not valid
merely because a preceding op cloned or moved a table. Post-clone targets need their own reviewed
mapping rule, not arithmetic guesses or fabricated editable IDs.

## Constraints and revised values

Use a closed, bounded integer model for caller-supplied totals: equality and a sum of an explicit
bounded list of integer binding keys compared with a supplied integer total. Reject missing or
non-integer values and checked-arithmetic overflow. No expression strings, scripting, functions,
implicit formulas, inferred subtotals or general evaluator. Exact ranges and list limits await
schema approval. The caller supplies subtotal/total values; a failed check refuses publication.

A reviewed plan may replay revised values against the same original source hash and unchanged
target/clone topology. Recompute constraints and the exact preview for the new values; target
selection and the declared budget remain fixed. Each replay starts from that original source,
so it does not clone an earlier result again. Applying to previous output or any changed source
requires recataloguing, a fresh source-bound plan and review. A matching structural fingerprint
alone is insufficient to bypass that boundary.

## Preview, receipt and edit budget

An explicitly requested local preview may contain exact cells, label evidence, old/new values
and clone maps. Public logs and default receipts contain hashes, occurrence/operation IDs,
counts and bounded cause/next-action codes; they must not echo document text, value strings or
legacy warnings containing original cell contents. Receipt identity binds source, reviewed plan,
values, output and the actual apply result; hashes do not attest layout or native compatibility.

Every target requires a reviewed reflow allowance and semantic edit budget. Declare permitted
value replacements, paragraph-count changes, clone additions and any allowed target formatting
change. Preserve non-target cell text, indentation, paragraph/character properties, grid, merge
spans, widths, borders, signatures and retained resources except explicit allowances. An allowance
must not silently shrink fonts, alter column geometry or accept arbitrary formatting changes.
Long values outside the reviewed budget refuse with the affected occurrence ID and next action.

Existing `set_cell` uses the first paragraph/character shape as its replacement template and
rebuilds the cell's paragraph list. Therefore, mixed-style, multi-paragraph or control-rich targets
cannot be advertised as format-preserving merely because `set_cell` accepts them. Precise text
edits should reuse candidate/splice logic; unsupported budgets require refusal or explicit review,
not a parallel rewriting implementation. This budget is semantic, not evidence that text fits a
page. #419 must evaluate the separate render/font/anchor acceptance evidence.

## Reuse and unresolved decisions

- `form.rs`: preserve candidate-pattern semantics while exposing occurrence evidence separately
  from aggregated `FormField` results. Review legacy normalization and first-wins conflicts.
- `edit.rs`: reuse `walk_form_cells`, `find_form_cells_by_label`, `set_cell`, splice primitives
  and `clone_table`; extend visibility only as needed within the existing engine.
- CLI edit execution: reuse source snapshots, address preflight, offset rules, dry-run/receipts
  and atomic writer. Existing `fill --forms` is HWPX-only, so it is not the both-format executor.
- Native named fields: decide whether they belong in this catalogue; current `list_fields` and
  `set_field` are separate name-based surfaces without per-occurrence target precision.
- Captions and raw-rich targets: current form/table walks omit captions, while some raw-backed
  containers cannot safely regenerate. Catalogue them as unsupported where discovered; do not
  invent segment IDs or promise mutation until a shared traversal/serialization contract exists.
- Approve the compound operation/schema, bounded fingerprint/constraint model, post-clone mapping
  rules and supported edit budgets before implementation.

## Proposed acceptance fixtures

Reuse the owner-authored P0 generator and `expected.json`: exact cell/grid/resource expectations,
repeated labels, merged signatures and four authored profiles. That source already contains four
profiles and does not itself prove cloning from one to four. Add a supplementary one-profile
source derived from the same authored blueprint to exercise three real clones and post-clone maps.
Neither source may be derived from private forms, proprietary fonts or specification extracts.

Reuse P0 `FormProjection`, `ExpectedCell`, `expect_cell_values` and comparisons as test oracles,
not production imports. The existing expectation helper preserves paragraph count and rejects
mixed/control-rich value replacement budgets; broader deliberate changes need an explicit
expected document/projection. Validate both formats, stale/ambiguous atomic refusal, revised
allocation consistency and CLI/MCP parity independently. No result from these synthetic semantic
checks is Hancom open/export evidence.
