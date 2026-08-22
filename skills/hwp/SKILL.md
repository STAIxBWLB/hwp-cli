---
name: hwp
description: Read, create, edit, convert, render and validate Hancom HWP 5.0 / HWPX documents with the hwp CLI or its MCP stdio server. Use whenever a task touches .hwp or .hwpx files — extracting text, searching, filling templates, editing content, converting to docx/pdf/html/md/json/odt/txt/csv, or rendering pages to images. Also use for Korean official documents (공문) — 기안문, 보고서, 계획서, 회의록, 공고문, 보도자료 — their markdown contract, templates and 표기법 (notation rules) are covered by the official-documents sub-guide.
---

[한국어](SKILL.ko.md) · [English](SKILL.md)

# hwp — HWP/HWPX document toolkit

`hwp` is a single-binary toolkit for Hancom HWP documents. It reads and writes the binary
HWP 5.0 format and the XML-based HWPX format, and exports to docx, pdf, html, markdown, json,
odt, txt and csv. No Hancom Office installation is required.

English is canonical; `SKILL.ko.md` is the full Korean mirror and both are always exported
together.

Install: `brew install staixbwlb/hwp/hwp`, or
`curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh`.
`hwp skill export [-o DIR] [--install claude-code|codex|amazon-quick]` materializes this
skill from the binary. Amazon Quick Desktop profiles can be selected with
`--quick-profile ID_OR_ABSOLUTE_PATH`.

## Command quick reference

Full flag reference: `hwp {command} --help` (generated docs: docs/manual/cli-reference.md).
Output formats are inferred from the output extension unless `--to`/`--format` is given.
In the usage lines below, single-brace tokens such as `{file}` and `{output}` are placeholders
to replace with your own values. Only the doubled form `{{name}}` is literal HWPX template-slot
syntax (used by `fill`, `slots` and the template tools).

- `hwp info {file} [--json]` — format, version, properties, stream list.
- `hwp cat {file} [--format plain|markdown|json|html|csv]` — extract text. Useful flags:
  `--preview` (PrvText only, no body parse), `--with-header-footer`, `--with-hidden`
  (hidden comments), `--with-segments` (markdown + source coordinates as a one-line JSON
  envelope). `--format json` exports the full IR (tables, images, formatting).
- `hwp grep {pattern} {file} [--ignore-case]` — paragraph substring search over body, table
  cells and text boxes. grep convention: exit code 1 when nothing matches.
- `hwp convert {inputs...} -o {output}` — format conversion (`--to hwp|hwpx|md|json|html|pdf|
  odt|txt|csv|docx`). `-` reads stdin / writes stdout (stdout for text formats only). Multiple
  inputs require `--out-dir {dir}`. `--strict` fails without publishing when unpreservable
  (opaque) data is found. PDF output delegates to the render path and needs CJK fonts:
  `--font-dir {dir}` (repeatable; default `HWP_FONT_DIR` or `fonts/`). Markdown export flags:
  `--media-dir`, `--with-header-footer`, `--with-hidden`, `--embed-bin` (json).
- `hwp new -o {out.hwpx|out.hwp}` — create a document. `--from {file.md|file.json}` imports
  markdown or a JSON IR (empty document when omitted); `--set-meta key=value` (title/author/
  subject/keywords, repeatable); `--preset official|report|plan|notice|minutes|press`
  (Korean official-document profiles, markdown input only); `gian` and other documented
  compatibility aliases normalize to a canonical profile. Per-side overrides are
  `--margin-top`, `--margin-bottom`, `--margin-left` and `--margin-right` in millimetres.
  `--strict` fails when markdown import drops content.
