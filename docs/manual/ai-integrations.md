[한국어](ai-integrations.ko.md) · [English](ai-integrations.md)

# AI client integrations

`ahwp` ships two integration surfaces for AI clients:

- an **MCP stdio server** (`ahwp mcp`, 23 tools) for clients that speak the Model Context
  Protocol, and
- an **agent skill** (the `skills/hwp/` tree in this repo) that teaches an agent the CLI and
  MCP usage. It is embedded in the binary, and `ahwp skill export` materializes it as a
  directory: `SKILL.md`, `SKILL.ko.md`, the official-document guide
  (`official-documents(.ko).md`), `references/` and `templates/`.

Whichever surface you use, prefer `ahwp mcp --root {dir}` so every file path the tools touch
stays under the given directories, and run `ahwp validate` on any file the agent writes.

The conventions every client follows (workflows, refusals, the preservation ledger, exit codes,
passwords and environment variables) are in [agent-conventions.md](agent-conventions.md). This page
covers per-client setup only.

## Claude Code / Claude Desktop

Register the MCP server (Claude Code: `.mcp.json` or `claude mcp add`; Claude Desktop:
`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "hwp": {
      "command": "ahwp",
      "args": ["mcp", "--root", "/path/to/workspace"]
    }
  }
}
```

Repeat `--root` for every directory the tools may touch. Without any `--root` the server is
unrestricted and prints a one-line warning to stderr at startup.

Claude Code additionally consumes the agent skill directly:

```sh
ahwp skill export --install claude-code   # installs the skill tree under ~/.claude/skills/hwp/
```

## Codex CLI

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.hwp]
command = "ahwp"
args = ["mcp", "--root", "/path/to/workspace"]
```

And install the agent skill:

```sh
ahwp skill export --install codex         # installs the skill tree under ~/.codex/skills/hwp/
```

## Codex cloud

Codex cloud environments build their container from a setup script. Install the binary there
(no Rust toolchain needed; it fetches the pre-built release archive):

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh | sh
```

Then register the MCP server as in Codex CLI above.

## Kiro / Kimi

Both accept a standard stdio MCP server registration, with the same shape as Claude Code:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "ahwp",
      "args": ["mcp", "--root", "/path/to/workspace"]
    }
  }
}
```

Put it in the client's MCP config (Kiro: `.kiro/settings/mcp.json`; Kimi: the MCP section of
its config). The skill directory conventions differ per client, so export the tree anywhere and
point the client at it with `ahwp skill export -o {dir}` (the command writes `SKILL.md`,
`SKILL.ko.md`, `official-documents(.ko).md`, `references/` and `templates/` under `{dir}`;
without `-o` it writes `./hwp`). The export refuses a symlinked destination, publishes the tree
as one directory replacement, and restores the previous tree if the publish fails, so installing
over an existing skill directory is safe.

## claude.ai (web)

The claude.ai code-execution sandbox has a registry-restricted network, so the binary cannot
be downloaded at runtime. Every release therefore attaches `hwp-skill-claude-web.zip`, which
contains `SKILL.md` at the zip root, `bootstrap.sh`, the Linux x86_64 `bin/ahwp`, and the
license files (`LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`):

1. Download `hwp-skill-claude-web.zip` from the
   [latest release](https://github.com/STAIxBWLB/agent-hwp/releases), along with the
   `hwp-skill-claude-web.zip.sha256` published beside it, and check the archive against it.
2. In claude.ai, open Settings → Capabilities → Skills and upload the zip.
3. In a chat with code execution, run `bash bootstrap.sh` once per session. It installs the
   bundled binary into `~/.local/bin` (override with `HWP_BIN_DIR`) as `ahwp`, runs
   `ahwp --version` as a smoke check, and prints the MCP registration snippet. Claude then drives
   `ahwp` as a CLI inside the sandbox. Only the CLI is available here, so
   `merge`, `split` and `compare` run as commands rather than as MCP tools.

## Amazon Quick Desktop

Amazon Quick Desktop can launch `ahwp mcp` as a local stdio connector. The connector has been
verified with all 20 HWP tools available. Quick UI labels may change between releases; the
following names match the current Desktop flow.

For a Windows-first, copy-paste procedure that includes binary verification, agent instructions,
an actual create/validate smoke test, and symptom-based recovery, use the dedicated
[Amazon Quick Desktop runbook](amazon-quick-desktop.md). The shorter reference below is retained for
cross-client comparison.

### 1. Verify one current binary

Use an absolute executable path in the connector. This prevents a stale installation earlier in
Quick's PATH from being selected.

```sh
command -v ahwp
ahwp --version
# zsh/bash: show duplicate installations, if any
type -a ahwp
```

On Apple Silicon Homebrew commonly resolves to `/opt/homebrew/bin/ahwp`. Treat that as an example,
not a portable constant. If `~/.cargo/bin/ahwp` is an older duplicate and Homebrew is the intended
installation, remove the Cargo registration with `cargo uninstall agent-hwp`; then rerun the checks.

### 2. Add the local MCP connector

Open **Settings → Capabilities → Connectors → + Create → MCP server → Local** and enter:

| Field | Value |
|---|---|
| Name | `hwp` |
| Command | the absolute path from `command -v ahwp` |
| Arguments (macOS example) | `mcp --font-dir /System/Library/Fonts --root /path/to/workspace` |
| Description | `Read, write, edit, render, validate, and convert HWP/HWPX documents.` |
| Timeout | `30` seconds (the default is normally sufficient) |

`/System/Library/Fonts` is a macOS CJK font source. Replace it on other systems or omit
`--font-dir` when rendering is not needed. Repeat `--root /another/authorized/directory` for every
location the tools may legitimately access. Do not omit all roots unless unrestricted filesystem
access is intentional.

Select **Test connection**, review Quick's command-execution confirmation, and approve **Add
server**. The test should report **Connected** and **23 tools available**. Select **Add MCP**, approve
the confirmation again, refresh connections, and verify that `Hwp` is enabled and shows **23 tools,
Connected**.

Equivalent import JSON:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "/absolute/path/to/ahwp",
      "args": [
        "mcp",
        "--font-dir",
        "/System/Library/Fonts",
        "--root",
        "/path/to/workspace"
      ]
    }
  }
}
```

