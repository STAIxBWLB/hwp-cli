[한국어](commands.ko.md) · [English](commands.md)

# Commands at a glance

A one-screen summary of every `ahwp` command, for a person choosing a command or an agent mapping a task to one. The full flag-level reference is the generated [cli-reference.md](cli-reference.md).

## Look up a command

The full generated reference is [docs/manual/cli-reference.md](cli-reference.md)
(Korean: [cli-reference.ko.md](cli-reference.ko.md)). Both languages are generated from the
clap definitions and a CI test enforces that they stay in sync with the code.

Help is shown in English by default and in Korean under a Korean locale; override it with
`--lang en|ko` or `HWP_LANG`. A summary:

| Command | Description |
|---|---|
| `info <file>` | Format, version, properties and stream diagnostics |
| `cat <file>` | Extract body text (plain/markdown/json/html/csv). `--with-segments` adds provenance coordinates |
| `grep <pattern> <file>` | Search paragraph text with grep semantics (non-zero exit when nothing matches) |
| `convert <input> -o <output>` | Format conversion; `--to` also targets `docx`, `odt`, `csv` and `txt`. A `.pdf` output delegates to the render path. `--strict` fails without publishing when unpreservable data is found |
| `render <input> -o <output>` | Render pages to PNG/SVG (one file per page) or PDF (single multi-page) |
| `merge <inputs...> -o <output>` | Combine two or more documents, one Section per input in argument order. `--strict` and `--loss-report` as in `convert` |
| `split <input> --out-dir <dir>` | One fragment per Section by default; `--pages` splits on page ranges estimated from the layout cache Hancom saved |
| `new -o <output>` | Create a document from markdown, from JSON IR, or from one of the eight embedded official-document templates (`--template`, `--list-templates`) |
| `compose <spec> -o <output>` | Compose DocumentSpec v1/v2 deterministically. `--dry-run` validates without writing |
| `template <template> --data <data> -o <output>` | Bounded expansion of the TemplateSpec/Data v1 typed AST |
| `edit <input> -o <output>` | Text, formatting and structural editing (paragraphs, table rows/columns, cell merge/split, fields, images, seals) |
| `fields` / `bookmarks` / `slots` `<file>` | List fields, bookmarks, or `{{name}}` slots |
| `styles <file>` | List named styles with resolved attributes and `used_by` paragraph segment ids (`--format json` emits `hwp-styles-v1`) |
| `fill <input> -o <output>` | Fidelity-preserving template filling (hwpx package preserved) |
| `validate <file>` | Structural validation (mimetype, required entries, XML parsing) |
| `lint <file>` | Ten official-document notation and structure rules. Advisory by default; `--strict` exits 1 on an error-severity finding and `--json` emits `hwp-lint-report-v1` |
| `certify <input> --policy <file> --report <dir>` | Certify and publish a report atomically. See [Certification v1](../design/16-certification-v1.md) |
| `corpus --manifest <file> --report <dir>` | Frozen structured corpus gate. See [the corpus contract](../design/17-structured-corpus-v1.md) |
| `diff <input> --ref <png>` | Compare a render against a Hancom reference PNG (ink, offset, pixel difference, MAE) |
| `compare <a> <b>` | Paragraph and structural differences between two documents, leaving both untouched. Exit codes follow diff(1): 0 identical, 1 differences found, 2 the run failed |
| `mcp` | Run the MCP stdio server |
| `serve` | Serve the same MCP tools over HTTP (`POST /mcp`) for container deployment. See [Remote MCP deployment](../design/22-remote-mcp-deployment.md) |
| `skill export` | Export or install the bundled agent skill, including Amazon Quick profile discovery |
| `update` | Self-update (a brew installation delegates to `brew upgrade`) |
| `dump <file>` | [developer] Dump record and package structure |

Output format is usually inferred from the output file extension; `convert`/`render` also accept
`--to`/`--format`.

## Run common commands

```sh
# Diagnostics: format, version, properties, streams
ahwp info report.hwp

# Extract body text
ahwp cat report.hwp                       # plain text
ahwp cat report.hwp --format markdown     # markdown
ahwp cat report.hwp --format json         # full IR (JSON)
ahwp cat report.hwp --format csv          # tables as CSV

# Search body text (grep semantics: exit 1 when nothing matches)
ahwp grep "예산" report.hwp

# Convert (format inferred from the output extension)
ahwp convert report.hwp   -o report.hwpx  # hwp → hwpx (tables, images, headers preserved)
ahwp convert report.hwpx  -o report.hwp   # hwpx → hwp binary
ahwp convert report.hwp   -o report.md    # images extracted to report.media/
ahwp convert report.hwp   -o doc.json --embed-bin   # self-contained JSON with embedded images

# Render
ahwp render report.hwp -o page.png --dpi 150
ahwp render report.hwp -o report.pdf --font-dir ./fonts   # single searchable multi-page PDF

# Create a new document
ahwp new -o out.hwpx --from notes.md
ahwp new -o out.hwp  --from doc.json

# Structured authoring
ahwp compose spec.yaml -o report.hwpx --report
ahwp template report-template.yaml --data report-data.json -o report.hwpx

# Official documents (공문서)
ahwp new --list-templates                              # the eight embedded templates
ahwp new -o gian.hwpx --template gian-internal         # 기안문, 두문/결문 frames included
ahwp slots gian.hwpx                                   # the {{slots}} left to fill
ahwp fill gian.hwpx -o final.hwpx --set "제목=예산 집행 계획"
ahwp lint final.hwpx --strict                          # 표기법 and structure rules

# Edit (images, formatting and unparsed records preserved)
ahwp fields form.hwp                        # list fillable fields
ahwp edit form.hwp -o filled.hwp \
    --replace "초안=>최종" \
    --set-cell "0:1:2=12,300원" \
    --set-field "수신처=홍길동" --verify

# Structural editing (insert/delete paragraphs, table rows/columns, cell merge/split)
ahwp edit report.hwp -o out.hwp \
    --insert-para "개요=>추가 설명 문단입니다." \
    --add-row "0" --add-col "1" --merge-cells "0:1:1:2:2" --verify

# Render fidelity comparison against a Hancom reference PNG
ahwp diff report.hwp --ref hancom_p1.png --page 1 --dpi 150 --font-dir ./fonts

# MCP stdio server
ahwp mcp --font-dir ./fonts
```