- `hwp edit {input} -o {output} [flags...]` — edit an existing document; images, formatting
  and unparsed records are preserved. String flags (all repeatable): `--replace "find=>repl"`,
  `--set-cell "t:r:c=value"` (0-based), `--set-field "name=value"`, `--set-meta "k=v"`,
  `--create-field "anchor=>name[=value]"`, `--create-bookmark "anchor=>name"`,
  `--create-hyperlink "anchor=>[text=>]URL"`, `--insert-image "anchor=>path[@WxH mm]"`,
  `--seal "anchor=>path[@size mm]"`, `--set-format "find:prop=value,..."`,
  `--set-align "find=left|right|center|justify|distribute"`. Structural flags (all
  repeatable): `--insert-para "anchor=>text"`, `--insert-para-before`, `--delete-para "text"`,
  `--add-row "t[:at[:count[:template_row]]]"` / `--add-col "t[:at[:count]]"` (`at` omitted or
  `end` appends; a number inserts before that row/column; merged tables supported) /
  `--delete-row "t:r"` / `--delete-col "t:c"` /
  `--merge-cells "t:r1:c1:r2:c2"` / `--split-cell "t:r:c"` / `--add-table "anchor=>[[row],...]"` /
  `--clone-table "src=>anchor[=>blank|keep]"` (deep-copy table `src` after the anchor —
  blank keeps structure/styles with empty cells, keep also clones nested tables/images
  with remapped IDs) /
  `--delete-table "n|anchor"` / `--delete-image "anchor"` / `--delete-field "name"` /
  `--delete-bookmark "name"`, paragraph shape `--set-para "find=>key:value"`
  (line-spacing, indent, left/right/top/bottom mm) and page setup `--set-page "key:value"`
  (width/height/margin-*/orientation). `--verify` re-reads the output; `--allow-partial`
  relaxes the all-or-nothing rule (see Safety rules).
- `hwp fill {template.hwpx} -o {output}` — fidelity-preserving `{{name}}` template fill
  (package preserved). `--set "name=value"` (repeatable); `--set "name=@part.md"` splices a
  part file (markdown + HTML table blocks) into the anchor paragraph — part-based composition
  for large documents (`@@` escapes a literal `@`); `--data {file.json}` bulk fill
  (`"parts": {...}` splicing, `"tables": [...]` row fill); `--json` prints the summary;
  `--allow-partial` publishes the matched subset. List slots first with `hwp slots`.
- `hwp compose {spec.json|yaml} -o {output}` — deterministic composition from DocumentSpec
  v1/v2. `--dry-run` validates without writing; `--report` prints the run report as JSON.
- `hwp template {template} --data {data} -o {output}` — typed native HWP/HWPX generation from
  TemplateSpec/Data v1. `--dry-run` runs expansion + writer + validation without publishing;
  `--report` prints the preservation/expansion report as JSON.
- `hwp render {input} -o {output.png|svg|pdf}` — render pages. `--pages "1"|"1-3"|"all"`,
  `--dpi 36..=600` (default 96), `--format png|svg|pdf`, `--font-dir {dir}` (repeatable).
  PNG/SVG write one file per page; PDF writes a single multi-page file. Needs CJK fonts.
- `hwp fields {file} [--json]` / `hwp bookmarks {file} [--json]` / `hwp slots {file} [--json]`
  — list fields (name/kind/value), bookmarks (bokm), `{{name}}` template slots.
- `hwp validate {file} [--json]` — structural validation (mimetype, required entries, XML
  parsing); exit code 0 when valid.
- `hwp certify {input} --policy {policy.json|yaml} --report {dir}` — certify package,
  semantics, native render and independent import under a versioned policy; publishes the
  report directory atomically.
- `hwp diff {input} --ref {hancom.png} [--page N] [--dpi N] [--tolerance N] [-o diff.png]` —
  compare a render against a Hancom reference PNG (offset, pixel difference).

## Official documents

`hwp` ships the Korean official-document (공문) surface natively. Six profiles are available,
one per document type: `official` (기안문), `report` (보고서), `plan` (계획서), `notice`
(공고문), `minutes` (회의록) and `press` (보도자료). The canonical official profile accepts
`gian` and `gongmun` as semantic compatibility aliases; aliases select the same profile, not a
raw-byte identity promise. Korean aliases are also accepted. Each type is authored as a markdown
skeleton, created with `hwp new --from ... --preset official` (or its matching profile), and
filled with `hwp fill`.

