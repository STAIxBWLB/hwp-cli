[한국어](ai-integrations.ko.md) · [English](ai-integrations.md)

# AI client integrations

`hwp` ships two integration surfaces for AI clients:

- an **MCP stdio server** (`hwp mcp`, 16 tools) for clients that speak the Model Context
  Protocol, and
- an **agent skill** (`skills/hwp/SKILL.md` in this repo) that teaches an agent the CLI and
  MCP usage. It is embedded in the binary, and `hwp skill export` materializes it.

Whichever surface you use, prefer `hwp mcp --root {dir}` so every file path the tools touch
stays under the given directories, and run `hwp validate` on any file the agent writes.

## Claude Code / Claude Desktop

Register the MCP server (Claude Code: `.mcp.json` or `claude mcp add`; Claude Desktop:
`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--root", "/path/to/workspace"]
    }
  }
}
```

Repeat `--root` for every directory the tools may touch. Without any `--root` the server is
unrestricted and prints a one-line warning to stderr at startup.

Claude Code additionally consumes the agent skill directly:

```sh
hwp skill export --install claude-code   # writes ~/.claude/skills/hwp/SKILL.md
```

## Codex CLI

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.hwp]
command = "hwp"
args = ["mcp", "--root", "/path/to/workspace"]
```

And install the agent skill:

```sh
hwp skill export --install codex         # writes ~/.codex/skills/hwp/SKILL.md
```

## Codex cloud

Codex cloud environments build their container from a setup script. Install the binary there
(no Rust toolchain needed; it fetches the pre-built release archive):

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
```

Then register the MCP server as in Codex CLI above.

## Kiro / Kimi

