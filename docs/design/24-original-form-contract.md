[한국어](24-original-form-contract.ko.md) · [English](24-original-form-contract.md)

# Original-form preservation contract

This P0 contract belongs to [#416](https://github.com/STAIxBWLB/hwp-cli/issues/416), under
[#411](https://github.com/STAIxBWLB/hwp-cli/issues/411). It defines bounded comparisons and an
owner-authored synthetic fixture for workflows that keep a supplied native form. English is
canonical. A passing projection is evidence for the fields it checks, not a new native-format rule
or proof that Hancom Office opens the output.

## Scope and delivery boundary

P0 supplies a deterministic authoring specification, expected values, shared test helpers and
negative self-checks. Range extraction (#417) is implemented separately as `ahwp split --from`,
and its tests use these helpers as the oracle. P0 does not implement production binding plans
(#418), original-form certification (#419), pagination/style operations (#420), or the complete
CLI/MCP workflow (#421). Existing production edit, compose, compare and render operations remain
the execution surfaces. A helper's in-memory expected document is a test oracle, not a new public
editing or certification API.

The comparison support uses `FormProjection`, `ExpectedCell`, `ContractViolation`, `project_form`
and `compare_form_contract` under `crates/hwp-cli/tests/common/`. `AnchorPositions` collection and
`compare_anchor_positions` keep render observations separate from the structural projection,
reusing the anchor comparison approach introduced by #410. The tests define exact signatures;
these names do not advertise a shipped CLI or MCP command.

`expect_cell_values` clones and adjusts only an expected projection; it never edits the source
`Document`. Its closed replacement budget keeps the existing paragraph count and accepts only
single-style paragraphs. Paragraph insertion or mixed-style replacement requires an explicit
expected document/projection rather than a wider text allowance.

The string-valued cell budget supports ordinary text, tabs and line breaks. Control-rich cell
values retain typed control atoms in the projection and require an explicit expected document;
they cannot be flattened into a string match or silently replaced. Character controls and inline
controls remain distinct atom variants even when their numeric codes match. Extended-control atoms
retain the code, `ctrl_id` and referenced control index; changing a field/object anchor cannot be
hidden by unchanged visible text. Only an optional trailing paragraph terminator is normalized; interior paragraph-break controls
remain atoms. Raw native payload tails remain outside this semantic projection.

The generated-source test compares full HWPX twin projections. Both formats independently check
manifest cell values, grid widths/spans, form markers/breaks and resource roles. HWP5 then has its
own full projection baseline and byte-identical native round-trip check. This does not assert
universal cross-format style equality or normalize writer materialization differences away.

## Reproduce the synthetic source

From the repository root:

```sh
python3 tools/generate_original_form_fixture.py target/original-form-fixture
ahwp compose target/original-form-fixture/original-form.spec.json \
  -o target/original-form-fixture/original-form.hwpx
```

The standard-library-only generator emits these four files:

- `original-form.spec.json`: DocumentSpec 2.0 wrapping a 1.0 document, with floating tables.
- `inside.png` and `outside.png`: distinct original 2x2 RGB checker images, encoded deterministically
  without external artwork or a font dependency.
- `expected.json`: versioned fixture provenance, spec/image SHA-256 values, form markers and
  paragraph indices, table identities, cell coordinates/spans/widths and expected text values.

The whole source contains an unrelated earlier image, an application form, a development report
with three lecture/practice allocations, four profile tables, repeated labels and merged signature
areas. The retained image is inside an application-table cell. Each form/profile has a unique
`FORM_START_*` heading preceded by a page-break block. The break is a separate compiled paragraph;
the heading itself must not be assumed to carry the break flag. The manifest's coordinates refer
to this compiled source, including its section-setup paragraph, not to future extracted outputs.

All styles specify zero before/after spacing. This avoids relying on the unresolved spacing-unit
calibration in #415. It does not establish a general zero-spacing or page-count rule. The generator
requests Noto Sans CJK KR but bundles no font and pins no resolved font identity. Spec/assets can
be reproduced byte-for-byte; native artifact and render evidence must additionally name the
product head and font environment used to create them.

No private or third-party source form, source-derived binary fixture, proprietary font or Hancom
specification derivative is an input. The generated native files are outputs of existing compose
and writer paths, not hand-authored HWP record bytes. Changing the output extension or converting
with existing commands exercises a second format; it does not supply native-open evidence.

## Comparison layers

| Layer | Checked contract | Evidence boundary |
|---|---|---|
| Cell values | Exact expected values at explicit table/grid coordinates, with paragraph boundaries and whitespace retained | Repeated labels are not unique addresses; plain-text totals alone cannot prove the right cell was filled |
| Grid | Table/cell counts, row/column coordinates, merge spans, widths, stored minimum heights and the authored grid | A missing/extra table or cell fails; positional `zip` must not discard unmatched entries |
| Semantic style | Resolved common properties described below | Palette indices and opaque serialization details are not formatting identities |
| Resources | Resolved payload hashes and their referring locations; explicit retained/excluded roles | A filename or unresolved reference is not the asset bytes |
| Pagination | Break/anchor constraints and same-environment relative observations | CI does not assert portable golden pages, glyphs or pixels |
| Fonts | Original requests plus separately recorded resolution outcomes and byte identities | A matching family name alone does not pin the actual font |
| Native opening/export | Person-run receipt for the exact delivered artifact | Structural validity, an empty loss ledger and CLI rendering are insufficient |

A grid address includes the containing table identity and the cell's anchor row/column; merged
coverage is recorded separately from that anchor. Labels such as `Course`, `Year` and `Signature`
remain values to preserve. The four profile occurrences must remain individually identifiable.
Source/output address mapping after extraction or cloning is a later package's responsibility.

## Semantic style projection

Resolve references through each document's own header and compare values, not numeric palette IDs.
The common subset comprises paragraph alignment, indent, left/right margins and before/after/line
spacing; each character run's requested face names, size, bold/italic state and text color; resolved
border/fill values; cell vertical-alignment attributes; and the table's semantic page-break policy
(`none`, `table` or `cell`, resolved by `Table::page_break_policy`). This compares the policy
meaning rather than every raw table-attribute bit. Named-style assignments resolve through
`header.styles` and compare their name/English-name identity, even when direct formatting is equal;
a dangling `StyleId` is an unresolved reference, not an unnamed default. This is a bounded named-style
identity check, not a universal comparison of every native style payload.

Canonical run boundaries must preserve actual format changes without treating redundant adjacent
equal runs as a change. Their WCHAR content extent counts interior paragraph-break controls; only
an optional final `CharCtrl(PARA_BREAK)` terminator is normalized away. A formatting run after an
interior break remains part of the projection.

Do not normalize different field meanings merely to obtain equality. A cross-format distinction
that the shared IR cannot express or a writer changes must remain an explicit limitation or a
narrow, reviewed expected change. Unknown references are violations, not default styles silently
substituted by the helper. Tab-definition index 0 is validated like every other index; zero does
not exempt an absent tab-definition reference from the unresolved-reference check.

Line caches, instance IDs, container metadata, raw opaque records and native field tails are not
part of this semantic subset. They remain protected by the independent HWP5 byte-identity,
source-preserving writer and typed preservation-ledger gates where applicable. An unchanged
projection cannot certify those layers.

## Expected changes and resource selection

The baseline comparison permits no undeclared change. An intentional fill names exact
`ExpectedCell` coordinates and values, or supplies an explicit expected document/projection for
that operation. Table repetition and deliberate year-label changes must be represented in that
expectation with the resulting counts and addresses. There is no ignore-all switch for text,
merges, widths, style or resources. Non-target cells and signature areas still compare against the
baseline.

The whole-source resource inventory contains both images. An extraction plan must explicitly
retain the in-form image and name the unrelated outside image as deliberately excluded. The P0
fixture/manifest identifies those roles; it does not silently prune resources or implement resource
closure. A missing referenced asset is a failure, distinct from planned exclusion. Hash resolved
payload bytes rather than HWP/HWPX container names, which may change on conversion.

The authored fixture checks the retained image's table identity as well as its cell: resolve the
manifest location's `table_id` through the authored table list, use that entry's `table_index` in
the projection, and require the image path to have that table's key and row/column prefix. The same
row/column in another table is a failure even if its payload hash matches. The outside image is
checked against its manifest section/paragraph address. These are source-fixture role checks, not
a production mapping of addresses after extraction.

The JSON export path `convert --to json --embed-bin` is the existing self-contained path. A JSON
dump without payload bytes is not a complete image round-trip. Passing a semantic/resource
projection also does not permit dropping unrelated opaque package entries to make strict
conversion succeed.

## Anchor, font and native evidence

Structural checks record explicit break intent independently of render positions. Anchor
collection associates form markers and table fragments with pages/boxes through the renderer's
segment map. Compare corresponding source/output observations in one font environment, require
all expected anchors to exist, and reject incomplete maps. Do not let two empty anchor sets pass
as equal. Missing text, repeated headers or a total page count cannot substitute for complete
body-cell marker coverage.

Before comparing geometry, `anchor_positions` derives the expected table-ID set from the existing
`to_markdown_with_segments_v2` document walk with default options, without image extraction or
image I/O. The observed `table` rows must have exactly that ID set, giving each expected table
at least one observed row and rejecting missing or unexpected table evidence even when both
render maps omit the same table. This is a bounded table-presence check; it does not prove that
every table row or continuation fragment rendered, or that pagination matches Hancom.

CI comparisons are relative: the two artifacts use the same available font environment, and the
checks compare their corresponding anchors rather than a hardcoded page count or pixel image.
The current helper compares selected body paragraph page assignments and table-fragment page/box
positions. Table-box coordinates must be finite and differ by **less than 0.05pt**, following the
#410 helper; this is a relative tolerance, not a portable coordinate golden. If a host resource prevents an
applicable observation, record the unavailable evidence through skip accounting or return a
limited result; strict required-resource mode must fail rather than claim coverage.

`FontIdentity` records actual requested family/bold state, resolved family, font-byte SHA-256,
face index and matched/substituted/missing outcome from `FontStore` observations. Its repeatability
comparison checks that observed identities agree; it neither pins a CI font nor turns matching
fallback observations into a matched-font or native-compatibility verdict. CI font observations
are explicitly labeled observed and externally unpinned. The identity also carries
`resolutions_complete`. A truncated collection is unavailable evidence and is rejected even when
the collected entries and completeness flags on both sides are equal.

Missing font-byte hashes, face indices or resolution outcomes are unavailable evidence, not equal
font identities. Required tests count that unavailable resource; strict mode fails it.

Page/pixel acceptance needs an external receipt recording the source/spec/output and render
hashes, product head/version, original requested family, resolved font-byte SHA-256, face index
and matched/substituted/missing outcome. Retain the original requests. Without pinned bytes or a
complete resolution result, report font/page evidence as limited; do not label fallback output
Hancom parity.

A separate person-run Hancom receipt names the native output hash, Hancom version/platform,
open result, export result and exported artifact identity. It remains unverified until that action
is performed. CLI validation, semantic preservation, diagnostic completeness, rendering and
native-open/export observations are distinct results; `render-report.complete` means diagnostic
collection completeness, not visual correctness.

## Known scope limits and negative checks

#410's bounded synthetic result is a cacheless-source CLI observation. Native HWP5 edits retain
the base cursor path rather than receiving the new floating fragment-anchor override or planner
page-start bit. P0 does not expand that policy or infer genuine-layout correctness from the probe.
The following separate investigations constrain broader claims:

- #415: paragraph-spacing unit calibration.
- #422: empty-body/cell fixed advancement versus synthesized metrics and empty-body anchors.
- #423: header/footer body bounds and first-anchor versus later multiline split rules.
- #424: genuine floating-section native-edit evidence after the relayout opt-out.
- #425: native horizontal/vertical sentinel-margin interpretation, requiring genuine evidence.
- #426: discarded anchor-prepass diagnostics and incomplete/default anchors.

Self-checks deliberately change an expected cell, a merge span and a form-start observation; each
must yield a layer-specific `ContractViolation`. Negative checks also change a table's page-break
policy, remove table evidence from otherwise usable render maps, and move the retained image to
the same cell coordinates in another table. Each must fail its corresponding bounded check.
Grid/style/resource checks must likewise detect
relevant mutations rather than relying on one aggregate document-text equality. Test-only broken
copies demonstrate the helper's rejection boundary; they are not production extraction or binding
features. Full `scripts/check.sh`, hosted CI, fixture skips and native/oracle evidence are reported
separately for the final tested head. No P0 result supplies universal native-table PDF support.
