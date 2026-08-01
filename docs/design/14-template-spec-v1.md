[한국어](14-template-spec-v1.ko.md) · [English](14-template-spec-v1.md)

# TemplateSpec and TemplateData v1

Status: frozen v1 contract. The normative schemas are
`schemas/template-spec-v1.schema.json`, `schemas/template-data-v1.schema.json`, and
`schemas/template-report-v1.schema.json`.
DocumentSpec v1 remains frozen and is the only native regeneration target.

Frozen SHA-256:

| Schema | SHA-256 |
|---|---|
| TemplateSpec v1 | `590b9ac7dd2b30d1f8fafc4e087adf3117a831f9e38de39267a102141c549039` |
| TemplateData v1 | `484bc86d01dcba17122507fad250791f88235be4dd933c12c721ef7b46eea298` |
| TemplateReport v1 | `aa2f011e02a52b29d07a458f84875e512cf1b1c80e6f2edea40ce756d436f705` |

## Goals

- Render typed data into native HWP/HWPX without string-delimiter substitution.
- Keep expansion deterministic, bounded, non-executable, and diagnosable by JSON Pointer.
- Prefer package-surgical reference HWPX filling when only existing text placeholders or fields change.
- Make regeneration explicit and fail closed when the reference contains unsupported objects.

## Top-level contracts

TemplateSpec:

```json
{
  "version": "1.0",
  "variables": {},
  "source": { "mode": "compose", "document": {} }
}
```

TemplateData:

```json
{
  "version": "1.0",
  "values": {}
}
```

Unknown properties are errors. JSON and YAML map to the same serde model. Values are never coerced:
a quoted number is a string, and YAML spellings such as `yes` do not become booleans in the
template contract.

## Typed variables

The scalar types are `string`, `number`, `bool`, `date`, and `enum`. `rich_blocks` contains native
DocumentSpec v1 block objects. `list` declares a closed scalar field schema and is the input type for
bounded row or block repetition.

Every type supports `required`, `default`, and `secret`. Applicable constraints are:

- string: `regex`, `min_length`, `max_length`
- number: finite `min`, `max`
- date: exact Gregorian `YYYY-MM-DD`, plus `min`, `max`
- enum: 1 to 256 unique strings
- rich_blocks/list: `min_items`, `max_items`

Rust's linear-time regex engine is compiled with a 1,024 Unicode-scalar pattern limit, bounded nesting, and bounded
automata sizes. Diagnostics name the JSON Pointer and rule but never include the rejected value.

Names match `[A-Za-z][A-Za-z0-9_]{0,63}`. `__proto__`, `prototype`, and `constructor` are rejected so
other runtimes can implement the contract without prototype or path-key ambiguity.

## Explicit AST

There is no `${...}`, `{{...}}`, expression language, code execution, dynamic property lookup,
include, macro, or template call in compose/regeneration mode.

Value binding:

```json
{ "node": "value", "pointer": "/values/title", "as": "text" }
```

The only pointers are `/values/<declared-name>` and, within `each`, `/item/<declared-field>`.
`as: native` preserves the JSON scalar type. `as: text` deterministically formats string, number,
boolean, enum, or date values. Rich blocks can only be spliced into a block array and must carry a
unique `region` id.

Conditional region:

```json
{
  "node": "if",
  "condition": "/values/show_summary",
  "region": "summary",
  "then": [],
  "else": []
}
```

Repeated region:

```json
{
  "node": "each",
  "items": "/values/items",
  "region": "item_rows",
  "body": []
}
```

`if` and `each` may occur only as items of DocumentSpec block arrays, header/footer block arrays,
table-cell block arrays, or table `rows`. They cannot alter styles, columns, runs, arbitrary object
properties, or metadata collections. Nested controls are bounded; the language has no recursion.

## Frozen budgets

| Resource | Limit |
|---|---:|
| Template input | 4 MiB |
| Data input | 8 MiB |
| Variables | 1,024 |
| Regex source | 1,024 Unicode scalars |
| One string | 2,000,000 Unicode scalars |
| Rich blocks | 20,000 |
| One list / one each | 10,000 items |
| Total each iterations | 100,000 |
| Control depth | 8 |
| Expanded nodes | 250,000 |
| Expanded JSON | 16 MiB |
| Regions | 20,000 |
| Error envelope | 64 KiB |
| Success report | 64 MiB |

DocumentSpec's own section, block, run, cell, text, and asset budgets are applied after expansion.

## Source modes

### `compose`