Both accept a standard stdio MCP server registration, with the same shape as Claude Code:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--root", "/path/to/workspace"]
    }
  }
}
```

Put it in the client's MCP config (Kiro: `.kiro/settings/mcp.json`; Kimi: the MCP section of
its config). The skill directory conventions differ per client, so export it anywhere and point
the client at it with `hwp skill export -o {dir}`.

## claude.ai (web)

The claude.ai code-execution sandbox has a registry-restricted network, so the binary cannot
be downloaded at runtime. Every release therefore attaches `hwp-skill-claude-web.zip`, which
contains `SKILL.md` at the zip root, `bootstrap.sh`, and the Linux x86_64 `bin/hwp`:

1. Download `hwp-skill-claude-web.zip` from the
   [latest release](https://github.com/STAIxBWLB/hwp-cli/releases).
2. In claude.ai, open Settings → Capabilities → Skills and upload the zip.
3. In a chat with code execution, run `bash bootstrap.sh` once per session. It installs the
   bundled binary into `~/.local/bin` and prints the MCP registration snippet. Claude then
   drives `hwp` as a CLI inside the sandbox.

## Amazon Quick Desktop

Amazon Quick Desktop can launch `hwp mcp` as a local stdio connector. The connector has been
verified with all 16 HWP tools available. Quick UI labels may change between releases; the
following names match the current Desktop flow.

For a Windows-first, copy-paste procedure that includes binary verification, agent instructions,
an actual create/validate smoke test, and symptom-based recovery, use the dedicated
[Amazon Quick Desktop runbook](amazon-quick-desktop.md). The shorter reference below is retained for
cross-client comparison.

### 1. Verify one current binary

Use an absolute executable path in the connector. This prevents a stale installation earlier in
Quick's PATH from being selected.

```sh
command -v hwp
hwp --version
# zsh/bash: show duplicate installations, if any
type -a hwp
```

On Apple Silicon Homebrew commonly resolves to `/opt/homebrew/bin/hwp`. Treat that as an example,
not a portable constant. If `~/.cargo/bin/hwp` is an older duplicate and Homebrew is the intended
installation, remove the Cargo registration with `cargo uninstall hwp-cli`; then rerun the checks.

### 2. Add the local MCP connector

Open **Settings → Capabilities → Connectors → + Create → MCP server → Local** and enter:

| Field | Value |
|---|---|
| Name | `hwp` |
| Command | the absolute path from `command -v hwp` |
| Arguments (macOS example) | `mcp --font-dir /System/Library/Fonts --root /path/to/workspace` |
| Description | `Read, write, edit, render, validate, and convert HWP/HWPX documents.` |
| Timeout | `30` seconds (the default is normally sufficient) |

`/System/Library/Fonts` is a macOS CJK font source. Replace it on other systems or omit
`--font-dir` when rendering is not needed. Repeat `--root /another/authorized/directory` for every
location the tools may legitimately access. Do not omit all roots unless unrestricted filesystem
access is intentional.

Select **Test connection**, review Quick's command-execution confirmation, and approve **Add
server**. The test should report **Connected** and **16 tools available**. Select **Add MCP**, approve
the confirmation again, refresh connections, and verify that `Hwp` is enabled and shows **16 tools,
Connected**.

Equivalent import JSON:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "/absolute/path/to/hwp",
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
      "command": "C:\\absolute\\path\\to\\hwp.exe",
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
write integrity of a local MCP child. Quick starts `hwp.exe` at Low mandatory integrity
(`S-1-16-4096`), while `C:\TEMP` and `%LOCALAPPDATA%\Temp` are normally Medium. That mismatch allows
connector discovery but rejects the first atomic output staging directory with
`Access is denied (os error 5)`. A child of `AppData\LocalLow` inherits the required Low label
without broad ACL changes. Move or copy inputs into that dedicated root, keep MCP inputs and
outputs under it, and copy validated artifacts to the approved destination afterward.

After changing an auto-disabled connector, explicitly enable it again. A successful recovery
reports **Connected** and **16 tools available** and remains enabled after refresh.

### 3. Install the publish-safe HWP skill

```sh
hwp skill export --install amazon-quick
```

The command reads `~/.quickwork/profiles.json`, prefers its valid `last_active` profile, and falls
back to the only valid profile. It writes only `skills/hwp/SKILL.md` inside that profile. It does
not create agents, connectors, or publish anything.

For multiple or ambiguous profiles, provide a profile ID or an absolute profile directory:

```sh
hwp skill export --install amazon-quick --quick-profile enterprise-example
hwp skill export --install amazon-quick --quick-profile /absolute/path/to/quick/profile
```

Restart or refresh Quick if it was already running when the skill was installed.

### 4. Use the tools

Examples for a normal Quick chat or an HWP-focused agent:

- "Summarize this HWP file and list its tables."
- "Convert this HWPX document to Markdown."
- "Create an HWPX report from this Markdown and validate the result."
- "Replace 'Draft' with 'Final', set table 1 row 2 column 3, and render page 1."
- "List the template slots, fill name and date, then validate the output."

After every write, the agent should call `hwp_validate`. Use `hwp_render` when the visual result
matters.

### 5. Configure one HWP-focused agent

Keep a single agent for the role to avoid duplicate names and stale instructions. Enable the HWP
MCP connector and instruct the agent to use the installed `hwp` skill, validate after every write,
and respect the configured roots. OneDrive or SharePoint connectors are optional and are needed
only when the source or destination is there.

If publishing fails with `assetDescriptor contains prohibited HTML/script content`, reinstall the
skill from a current `hwp` binary. Current skill exports use brace placeholders such as `{file}` and
contain no angle-bracket markup that Quick can misclassify as HTML.

### Desktop acceptance checklist

- `hwp --version` reports the intended current binary.
- Connector test reports **Connected** and **16 tools available**.
- After refresh, the connector remains enabled and reports **16 tools, Connected**.
- `hwp_new`, `hwp_read`, `hwp_validate`, and `hwp_render` succeed on a test HWPX document (under
  the configured LocalLow root, e.g. `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace`, on Windows).
- Exactly one HWP-focused agent exists, and it publishes without the prohibited HTML/script error.

## Amazon Quick Web

Quick Web runs in the cloud and cannot launch the local stdio process or access Desktop's local
filesystem. Today, convert the document to a format Quick can read, then upload the result:

```bash
hwp convert input.hwp -o output.docx   # or: -o output.pdf
```

Download edited results and convert them back with `hwp convert` as needed. A Desktop/Outpost
execution path can substitute when available. Do not expose a local `hwp mcp` process directly to
the network.

Native Web integration requires an authenticated Streamable HTTP MCP service, tenant-isolated
storage, and content/artifact transfer instead of client-local path arguments. It is not implemented
in this release. The implementation contract is documented in
[Remote MCP transport](../design/20-remote-mcp.md) and tracked in
[issue #52](https://github.com/STAIxBWLB/hwp-cli/issues/52).

## Upstream skill vs downstream `hwpx` skill

This repo ships the **generic** skill at [`skills/hwp/SKILL.md`](../../skills/hwp/SKILL.md):
binary quick reference, MCP usage and safety rules. It is English-only by design (agents consume
it, and one canonical language avoids bilingual double-maintenance).

The Korean official-document (공문서) skill `skills/hwpx` in the separate `STAIxBWLB/skills`
repository is **downstream**. It wraps this generic skill with workspace-specific templates
(기안문/보고서 presets and document conventions). It is intentionally not merged here: this repo
stays the format/toolkit layer, and downstream layers carry site-specific document policy.
