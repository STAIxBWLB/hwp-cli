[한국어](README.md) · [English](README.en.md)

# Part Composition Example — Template + Part Fill

A workflow for writing large documents (business plans, final reports) part-by-part and
composing them. Prose is written in markdown; non-prose blocks (tables, figures) are
written as HTML fragments (contract:
[docs/design/18](../../docs/design/18-html-fragment-contract.en.md)).

## Files

- `template.md` — the skeleton document. A paragraph containing only `{{name}}` is a part
  anchor.
- `part-overview.md` — a markdown part (prose).
- `part-table.md` — an md + HTML table part (with merged cells).

## Compose

```bash
# 1) Build the template as hwpx
hwp new --from template.md -o template.hwpx

# 2) Splice parts into the anchors (field substitution and part splice in one run)
hwp fill template.hwpx -o result.hwpx \
  --set 작성일=2026-08-01 \
  --set 개요=@part-overview.md \
  --set 실적표=@part-table.md

# 3) Validate
hwp validate result.hwpx
```

## Rules

- An anchor paragraph must contain only `{{name}}`. A `{{name}}` inside a sentence cannot
  be spliced as a block and is only handled as plain field substitution.
- Template and parts must share the hwp-cli default palette family (outputs of `hwp new`
  or `compose`). Parts inherit the template's typography.
- Relative image paths inside a part resolve against the part file's directory. For SVG,
  v1 uses the DocumentSpec v2 svg visual path (`hwp compose`).