The expanded AST must deserialize as frozen DocumentSpec v1. It is passed to the existing compose
compiler and atomic semantic verification path. Writer DROP warnings are fatal.

### `reference_hwpx`

Bindings target an existing placeholder name or HWPX field name. Values must be scalar. The
implementation validates the entire input package before staging, changes only selected section XML,
raw-copies untouched ZIP local records, compressed payloads, and central-directory metadata (only
the required new local-header offset changes), validates the staged package and semantic result, and atomically
publishes. Duplicate targets, missing targets, ambiguous duplicate fields, non-text field regions,
output growth beyond package budgets, and unresolved requested targets fail without changing the
destination.

Placeholder matching is namespace-aware: only character data under local name `t` in the canonical
HWPX paragraph namespace is eligible. A requested placeholder in an attribute, control metadata,
comment, CDATA, non-text node, or foreign namespace fails closed. Field fill accepts only one
unambiguous field whose region contains text and line breaks.

The reference is copied from one opened handle into a private snapshot. Its SHA-256, strict gate,
package patch, and semantic validation all use those same bytes. One command-start destination
snapshot remains authoritative through final atomic publish; a destination race is rejected instead
of becoming a new overwrite baseline.

This is package-surgical preservation, not whole-file byte identity. The report lists changed regions;
it never contains data values.

The preservation boundary is exact but narrow:

- a placeholder must be wholly contained in one canonical HWPX text node; split run/text-node
  placeholders are unresolved and fail the requested-target check;
- a requested field must occur exactly once and contain only `hp:t` text plus `hp:lineBreak`;
- a changed section is reserialized/recompressed, and package-wide offsets/EOCD necessarily change;
- `changed_regions` records validated logical bindings and aggregate instance counts, not a byte-level
  XML diff or a list of every touched ZIP record;
- untouched entry local records, compressed payloads, and central metadata are preserved, but the
  resulting package as a whole is not claimed to be byte-identical.

### `reference_regenerate`

Structural `if`/`each` requests cannot silently leave package-surgical mode. They require the explicit
`reference_regenerate` source mode and literal `strict_unsupported_objects: true`. Before compose,
the reference is loaded and checked for unsupported or opaque content that would be lost. Any such
content fails with `unsupported_reference_object`. Successful output is reported as regeneration, not
preservation.

Regeneration does not merge edits into the reference package. The reference is only an explicit
strict compatibility gate; the output document is compiled entirely from the expanded frozen
DocumentSpec in `source.document`. Content, layout, metadata, or package artifacts not restated there
are not inherited. The gate can reject unsupported/opaque content exposed by the current reader and
writer warnings, but it is not a proof that arbitrary future HWPX extensions are modeled.

## CLI and MCP contract

CLI:

```text
hwp template TEMPLATE --data DATA -o OUTPUT
  [--template-format json|yaml] [--data-format json|yaml]
  [--dry-run] [--report]
```

File paths and reference assets resolve from the TemplateSpec directory. `--dry-run` parses, validates,
expands, checks the reference when applicable, and runs the DocumentSpec compiler, but does not publish.
Compose and reference-regeneration dry-runs report output semantic/package validation as `not_run`.
Package-preserving reference dry-run materializes and validates a private package, so both statuses
are `passed` without touching the destination.

MCP tool `hwp_template` accepts exactly one of `template`/`template_path`, exactly one of
`data`/`data_path`, the same two optional formats, `base_dir` for inline inputs, `output`, `dry_run`,
and no unknown arguments. MCP and CLI use the same executor and error envelope:

```json
{
  "error": "template_spec",
  "issues": [
    { "code": "type_mismatch", "pointer": "/values/count", "message": "..." }
  ]
}
```

Native stderr/MCP errors contain this value-free envelope. Success output is the same report object.

## Preservation report

The machine-readable report contains:

- mode, dry-run, deterministic flag
- SHA-256 of template/data/reference/output bytes
- provided/defaulted variable names, never values
- bounded expansion counts and changed/generated region ids
- unsupported, fallback, and dropped arrays
- template/data, semantic, and package validation status
- nested compose report for regeneration

`fallback` and `dropped` must be empty for success in v1. Hashes describe exact byte inputs and output;
they do not substitute for semantic or package validation.

Repeated or conditional regions are logical aggregates. `input_items` and `generated_items` sum all
nested occurrences, while `instances` records stable paths (`/`, `/0`, `/0/1`, ...) and per-instance
counts. This avoids duplicate logical region ids while preserving deterministic audit detail.
