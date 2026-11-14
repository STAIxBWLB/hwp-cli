[한국어](authoring.ko.md) · [English](authoring.md)

# Authoring documents

How to produce new HWP/HWPX files: from a DocumentSpec or TemplateSpec, as a Korean official document (공문서), or regenerated from the JSON IR. For people and agents writing documents rather than editing one.

## Compose a document from a spec

`compose` compiles a v1 JSON/YAML specification into paragraphs, runs, styles, lists, tables,
equations, fields, headers/footers, page numbers and sections. v2 wraps that document and adds images
with accessibility descriptions, a closed SVG→PNG fallback, and HWPX native rectangular text boxes.
The v2 fallback policy is stated per target; omitting it fails as native-only.

```sh
# Validate schema, references, table spans, assets and native support without writing
ahwp compose examples/document-spec-v1/basic.json -o /tmp/basic.hwpx --dry-run --report

# Publish the validated file atomically
ahwp compose examples/document-spec-v1/comprehensive.yaml -o /tmp/report.hwpx --report
```

`template` allows only a typed `value`/`if`/`each` AST, with no string interpolation and no expression
evaluation. `reference_hwpx` surgically fills only the named text and fields of an existing package;
regenerating structure requires an explicit strict gate.

Normative schemas are in [`schemas/`](../../schemas/) and the contracts in
[13-document-spec-v1](../design/13-document-spec-v1.md),
[14-template-spec-v1](../design/14-template-spec-v1.md) and
[15-document-spec-v2](../design/15-document-spec-v2.md). Examples are in [`examples/`](../../examples/).

## Write an official document (공문서)

Korean official documents follow a fixed notation and layout. That layer is built into the binary
rather than left to a prompt.

**Profiles** (`--preset`) set the margins, numbering and paragraph shapes of a document type:
`official` (기안문·공문서), `report` (보고서), `plan` (사업계획서), `notice` (공고문),
`minutes` (회의록) and `press` (보도자료). Korean aliases (`기안문`, `보고서`, `공고`, …) normalize
onto the same six. Nested lists render the statutory eight-level item marks.

**Templates** (`--template`) go further: each brings its own profile *and* native 두문/결문 tables,
pre-filled with `{{slots}}`.

| Slug | Korean alias | Notes |
|---|---|---|
| `gian-internal` | 기안문-내부결재 | Internal approval |
| `gian-external` | 기안문-대외시행 | External dispatch |
| `gongmun-basic` | 공문서-기본 | Multi-recipient form (`{{수신자}}`) |
| `report` | 보고서 | |
| `plan` | 사업계획서 | |
| `minutes` | 회의록 | |
| `notice` | 공고문 | |
| `press` | 보도자료 | |

**Frames** fill those header and footer blocks directly: `--doc-head 기관명=…`,
`--doc-foot 발신명의=…`, `--notice-head`, `--notice-foot` and `--press-head`. Since v0.10.0 both
`--preset` and the frame flags **override** a template's own defaults instead of being refused.

**Filling** A generated document is itself a template. `ahwp slots` lists what is still unfilled and
`ahwp fill` replaces `{{name}}`, preserving the rest of the hwpx package byte for byte. Both commands
read a slot the same way: the name is any text without braces or control characters
(`{{성 명}}`, `{{사업명(국문)}}`), padding inside the braces (`{{ name }}`) is trimmed, and so are
requested names. Values are inserted literally in one pass, so a value that spells a slot is not
filled again. An unmatched slot is an error and nothing is written unless `--allow-partial` is
given; with it, even a fill that matched nothing publishes the input unchanged and reports zero
counts.

**Form fields** Korean public forms rarely carry slots. `ahwp slots --forms` also lists label cells
(성명, 연락처, ...), inline `라벨: 값` text and slots, each with a key, label, source, confidence
and occurrence count. `ahwp fill --forms` fills them from the same `--data` or `--set` values in one
pass: the cell next to or below a label cell, the text after an inline label, `학년(  )반` blanks,
`□동의` checkboxes and `(비고:   )` blanks. A non-empty value cell, and the inline text after
`라벨:` (up to a comma, semicolon, the next `라벨:` or the line end; a bracket the label sits in,
as in `(한문:   )`, and a bracketed note such as `(서명/인)` stay), is overwritten, as kordoc does;
the report warns for each overwrite. A cell holding a field (누름틀), picture or table is never
written, and values are inserted literally. Keys match with spaces, colons and parentheses
ignored, and `--json` adds the `unmatched` keys. The rules are ported from [kordoc](https://github.com/chrisryugj/kordoc) (MIT).
This path goes through the IR and writes `.hwpx` only: the sections it changed are rewritten the
way `ahwp edit` writes them, and every other section and package entry is copied byte for byte.

**Linting** `ahwp lint` applies ten rules to `.md`, `.hwp` and `.hwpx` files (or to stdin markdown
with `-`): seven notation rules (date `2026. 8. 20.`, time, money, the `붙임:` colon and its
numbering, the closing `끝.`, punctuation), one against decorative item marks (`■ ▶ ▲ ◆ ● ※`, which
belong as `□ ○` or as nested lists), and two structural rules for statutory item marks and
Roman-numeral headings. Only the two structural rules are error severity; every notation finding is
a warning. The command is advisory and exits 0 unless `--strict` is given.

**Table styling** `ahwp edit --style-tables <preset>` applies the profile's table look. Single-column
tables are skipped, and applying it twice produces byte-identical output.

## Regenerate a document from the JSON IR

A document can be recreated from scratch rather than edited in place.

```sh
ahwp convert report.hwpx -o report.json   # hwp/hwpx → JSON IR (--embed-bin also embeds images)
ahwp new --from report.json -o regen.hwpx # JSON IR → new document
```

Regeneration is verified (`crates/hwp-cli/tests/regen.rs`) by validation passing, identical `ahwp cat`
output, an identical table map (count, rows/columns, merges, cell widths) and byte-identical secPr and
tabProperties slices. What still differs is by design: the line layout cache (recomputed by Hancom
Office on open), the preview image and settings.xml.
