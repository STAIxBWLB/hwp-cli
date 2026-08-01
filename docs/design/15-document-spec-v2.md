[한국어](15-document-spec-v2.ko.md) · [English](15-document-spec-v2.md)

# DocumentSpec v2

## Status and source of truth

`schemas/document-spec-v2.schema.json` is the normative input contract and
`schemas/document-report-v2.schema.json` is the normative compose-report contract.
`hwp_cli::document_spec_v2` is their closed Rust/serde implementation. DocumentSpec v1 remains
unchanged and is nested under the v2 `document` property.

Version 2 intentionally freezes only visual operations that the writers and post-write verifier can
prove:

- exact PNG, JPEG, GIF, or BMP embedding;
- deterministic crop/rotation/resize to PNG when fallback is explicitly allowed;
- a bounded, closed SVG geometry subset rasterized to deterministic PNG;
- native inline rectangle text boxes in HWPX.

Charts, diagrams, arbitrary shapes, floating placement, SVG text, and any implicit visual fallback
are outside this version. They require a later schema version after a deterministic font/rendering
or native writer contract exists.

## Target policy

Every visual has independent `policy.hwp` and `policy.hwpx` values:

- `required_native` (default): fail if the target cannot preserve the requested representation
  natively;
- `prefer_native`: use native representation when available, otherwise use only a proven visual
  fallback;
- `force_visual_fallback`: require the proven fallback path.

Omitted targets default to `required_native`. The deprecated CLI/MCP
`allow_visual_fallback` flag remains a DocumentSpec v1 compatibility input. Passing it with v2 is a
typed `policy_conflict`; it never overrides per-target v2 policy.

## Accessibility and placement

`alt` is mandatory. The embedded object description is derived as `title + "\n\n" + alt` when a
distinct non-empty title exists, otherwise as trimmed `alt`. The result must be XML-safe, contain no
carriage return, and fit 65,535 UTF-16 code units. Text-box content has the same character gate.

Version 2 supports `inline` placement only. Multiple objects at one paragraph location retain visual
array order through paragraph control order; inline z-order is canonically zero in both HWP and HWPX.

## Contained assets and SVG fallback

Asset paths contain relative normal components only. The compiler opens them below the spec
directory without following links, rejects hard links, reads a bounded immutable snapshot once, and
uses those same bytes for validation, hashing, transformation, and embedding. A path is limited to
4,096 Unicode scalars and 4,096 UTF-8 bytes. JSON Schema `maxLength` expresses the scalar bound; the
runtime containment gate enforces both bounds.

The SVG subset permits only `svg`, `g`, `rect`, `ellipse`, `circle`, `line`, `polyline`, and `polygon`
with element-specific numeric/color attributes. It rejects DTDs, processing instructions, scripts,
styles, text, paths, transforms, prefixes, external references, resource elements, and unsupported
attributes. Canonical sanitized SVG is parsed with resource lookup disabled and rendered at the
declared output pixel size. Source SVG is never embedded. Reports distinguish source, sanitized SVG,
semantic, and final PNG hashes.

Per-item and aggregate byte, pixel, element, nesting, point, and render-work budgets run before
publication. Empty or fully transparent SVG renders fail.

Crop fields are independently bounded to 0..1 by JSON Schema. Because draft 2020-12 cannot express
arithmetic between sibling properties, the runtime semantic gate additionally requires
`x + width <= 1` and `y + height <= 1`; violations are typed `invalid_crop` errors.

## Execution and semantic verification

CLI and MCP dispatch by the top-level `version` and share the same compile/publish path. Non-dry-run
composition stages output atomically, rejects writer `DROP:` warnings, reopens the staged HWP/HWPX,
and compares the full canonical document projection. The projection normalizes only exact
writer-generated scaffolding and caches. PNG bytes and hash identity, dimensions, derived object
description, inline placement/order, and all other active semantics remain part of the comparison.

The report contains no source path, title, alternative text, description, or output path. It records
stable policy/representation reasons plus semantic, source, sanitizer, and media hashes.
