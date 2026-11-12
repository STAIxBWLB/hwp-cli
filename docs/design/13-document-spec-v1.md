[한국어](13-document-spec-v1.ko.md) · [English](13-document-spec-v1.md)

# DocumentSpec v1

## Status and source of truth

`schemas/document-spec-v1.schema.json` is the normative wire contract for native structured
authoring. `hwp_cli::document_spec` is the versioned Rust/serde implementation. A change to either
requires the other, examples, and contract tests to change in the same commit.

Consumers compiling `schemas/edit-ops-v1.schema.json` must also load
`schemas/document-spec-v1.schema.json` and register it under its `$id`,
`https://hwp-cli.staix.net/schemas/document-spec-v1.schema.json`. EditOps references its block,
style and list definitions; validators may resolve them when compiling any batch, even one
without `insert_blocks`. Load both schemas from the same bundle. The CLI embeds and registers
both resources, so validation needs no network fetch. Insertion compiles blocks against a
header-only copy of the target, so it no longer clones sections or binary streams per operation.
New image streams are grafted by name afterwards: an identical stream is shared and a same-named
stream with different bytes is refused.

Reporting and transport. The edit report lists the inserted segments: one creation pair
(`before` null) for each segment `ahwp cat --segments v2` publishes inside the inserted top-level
paragraphs, read from the document right after the op, so the ids equal what a fresh `cat` shows
and a run id is directly addressable by `set_format`. One exception (#521): an inserted top-level paragraph that
publishes no segment (an empty or break-only one) is named by its paragraph id, which a fresh
`cat` does not list but `move_para` and `delete_para` accept, so every inserted paragraph has an id. `edit-report-v1` caps `changed` at 10,000
items, and a valid request can publish more (one 100 x 50 table is 15,002 segments), so the
report keeps the first 10,000 pairs in document order and states the real count in the optional
`changed_total`; its absence means the list is complete. A coarser report cannot be bounded
either, since one request may carry 20,000 blocks. `hwp_edit` accepts the op as an
`insert_blocks` array (the item is the edit-ops object without its `op` tag, deserialized by the
ops channel's own entry type, so CLI and MCP share one parser). Its image paths resolve against
the `base_dir` argument, itself a sandbox-checked read path; the `--root` roots bind every asset
read. With no `base_dir` image blocks are refused rather than read from the server's working
directory.

The contract is deliberately closed:

- root `version` is exactly `"1.0"`;
- every object rejects unknown properties;
- every union is explicitly tagged by `type`;
- invalid references, unsupported values, non-finite dimensions, and unavailable assets are typed
  errors, never warnings or silent substitutions;
- JSON and YAML map to the same data model;
- object maps use deterministic key ordering and compilation never depends on hash iteration order.

## Authoring model

```text
DocumentSpec
├── metadata
├── page
├── styles: {name -> StyleSpec}
├── lists: {name -> ListSpec}
└── sections[]
    ├── page override
    ├── header/footer: default, first, odd, even block lists
    ├── page_number
    └── blocks[]
        ├── paragraph -> runs[]
        ├── table -> columns[] + rows[].cells[].blocks[]
        ├── image
        ├── equation
        ├── field
        └── break: page | column | section
```

Paragraph runs are `text`, `field`, `equation`, `image`, or `line_break`. Text runs may combine a
named style with explicit run formatting; explicit values win. Paragraphs may reference a named
list and zero-based level. Table cells support rectangular `col_span` and `row_span`; covered cells
must be omitted from later rows/cells.

Ordered lists request the Digit format. HWP compose refuses a declared marker/start that the
existing NUMBERING fallback cannot preserve; every supplied level must match its default marker
and start 1. A referenced level must also use a Digit slot in that record (zero-based 0, 2 or 4).
The native default ladder is `1. 가. 1) 가) (1) (가) ①`, not seven plain-digit levels. The same
check is shared with HWP `insert_blocks`, including table cells and header/footer blocks at
compose time. Use HWPX for custom numbering that the HWP fallback cannot preserve.

Conversion remains allowed: CLI stderr and MCP `ConvertReport.warnings` report
`numbering_default_substitution` with definition/level IDs and the changed template/format/start
categories whenever an unbacked modeled definition becomes that fallback. The warning does not
echo private marker text. Definitions with raw NUMBERING records and modeled levels equal to the
actual defaults do not produce this warning. Levels beyond the fallback's seven slots count only
when a numbered paragraph references that definition and level; unused HWPX or Markdown extension
levels do not warn. Differences within the seven slots still warn even if unused. A successful conversion is not a claim that its
requested list markers were retained. Compose's HWP verification separately checks used levels
before generic fallback projection, so substituted templates/starts or known default format
selectors cannot pass silently even if an authoring guard regresses.

All physical dimensions are decimal millimetres (`*_mm`), font/paragraph spacing is points
(`*_pt`), line height is a percent, colors are `#RRGGBB`, list levels are zero-based at use sites,
and table columns are declared left-to-right. String limits count Unicode scalar values, matching
JSON Schema `maxLength`, rather than UTF-8 bytes. Section `id` values are unique logical diagnostic
keys; HWP/HWPX has no matching payload field, so they are not serialized into the document.
Image assets must be PNG, JPEG, GIF, or BMP. When `height_mm` is omitted, the compiler reads the
pixel dimensions and preserves the intrinsic aspect ratio; invalid or zero-sized headers fail.

## Native-first execution contract

`ahwp compose SPEC -o OUTPUT` follows one pipeline for CLI and MCP:

1. bound and parse JSON/YAML;
2. validate the closed v1 contract and all cross-references;
3. resolve styles and assets relative to the spec file;
4. compile deterministic `hwp_model::Document`;
5. in dry-run mode, return the plan/report without writing;
6. otherwise stage through the shared atomic publisher;
7. reject every writer `DROP:` warning;
8. reopen the staged HWP/HWPX and compare semantic signatures;
9. publish only after verification succeeds.

`deterministic=true` means byte-reproducible output for the same spec, asset bytes, and target
format, including across different output paths and wall-clock times. The report's `output` string
still reflects the caller's requested path and is not part of the document bytes.

Native compilation is the default and the report always sets `native=true`. An unsupported native
request fails with a stable issue code/path. Visual fallback is policy, not an implicit recovery:
it is disabled unless the caller sets `--allow-visual-fallback` (or the MCP boolean), and every
actual fallback must be listed in `visual_fallback_used`. v1 has no request that automatically
falls back, so opting in does not convert an unsupported native feature into success.

The v1 native intersection deliberately fails closed for distinct first-page headers/footers,
non-decimal page numbers, unequal page-number side characters, `keep_with_next`, and non-empty
image `alt`. The last item remains in the schema for forward compatibility but is never silently
dropped; support requires a future Picture description model implemented by both writers.

## Limits

The implementation applies a 4 MiB spec limit, 64 sections, 20,000 blocks, 100,000 runs, 100,000
table grid slots, 16 nested block levels, 2,000,000 text scalars, 64 MiB per asset, and 128 MiB
total unique asset bytes. Names are at most 128 Unicode scalars, short strings 4,096, descriptions
32,768, and equation scripts 100,000. Page/dimension ranges are checked before allocating or
publishing. Every violation is returned as a typed validation issue.

## Compatibility

Minor additions require a new explicit schema version if they introduce a field that v1 readers
would reject. Existing v1 meanings never change. Future versions are separate Rust variants and
schema files; they do not reuse `"1.0"` with permissive parsing.