#### Windows sandbox-compatible setup

On Windows, create a dedicated child of the system-provided Low-integrity directory before
registering the connector:

```powershell
$QuickHwpRoot = Join-Path $env:USERPROFILE 'AppData\LocalLow\hwp-quick-workspace'
New-Item -ItemType Directory -Path $QuickHwpRoot -Force | Out-Null
icacls.exe $QuickHwpRoot
```

The output must include an inherited Low mandatory label. Substitute the actual Windows account
folder for `YOUR_NAME` in the connector JSON; the argument list does not expand environment
variables:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "C:\\absolute\\path\\to\\ahwp.exe",
      "args": [
        "mcp",
        "--font-dir",
        "C:\\Windows\\Fonts",
        "--root",
        "C:\\Users\\YOUR_NAME\\AppData\\LocalLow\\hwp-quick-workspace"
      ]
    }
  }
}
```

Keep each argument as a separate JSON array item; do not add shell quotes around Windows paths.
Quick's **Local folders and access permissions** control its built-in read/search tools, not the
write integrity of a local MCP child. Quick starts `ahwp.exe` at Low mandatory integrity
(`S-1-16-4096`), while `C:\TEMP` and `%LOCALAPPDATA%\Temp` are normally Medium. That mismatch allows
connector discovery but rejects the first atomic output staging directory with
`Access is denied (os error 5)`. A child of `AppData\LocalLow` inherits the required Low label
without broad ACL changes. Move or copy inputs into that dedicated root, keep MCP inputs and
outputs under it, and copy validated artifacts to the approved destination afterward.

After changing an auto-disabled connector, explicitly enable it again. A successful recovery
reports **Connected** and **23 tools available** and remains enabled after refresh.

### 3. Install the publish-safe HWP skill

```sh
ahwp skill export --install amazon-quick
```

The command reads `~/.quickwork/profiles.json`, prefers its valid `last_active` profile, and falls
back to the only valid profile. It writes only `skills/hwp/SKILL.md` inside that profile — the
official-document files (`SKILL.ko.md`, `official-documents(.ko).md`, `references/`,
`templates/`) are **not** installed on the Quick path, and the command prints a note saying so.
It does not create agents, connectors, or publish anything.

For multiple or ambiguous profiles, provide a profile ID or an absolute profile directory:

```sh
ahwp skill export --install amazon-quick --quick-profile enterprise-example
ahwp skill export --install amazon-quick --quick-profile /absolute/path/to/quick/profile
```

Restart or refresh Quick if it was already running when the skill was installed.

### 4. Use the tools

Examples for a normal Quick chat or an HWP-focused agent:

- "Summarize this HWP file and list its tables."
- "Convert this HWPX document to Markdown."
- "Create an HWPX report from this Markdown and validate the result."
- "Replace 'Draft' with 'Final', set table 1 row 2 column 3, and render page 1."
- "List the template slots, fill name and date, then validate the output."
- "Merge these three HWPX files in this order and report what the merge could not preserve."
- "Split this document into one file per section and list the fragment paths."
- "Compare these two documents and tell me which paragraphs differ."
- "Lint this official document and list every error-severity finding."

After every write, the agent should call `hwp_validate`. Use `hwp_render` when the visual result
matters, `hwp_lint` before an official document is handed on, and read the `preservation` field of
every `hwp_merge` or `hwp_split` response rather than assuming a lossless run.

### 5. Configure one HWP-focused agent

Keep a single agent for the role to avoid duplicate names and stale instructions. Enable the HWP
MCP connector and instruct the agent to use the installed `hwp` skill, validate after every write,
and respect the configured roots. OneDrive or SharePoint connectors are optional and are needed
only when the source or destination is there.

If publishing fails with `assetDescriptor contains prohibited HTML/script content`, reinstall the
skill from a current `ahwp` binary. Current skill exports use brace placeholders such as `{file}` and
contain no angle-bracket markup that Quick can misclassify as HTML.

### Desktop acceptance checklist

- `ahwp --version` reports the intended current binary.
- Connector test reports **Connected** and **23 tools available**.
- After refresh, the connector remains enabled and reports **23 tools, Connected**.
- `hwp_new`, `hwp_read`, `hwp_validate`, and `hwp_render` succeed on a test HWPX document (under
  the configured LocalLow root, e.g. `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace`, on Windows).
- `hwp_merge`, `hwp_split` and `hwp_compare` succeed on two copies of that document under the same
  root, and `hwp_compare` reports `identical` rather than an error when the two differ.
- Exactly one HWP-focused agent exists, and it publishes without the prohibited HTML/script error.

## Amazon Quick Web

Quick Web runs in the cloud and cannot launch the local stdio process or access Desktop's local
filesystem. Today, convert the document to a format Quick can read, then upload the result:

```bash
ahwp convert input.hwp -o output.docx   # or: -o output.pdf
```

Download edited results and convert them back with `ahwp convert` as needed. A Desktop/Outpost
execution path can substitute when available. Do not expose a local `ahwp mcp` process directly to
the network.

Native Web integration requires an authenticated Streamable HTTP MCP service, tenant-isolated
storage, and content/artifact transfer instead of client-local path arguments. It is not implemented
in this release. The implementation contract is documented in
[Remote MCP transport](../design/20-remote-mcp.md) and tracked in
[issue #52](https://github.com/STAIxBWLB/agent-hwp/issues/52).

## The `hwpx` skill has been absorbed

This repo ships the **single bundled** skill under
[`skills/hwp/`](../../skills/hwp/SKILL.md): the binary quick reference, MCP usage, the exit-code
and preservation-ledger conventions and the safety rules, plus the Korean official-document
(공문서) surface — the markdown contract, per-document recipes, the regulation reference and the
markdown templates. Since v0.12.0 it also carries the native editing crosswalk
([`references/editing-recipes.md`](../../skills/hwp/references/editing-recipes.md)), and since
v0.13.0 three editing recipes verified against real documents: analyze, edit-section and guard.

The old user-scope skill `skills/hwpx` in the separate `STAIxBWLB/skills` repository has been
**absorbed** into this bundled skill and retired
([skills#35](https://github.com/STAIxBWLB/skills/issues/35), closed 2026-08-27); there is no
upstream/downstream split any more. Do not install or invoke the retired skill. Parity between
the old `./hwpx` subcommands and the native commands is recorded — verified, with dated evidence
rather than inferred — in the matrix in
[23-hwpx-skill-absorption](../design/23-hwpx-skill-absorption.md).