개조식 is a writing style, not a profile: it is the noun-form sentence ending used inside
보고서·계획서 and 내부결재 bodies. Choose the profile by document type and apply the style in the
body text — see `references/korean-official-format.md` §6 어투.

Every profile uses top/bottom/left/right margins of 20/10/20/20 mm before an explicit per-side
override. Body defaults are: official Malgun Gothic 12pt/160%; report and plan HCR Batang
15pt/160%; notice Malgun Gothic 15pt/160%; minutes HCR Batang 14pt/130%; and press HCR Batang
14pt/160%. Report and plan use 15 mm header/footer margins; notice and press use 10 mm; official
and minutes use 0 mm. Report, plan, notice and press include `- N -` page numbers; official and
minutes do not.

Item marks come from nested-list depth: ordered lists render as `1.` → `가.` → `1)` → `가)` →
`(1)` → `(가)` → `①` → `㉮`. All eight depths are supported; depth 9 or deeper is rejected
without publishing. HWPX emits the matching numbering directly. HWP5 uses the verified safe,
direct encoding path for this official ladder, including the evidenced post-`하` continuation.

The □ ○ ladder is literal: type `□ ` and `○ ` as paragraph-leading symbols and the engine
indents them — never substitute ASCII lookalikes. `- ` list bullets render as `-` at depth 1
and `·` below.

Heading numbers are literal: type `Ⅰ. 1.` (Ⅰ = U+2160 full-width; ASCII `I.` is forbidden)
in the heading text — headings carry no automatic numbering.

A single item is a plain paragraph: when a list would contain exactly one item, write it as
an unmarked paragraph instead.

Never hand-type marks on the numbered path: write plain nested lists and let the engine
assign marks; a hand-typed `가.` inside an ordered list double-numbers.

Per-document recipes, slot tables, the Korean alias table, fill/validate workflows and the
Hancom final check live in `official-documents.md` (exported next to this file); regulation
background lives in `references/korean-official-format.md`.

## MCP server

`hwp mcp` speaks synchronous JSON-RPC 2.0 over stdio (line-delimited; stdout carries the
protocol, logs go to stderr). Protocol version is negotiated at `initialize`: a client
`protocolVersion` of `2025-06-18`, `2025-03-26` or `2024-11-05` is echoed back, anything else
gets the latest supported version.

Always start it with at least one sandbox root:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--root", "/path/to/workspace", "--font-dir", "/path/to/fonts"]
    }
  }
}
```

`--root {dir}` (repeatable) restricts every file path the tools touch — inputs, outputs,
nested image/part paths, spec `base_dir`s, per-call `font_dir`s and the certify report
directory — to the given directories. Roots are canonicalized at startup; a missing or
unreadable root fails fast. Without any `--root` the server is unrestricted and prints a
one-line warning to stderr at startup — prefer `--root` whenever the client allows it.

Amazon Quick Desktop on Windows starts local MCP children at Low mandatory integrity
(`S-1-16-4096`). Use a dedicated child of the Windows `LocalLow` directory and
`C:\Windows\Fonts`, with separate JSON arguments:
`["mcp", "--font-dir", "C:\\Windows\\Fonts", "--root",
"C:\\Users\\YOUR_NAME\\AppData\\LocalLow\\hwp-quick-workspace"]`. Replace `YOUR_NAME`
with the actual account folder before configuring Quick or calling a tool; neither the argument
list nor MCP paths expand `%USERPROFILE%`. Create the root under `AppData\LocalLow` so it inherits
the Low mandatory label. Do not use ordinary `C:\TEMP` or `%LOCALAPPDATA%\Temp` as a write root:
discovery may work there while `hwp_new` fails to create its private staging directory with
`Access is denied (os error 5)`.

A folder added to Quick's local-folder permissions is available to its built-in read/search tools
but does not become writable by the Low-integrity MCP child. Keep all MCP inputs and outputs under
the configured `LocalLow` root, use Quick's file tools or Explorer to copy artifacts into and out
of it, and re-enable the connector if repeated startup failures caused Quick to auto-disable it.

When helping someone configure Quick, prefer JSON import and never put shell quote characters
inside an argument. Verify three layers in order: the exact absolute binary returns a version;
Quick reports 16 tools and stays enabled after refresh; then `hwp_new` followed by `hwp_validate`
succeeds on an absolute path under the configured LocalLow root (for example
`C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\quick-hwp-smoke.hwpx`). Do not claim
that discovery alone proves file access.
After a connector edit, refresh or start a new chat instead of reusing an old generated tool prefix.
The copy-paste operator and AI runbook is:
`https://github.com/STAIxBWLB/hwp-cli/blob/main/docs/manual/amazon-quick-desktop.md`.

