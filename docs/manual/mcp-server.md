[한국어](mcp-server.ko.md) · [English](mcp-server.md)

# MCP server

What `ahwp mcp` exposes, how to configure a client and how an agent runs the read → edit → rewrite loop. Per-client setup is in [ai-integrations.md](ai-integrations.md); the conventions every client follows are in [agent-conventions.md](agent-conventions.md).

## Run the MCP server

`ahwp mcp` implements synchronous JSON-RPC 2.0 over stdio (line-delimited) using only `serde_json`,
with no tokio and no SDK. The protocol version is negotiated at `initialize`: a client
`protocolVersion` of `2025-06-18`, `2025-03-26` or `2024-11-05` is echoed back, anything else gets
the latest supported version. stdout carries the protocol; logs go to stderr.

Per-client setup (Claude Code/Desktop, Codex CLI/cloud, Kiro, Kimi, claude.ai skill upload,
and Amazon Quick Desktop) and the bundled agent skill (`ahwp skill export`):
[docs/manual/ai-integrations.md](ai-integrations.md).
The copy-paste Windows setup, create/validate acceptance test, reusable agent instructions, and
symptom-driven recovery are in the dedicated
[Amazon Quick Desktop runbook](amazon-quick-desktop.md).

Amazon Quick Desktop can launch this local stdio server and expose all 23 tools. Install the
publish-safe skill into its active profile with:

```sh
ahwp skill export --install amazon-quick
```

On Windows, create a dedicated exchange root under `%USERPROFILE%\AppData\LocalLow` (for example
`hwp-quick-workspace`) and pass its absolute path as the MCP `--root` (Quick arguments do not
expand environment variables). Quick starts the local MCP child at Low mandatory integrity, so
`C:\TEMP` can pass tool discovery but reject the first write. Quick's local-folder permissions do
not change that write integrity; use the dedicated runbook's creation, import JSON, and recovery
steps.

Amazon Quick Web cannot launch a local stdio process. `ahwp serve` now runs the same tools over
HTTP for container deployment (`POST /mcp`, see [deployment design](../design/22-remote-mcp-deployment.md)),
but it is a private hop that expects a trusted edge in front of it: authenticated Streamable HTTP,
tenant isolation and artifact transfer remain future work in
[Remote MCP transport](../design/20-remote-mcp.md).

### Exposed tools (23)

| Tool | Required arguments | Purpose |
|---|---|---|
| `hwp_info` | `path` | Format, version, properties and stream diagnostics |
| `hwp_read` | `path` | Extract body text (`plain`/`markdown`/`json`/`html`/`csv`; header/footer, hidden and segment options). UTF-8 byte pagination (256 KiB default, 1 MiB max) |
| `hwp_grep` | `path`, `pattern` | Paragraph text search; returns `{matches, count, truncated}` — zero matches is a normal result |
| `hwp_list_fields` | `path` | List fields |
| `hwp_list_bookmarks` | `path` | List bookmarks (bokm) |
| `hwp_styles` | `path` | List named styles with resolved attributes and `used_by` segment ids, as an `hwp-styles-v1` listing |
| `hwp_slots` | `path` | List `{{name}}` placeholders |
| `hwp_render` | `path` | Render pages to PNG (base64, response up to 16 MiB) or write PNG/SVG/PDF files via `output_path` (dpi 36..=600) |
| `hwp_edit` | `input`, `output` | Strict atomic editing through typed JSON operations |
| `hwp_convert` | `input`, `output` | Format conversion (`strict` defaults to true over MCP) |
| `hwp_new` | `output` | Create a document from markdown or JSON IR plus metadata |
| `hwp_compose` | `output`, `spec`/`spec_path` | Compose DocumentSpec v1/v2 through the same path as the CLI |
| `hwp_template` | `output`, `template` (+`data`) | Bounded expansion of TemplateSpec/Data v1 |
| `hwp_certify` | `input`, `policy`, `report` | Run certification and publish the report atomically |
| `hwp_fill` | `input`, `output`, `values` | Fill `{{name}}` in an hwpx template (package preserved) |
| `hwp_diff` | `input`, `ref` | Render one page and compare it to a reference PNG |
| `hwp_validate` | `path` | Structural validation, `{valid, errors, warnings}` |
| `hwp_lint` | `path` | Ten official-document notation and structure rules, as an `hwp-lint-report-v1` report |
| `hwp_merge` | `inputs`, `output` | Combine two or more documents; returns the preservation ledger |
| `hwp_split` | `input`, `out_dir` | Split per Section (or per `pages` range), or extract one typed range with `from`/`to`; returns the published fragment paths and, for a range, the `hwp-range-extraction-v1` report |
| `hwp_compare` | `a`, `b` | Read-only paragraph/structure diff as `hwp-compare-report-v1`. Differences never set `isError` — read `identical` |
| `hwp_put_file` | `name`, `content` | Write base64 content into the session workspace. The only way to hand an existing document to a remote deployment: tool arguments take paths, so upload first and pass the name. 512 KiB decoded |
| `hwp_get_file` | `path` | Return a workspace file as base64 (a receipt block plus an embedded resource). Refuses above 512 KiB rather than truncating. Keep intermediates in the workspace and fetch only the final artifact - the base64 lands in the client's message stream |

### Client configuration example

```json
{
  "mcpServers": {
    "hwp": {
      "command": "ahwp",
      "args": ["mcp", "--font-dir", "<repo>/fonts", "--root", "<repo>"]
    }
  }
}
```

`--root` (repeatable) sandboxes every file path the tools touch — inputs, outputs, nested
image/part paths, spec `base_dir`s and per-call `font_dir`s — under the given directories. The
roots also bind compose/template spec internals: image/visual assets and `reference_hwpx` packages
referenced by a spec must resolve under an allowed root. Without
any `--root` the server keeps the old unrestricted behavior and prints a one-line warning to stderr
at startup.

### The read → edit → rewrite loop

1. **Read** `hwp_read` (`format=json`) exports the whole document as IR, including tables, image
   references, formatting and unparsed records.
2. **Edit** `hwp_edit` applies replacements, cell values and field filling. Because only the IR
   changes, images, formatting and opaque records survive; only the line layout of edited paragraphs
   is invalidated and re-synthesized by the writer.
3. **Verify** `hwp_render` returns the resulting page as PNG so the agent can check the change
   visually, and `hwp_diff` compares it numerically against a Hancom reference render.

An edited hwp goes through the writer's synthesis path, which re-establishes Hancom's paragraph
invariants (line layout, the `0x0d` paragraph terminator, nchars and so on), so it opens correctly in
Hancom Office.
