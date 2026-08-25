<!-- Generated document. Do not edit by hand. Regenerate with: HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference -->

[한국어](cli-reference.ko.md) · [English](cli-reference.md)

# hwp CLI command reference

This document is generated from the clap definitions of the `hwp` CLI. Do not edit it by hand: when a command or flag changes, regenerate it with `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`. A CI test enforces that it stays in sync with the code.

## Command index

- [`hwp info`](#hwp-info)
- [`hwp cat`](#hwp-cat)
- [`hwp grep`](#hwp-grep)
- [`hwp convert`](#hwp-convert)
- [`hwp render`](#hwp-render)
- [`hwp new`](#hwp-new)
- [`hwp compose`](#hwp-compose)
- [`hwp template`](#hwp-template)
- [`hwp diff`](#hwp-diff)
- [`hwp edit`](#hwp-edit)
- [`hwp fields`](#hwp-fields)
- [`hwp bookmarks`](#hwp-bookmarks)
- [`hwp slots`](#hwp-slots)
- [`hwp fill`](#hwp-fill)
- [`hwp validate`](#hwp-validate)
- [`hwp lint`](#hwp-lint)
- [`hwp certify`](#hwp-certify)
- [`hwp corpus`](#hwp-corpus)
- [`hwp mcp`](#hwp-mcp)
- [`hwp update`](#hwp-update)
- [`hwp skill`](#hwp-skill)
- [`hwp skill export`](#hwp-skill-export)
- [`hwp dump`](#hwp-dump)

## `hwp info`

Show file information: format, version, properties and stream list

**Usage:** `hwp info [OPTIONS] <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<FILE>` |  |  | Target HWP/HWPX file |
| `--json` |  |  | Print as JSON |

## `hwp cat`

Extract text

**Usage:** `hwp cat [OPTIONS] <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<FILE>` |  |  | Target HWP/HWPX file |
| `--format` | `plain` \| `markdown` \| `json` \| `html` \| `csv` | `plain` | Output format |
| `--preview` |  |  | Print only the PrvText preview, without parsing the body |
| `--with-header-footer` |  |  | Also extract header and footer text (default: excluded) |
| `--with-hidden` |  |  | Also extract hidden comment text (default: excluded) |
| `--with-segments` |  |  | (markdown only) Emit the markdown together with the source coordinates (section/paragraph) of each output character range, as a one-line JSON envelope: {"markdown": ..., "segments": [...]} |

## `hwp grep`

Search paragraph text (grep semantics; non-zero exit when no match)

**Usage:** `hwp grep [OPTIONS] <PATTERN> <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<PATTERN>` |  |  | Pattern to find (substring match) |
| `<FILE>` |  |  | Target HWP/HWPX file |
| `--ignore-case` |  |  | Case-insensitive match |

## `hwp convert`

Convert between formats

**Usage:** `hwp convert [OPTIONS] <INPUTS>...`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<INPUTS>` |  |  | Input HWP/HWPX files ("-" reads stdin; multiple inputs require --out-dir) (repeatable) |
| `-o, --output` | `<OUTPUT>` |  | Output file path ("-" writes stdout for text formats: md/json/html/txt/csv; required with a single input) |
| `--out-dir` | `<OUT_DIR>` |  | Output directory for multiple inputs (file names are "<stem>.<ext>", requires --to) |
| `--to` | `hwp` \| `hwpx` \| `md` \| `json` \| `html` \| `pdf` \| `odt` \| `txt` \| `csv` \| `docx` |  | Output format (inferred from the extension when omitted) |
| `--strict` |  |  | Fail when data that cannot be preserved (opaque) is found during conversion |
| `--loss-report` | `<LOSS_REPORT>` |  | Write the typed preservation ledger (hwp-preservation-report-v1) as JSON to this path, even when the conversion succeeds without loss (single input only). The preservation inspection only runs for hwp/hwpx targets — for other output formats (docx, md, ...) the ledger is always empty |
| `--preserve-layout` |  |  | Preserve the line layout cache (unmodified round-trips only; Hancom treats a layout inconsistent with the content as tampering, so it is dropped by default) |
| `--embed-bin` |  |  | Embed attached binaries (images) as base64 in JSON output (self-contained JSON) |
| `--media-dir` | `<MEDIA_DIR>` |  | (md) Image extraction directory, default "<output stem>.media". A relative path resolves against the output file and links use the path as given (e.g. figs) |
| `--with-header-footer` |  |  | (md) Also include header and footer text (default: excluded) |
| `--with-hidden` |  |  | (md) Also include hidden comment text (default: excluded) |
| `--font-dir` | `<FONT_DIR>` |  | (pdf) Additional font directory (repeatable; defaults to HWP_FONT_DIR or fonts/) |

## `hwp render`

Render pages

**Usage:** `hwp render [OPTIONS] --output <OUTPUT> <INPUT>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<INPUT>` |  |  | Input HWP/HWPX file |
| `-o, --output` | `<OUTPUT>` |  | Output file path |
| `--pages` | `<PAGES>` | `all` | Page range: "1", "1-3", "all" |
| `--dpi` | `<DPI>` | `96` | Resolution in DPI (finite, 36..=600) |
| `--format` | `png` \| `svg` \| `pdf` |  | Output format (inferred from the extension when omitted) |
| `--report` | `<REPORT>` |  | Write a closed machine-readable render report atomically |
| `--font-dir` | `<FONT_DIR>` |  | Additional font directory (repeatable) |

## `hwp new`

Create a new document

**Usage:** `hwp new [OPTIONS]`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `-o, --output` | `<OUTPUT>` |  | Output HWP/HWPX path (not required with --list-templates) |
| `--from` | `<FROM>` |  | Input markdown or JSON file (empty document when omitted) |
| `--template` | `<TEMPLATE>` |  | Use an embedded document template by English slug or Korean alias (see --list-templates). Brings its own profile and native 두문/결문 frames, whose values default to the template's own {{slots}}. Mutually exclusive with --from; --preset and the frame flags override one template default each |
| `--list-templates` |  |  | List all embedded document templates (slug and Korean alias) and exit; needs no -o |
| `--set-meta` | `<SET_META>` |  | Set metadata "key=value" (keys: title\|author\|subject\|keywords; repeatable) |
| `--preset` | `<PRESET>` |  | Official-document profile (markdown input only): official, report, plan, notice, minutes, or press. Legacy and Korean aliases normalize to one profile |
| `--margin-top` | `<MARGIN_TOP>` |  | Top page margin in millimetres (0..=200) |
| `--margin-bottom` | `<MARGIN_BOTTOM>` |  | Bottom page margin in millimetres (0..=200) |
| `--margin-left` | `<MARGIN_LEFT>` |  | Left page margin in millimetres (0..=200) |
| `--margin-right` | `<MARGIN_RIGHT>` |  | Right page margin in millimetres (0..=200) |
| `--strict` |  |  | Fail (non-zero exit) when markdown import drops content, e.g. an HTML block that violates the import contract. Default: warn and continue (exit 0) |
| `--doc-head` | `<DOC_HEAD>` |  | Document header block (두문) "key=value" (keys: 기관명\|수신\|경유; repeatable) |
| `--doc-foot` | `<DOC_FOOT>` |  | Document footer block (결문) "key=value" (keys: 발신명의\|기안자\|검토자\|결재자\|협조자\| 시행번호\|시행일자\|접수번호\|접수일자\|주소\|홈페이지\|전화\|팩스\|이메일\|공개구분\|수신자; repeatable. 수신자 is the recipient list of a document whose 두문 reads "수신자 참조", and is the one 결문 row emitted only when supplied) |
| `--notice-head` | `<NOTICE_HEAD>` |  | Notice (공고문) header block "key=value" (keys: 기관명\|공고번호; repeatable) |
| `--notice-foot` | `<NOTICE_FOOT>` |  | Notice (공고문) footer block "key=value" (keys: 공고일자\|발신명의; repeatable) |
| `--press-head` | `<PRESS_HEAD>` |  | Press release (보도자료) header block "key=value" (keys: 기관명\|보도시점\|배포일\| 담당부서\|담당자\|연락처; repeatable) |

## `hwp compose`

Compose a structured document deterministically from DocumentSpec v1/v2 (JSON/YAML)

**Usage:** `hwp compose [OPTIONS] --output <OUTPUT> <SPEC>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<SPEC>` |  |  | DocumentSpec v1/v2 input file (.json, .yaml, .yml) |
| `-o, --output` | `<OUTPUT>` |  | Output HWP/HWPX |
| `--format` | `json` \| `yaml` |  | Input format (inferred from the spec extension when omitted) |
| `--dry-run` |  |  | Produce the validation and compilation report without writing the file |
| `--report` |  |  | Print the run report as JSON |
| `--allow-visual-fallback` |  |  | [deprecated] v1 compatibility only; v2 rejects this policy override |

## `hwp template`

Generate typed native HWP/HWPX from TemplateSpec/Data v1

**Usage:** `hwp template [OPTIONS] --data <DATA> --output <OUTPUT> <TEMPLATE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<TEMPLATE>` |  |  | TemplateSpec v1 input file (.json, .yaml, .yml) |
| `--data` | `<DATA>` |  | TemplateData v1 input file (.json, .yaml, .yml) |
| `-o, --output` | `<OUTPUT>` |  | Output HWP/HWPX |
| `--template-format` | `json` \| `yaml` |  | TemplateSpec input format (inferred from the extension when omitted) |
| `--data-format` | `json` \| `yaml` |  | TemplateData input format (inferred from the extension when omitted) |
| `--dry-run` |  |  | Run the real expansion, writer and validation paths without publishing the result |
| `--report` |  |  | Print the preservation and expansion report as JSON |

## `hwp diff`

Compare a render against a Hancom reference PNG (offset and pixel difference)

**Usage:** `hwp diff [OPTIONS] --ref <REF> <INPUT>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<INPUT>` |  |  | Input HWP/HWPX file |
| `--ref` | `<REF>` |  | Reference PNG exported from Hancom for the same page at the same DPI |
| `--page` | `<PAGE>` | `1` | Page to compare (1-based) |
| `--dpi` | `<DPI>` | `96` | Resolution in DPI (finite, 36..=600) |
| `-o, --out` | `<OUT>` |  | Difference image output path (defaults to <ref>.diff.png) |
| `--font-dir` | `<FONT_DIR>` |  | Additional font directory (repeatable) |
| `--tolerance` | `<TOLERANCE>` | `16` | Per-channel tolerance; differences at or below this count as equal |
| `--format` | `text` \| `json` | `text` | Report output format (json = machine-readable, for the parity batch runner) |
| `--ours-png` | `<OURS_PNG>` |  | Compare this raster (e.g. pdftoppm of our PDF) against --ref instead of rendering the input document; the input path is only recorded in the report |

## `hwp edit`

Edit an existing document (text replacement, table cells); images and formatting preserved

**Usage:** `hwp edit [OPTIONS] --output <OUTPUT> <INPUT>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<INPUT>` |  |  | Input HWP/HWPX file |
| `-o, --output` | `<OUTPUT>` |  | Output file path |
| `--replace` | `<REPLACE>` |  | Replace text, "find=>replace" (repeatable; replaces every match) |
| `--set-cell` | `<SET_CELL>` |  | Set a table cell, "table:row:col=value" (repeatable; 0-based indices) |
| `--set-field` | `<SET_FIELD>` |  | Fill a field, "name=value" (repeatable; list names with hwp fields) |
| `--set-meta` | `<SET_META>` |  | Set metadata, "key=value" (keys: title\|author\|subject\|keywords; repeatable) |
| `--create-field` | `<CREATE_FIELD>` |  | Create a field, "anchor=>name" or "anchor=>name=value": insert a %clk field after the anchor text (repeatable) |
| `--create-bookmark` | `<CREATE_BOOKMARK>` |  | Create a bookmark, "anchor=>name": insert a bokm marker after the anchor text (repeatable) |
| `--create-hyperlink` | `<CREATE_HYPERLINK>` |  | Create a hyperlink, "anchor=>URL" or "anchor=>text=>URL": insert %hlk after the anchor (repeatable) |
| `--insert-image` | `<INSERT_IMAGE>` |  | Insert an image, "anchor=>path" or "anchor=>path@WxH" (mm): insert a picture after the anchor (repeatable) |
| `--seal` | `<SEAL>` |  | Stamp a seal, "anchor=>path" or "anchor=>path@size" (mm): float the seal over the anchor text (repeatable) |
| `--set-format` | `<SET_FORMAT>` |  | Character formatting, "find:property=value,..." (for example "Title:bold=on,size=16,color=#FF0000") (repeatable) |
| `--set-align` | `<SET_ALIGN>` |  | Paragraph alignment, "find=alignment" (left/right/center/justify/distribute) (repeatable) |
| `--insert-para` | `<INSERT_PARA>` |  | Insert a paragraph, "anchor=>text": after the paragraph containing the anchor (repeatable) |
| `--insert-para-before` | `<INSERT_PARA_BEFORE>` |  | Insert a paragraph before, "anchor=>text": before the paragraph containing the anchor (repeatable) |
| `--delete-para` | `<DELETE_PARA>` |  | Delete a paragraph, "text": delete the paragraph containing the text (repeatable) |
| `--add-row` | `<ADD_ROW>` |  | Add table rows, "table[:at[:count[:template_row]]]": at omitted or "end" appends, a number inserts before that row; count defaults to 1; template_row donates row height and cell/paragraph/character styling, never text (repeatable, 0-based; merged tables supported) |
| `--add-col` | `<ADD_COL>` |  | Add table columns, "table[:at[:count]]": at omitted or "end" appends, a number inserts before that column; count defaults to 1; total width is preserved by shrinking existing columns evenly. Merged tables supported (repeatable, 0-based) |
| `--delete-row` | `<DELETE_ROW>` |  | Delete a table row, "table:row" (repeatable, 0-based; a merged row is refused) |
| `--delete-col` | `<DELETE_COL>` |  | Delete a table column, "table:col": total width is preserved by redistributing to the remaining columns; merged cells shrink (repeatable, 0-based) |
| `--merge-cells` | `<MERGE_CELLS>` |  | Merge cells, "table:r1:c1:r2:c2": merge a rectangular area into its top-left anchor (repeatable, 0-based) |
| `--split-cell` | `<SPLIT_CELL>` |  | Split a cell, "table:row:col": break a merged cell back into 1x1 cells (repeatable, 0-based) |
| `--add-table` | `<ADD_TABLE>` |  | Insert a table, "anchor=>json": insert a uniform table after the anchor paragraph; json is an array of row arrays (repeatable) |
| `--clone-table` | `<CLONE_TABLE>` |  | Clone a table, "source_table=>anchor[=>blank\|keep]": deep-copy table source_table (0-based, recursive) after the anchor paragraph; blank (default) keeps structure/styles with empty cells, keep also clones supported content (nested tables, images) with remapped ids (repeatable) |
| `--set-para` | `<SET_PARA>` |  | Paragraph shape properties, "find=>key:value" (keys: line-spacing (% or Npt), indent, left, right, top, bottom (mm); repeatable) |
| `--set-page` | `<SET_PAGE>` |  | Page setup, "key:value" (keys: width, height, margin-left, margin-right, margin-top, margin-bottom (mm), orientation (portrait\|landscape); repeatable) |
| `--delete-image` | `<DELETE_IMAGE>` |  | Delete an image, "anchor": delete the picture in the anchor paragraph (repeatable) |
| `--delete-table` | `<DELETE_TABLE>` |  | Delete a table, "n" (0-based index) or "anchor" (table in the anchor paragraph) (repeatable) |
| `--delete-field` | `<DELETE_FIELD>` |  | Delete a field by name, "name" (repeatable; list names with hwp fields) |
| `--delete-bookmark` | `<DELETE_BOOKMARK>` |  | Delete a bookmark by name, "name" (repeatable; list names with hwp bookmarks) |
| `--style-tables` | `<STYLE_TABLES>` |  | Style every eligible table (header shade/bold/center, content-proportional widths) under an official-document preset: official, report, plan, notice, minutes, or press. Skips single-column tables (frame blocks); byte-stable when applied twice |
| `--verify` |  |  | Verify by re-reading after writing |
| `--allow-partial` |  |  | Publish the matched edits even if some requests found no target (default: fail if any is unapplied) |

## `hwp fields`

List fields (name, kind, value)

**Usage:** `hwp fields [OPTIONS] <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<FILE>` |  |  | Target HWP/HWPX file |
| `--json` |  |  | Print as JSON |

## `hwp bookmarks`

List bookmarks (name)

**Usage:** `hwp bookmarks [OPTIONS] <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<FILE>` |  |  | Target HWP/HWPX file |
| `--json` |  |  | Print as JSON |

## `hwp slots`

List `{{name}}` text placeholders (template slots)

**Usage:** `hwp slots [OPTIONS] <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<FILE>` |  |  | Target HWP/HWPX file |
| `--json` |  |  | Print as JSON |

## `hwp fill`

Fidelity-preserving template fill (replace `{{name}}` in hwpx, package preserved)

**Usage:** `hwp fill [OPTIONS] --output <OUTPUT> <INPUT>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<INPUT>` |  |  | Input HWPX template |
| `-o, --output` | `<OUTPUT>` |  | Output file path |
| `--set` | `<SET>` |  | Fill a placeholder, "name=value" (repeatable; replaces `{{name}}`). "name=@part.md" splices a part file (markdown + HTML table blocks, docs/design/18 contract) into the `{{name}}` anchor paragraph instead — part-based composition for large documents. "@@" escapes a literal '@' |
| `--data` | `<DATA>` |  | JSON object file mapping name to value (bulk fill; "parts": {"name": "path"} splices part files, "tables": [...] fills table rows) |
| `--json` |  |  | Print the replacement summary as JSON ({output, replaced, counts}) |
| `--allow-partial` |  |  | Publish the matched values even if some requests found no placeholder (default: fail if any is unreplaced) |

## `hwp validate`

Structural validation (mimetype, required entries, XML parsing); exit code 0 when valid

**Usage:** `hwp validate [OPTIONS] <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<FILE>` |  |  | Target HWP/HWPX file |
| `--json` |  |  | Print as JSON |

## `hwp lint`

Lint official-document notation and structure rules; advisory by default (always exit 0) — --strict exits 1 only when an error-severity finding exists

**Usage:** `hwp lint [OPTIONS] <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<FILE>` |  |  | Target .md/.hwp/.hwpx file ("-" reads stdin as markdown) |
| `--profile` | `gongmun` \| `report` | `gongmun` | Lint profile: gongmun (default) or report; both run the same rule table in v1 |
| `--json` |  |  | Print the hwp-lint-report-v1 JSON report |
| `--strict` |  |  | Exit 1 when an error-severity finding exists (default: always exit 0) |

## `hwp certify`

Certify package, semantics, native render and independent import under a versioned policy

**Usage:** `hwp certify --policy <POLICY> --report <REPORT> <INPUT>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<INPUT>` |  |  | HWP/HWPX input to certify |
| `--policy` | `<POLICY>` |  | hwp-certification-policy-v1 JSON/YAML |
| `--report` | `<REPORT>` |  | Atomic artifact directory to create (an existing path is refused) |

## `hwp corpus`

Generate the frozen structured corpus twice, reopen it and certify natively

**Usage:** `hwp corpus --manifest <MANIFEST> --report <REPORT>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `--manifest` | `<MANIFEST>` |  | hwp-structured-corpus-v1 manifest JSON |
| `--report` | `<REPORT>` |  | Atomic run report directory to create (an existing path is refused) |

## `hwp mcp`

MCP (Model Context Protocol) stdio server: a tool interface for AI agents

**Usage:** `hwp mcp [OPTIONS]`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `--font-dir` | `<FONT_DIR>` |  | Default font directory for the render and diff tools (repeatable) |
| `--root` | `<ROOT>` |  | Restrict all file access to this directory (repeatable). Default: unrestricted |

## `hwp update`

Self-update: fetch the latest `hwp` from GitHub releases and replace the running binary

**Usage:** `hwp update [OPTIONS]`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `--check` |  |  | Report the current and latest versions without replacing |
| `--tag` | `<TAG>` |  | Pin a specific release (for example "v0.2.0", to roll back) |
| `--force` |  |  | Re-download and replace even at the same version (to repair a broken install) |
| `--json` |  |  | Print as JSON |

## `hwp skill`

Manage the bundled agent skill (SKILL.md for AI coding assistants)

**Usage:** `hwp skill <COMMAND>`

_No arguments or flags_

## `hwp skill export`

Write the embedded skill tree (SKILL.md, SKILL.ko.md, the official-documents guide, references/ and templates/) into a directory (default ./hwp)

**Usage:** `hwp skill export [OPTIONS]`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `-o, --output` | `<OUTPUT>` |  | Output directory for the skill tree (mutually exclusive with --install) |
| `--install` | `claude-code` \| `codex` \| `amazon-quick` |  | Install into a known agent skills directory instead |
| `--quick-profile` | `<ID_OR_ABSOLUTE_PATH>` |  | Amazon Quick profile ID or absolute profile directory (Amazon Quick installs only) |

## `hwp dump`

[developer] Dump record and package structure

**Usage:** `hwp dump [OPTIONS] <FILE>`

| Argument/flag | Value | Default | Description |
|---|---|---|---|
| `<FILE>` |  |  | Target HWP/HWPX file |
| `--stream` | `<STREAM>` |  | Target stream or entry (for example "DocInfo", "BodyText/Section0", "Contents/header.xml") |
| `--raw` |  |  | Print record payloads as hex |
| `--json` |  |  | Print as JSON |