Tools (16):

| Tool | Required arguments | Purpose |
|---|---|---|
| `hwp_info` | `path` | Format, version, properties and stream diagnostics |
| `hwp_read` | `path` | Extract text (`format`: plain/markdown/json/html/csv; `with_header_footer`, `with_hidden`, `with_segments`); UTF-8 byte pagination |
| `hwp_grep` | `path`, `pattern` | Paragraph substring search; `{matches, count, truncated}`; zero matches is a normal result |
| `hwp_list_fields` | `path` | List fields |
| `hwp_list_bookmarks` | `path` | List bookmarks (bokm) |
| `hwp_slots` | `path` | List `{{name}}` placeholders |
| `hwp_render` | `path` | Render pages (`format`: png/svg/pdf, `pages` range); single-page PNG returns base64, larger results write files via `output_path` |
| `hwp_edit` | `input`, `output` | Strict atomic editing through typed JSON operations (mirrors every `hwp edit` flag, incl. `add_table`, `clone_table`, `set_para`, `set_page`, `delete_*`) |
| `hwp_convert` | `input`, `output` | Format conversion (`strict` defaults to true over MCP) |
| `hwp_new` | `output` | Create a document from markdown or JSON IR plus metadata, official profile and `margin_top`/`margin_bottom`/`margin_left`/`margin_right` overrides |
| `hwp_compose` | `output`, `spec`/`spec_path` | Compose DocumentSpec v1/v2 through the same path as the CLI |
| `hwp_template` | `output`, `template` (+`data`) | Bounded expansion of TemplateSpec/Data v1 |
| `hwp_fill` | `input`, `output`, `values` | Fill `{{name}}` in an hwpx template (package preserved) |
| `hwp_validate` | `path` | Structural validation, `{valid, errors, warnings}` |
| `hwp_certify` | `input`, `policy`, `report` | Run certification and publish the report atomically |
| `hwp_diff` | `input`, `ref` | Render one page and compare it to a reference PNG |

## Safety rules (must follow)

1. **Validate after every write.** After `new` / `edit` / `fill` / `compose` / `template` /
   `convert` to hwp/hwpx (or the MCP equivalents), run `hwp validate {output}` (or
   `hwp_validate`) and check exit code 0 before handing the file on.
2. **Mutations are fail-closed.** Authoring commands (`new`, `edit`, `fill`) treat `DROP:`
   warnings — content that cannot be preserved in the output — as hard failures and do not
   publish. A failed command leaves any existing output file untouched.
3. **Atomic publish.** Outputs are written to a private staging workspace next to the
   destination, verified, then swapped in. There is no partial or half-written output state;
   on failure the previous file is preserved (and restored on rollback).
4. **All-or-nothing by default.** `edit` and `fill` fail the whole command when any requested
   change found no target (unapplied edit / unreplaced placeholder). `--allow-partial`
   (or the MCP `allow_partial` argument) publishes only the matched subset — use it
   deliberately, and re-check the result.
5. **Prefer `--root` for MCP.** Scope the server to the working directory (repeat the flag
   for every directory the tools may legitimately touch) instead of running unrestricted.
