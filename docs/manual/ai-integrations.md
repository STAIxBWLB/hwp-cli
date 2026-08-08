[한국어](ai-integrations.ko.md) · [English](ai-integrations.md)

# AI client integrations

`hwp` ships two integration surfaces for AI clients:

- an **MCP stdio server** (`hwp mcp`, 16 tools) for clients that speak the Model Context
  Protocol, and
- an **agent skill** (`skills/hwp/SKILL.md` in this repo) that teaches an agent the CLI and
  MCP usage. It is embedded in the binary — `hwp skill export` materializes it.

Whichever surface you use, prefer `hwp mcp --root <dir>` so every file path the tools touch
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
(no Rust toolchain needed — it fetches the pre-built release archive):

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
```

Then register the MCP server as in Codex CLI above.

## Kiro / Kimi

Both accept a standard stdio MCP server registration — same shape as Claude Code:

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
its config). The skill directory conventions differ per client — export it anywhere and point
the client at it with `hwp skill export -o <dir>`.

## claude.ai (web)

The claude.ai code-execution sandbox has a registry-restricted network, so the binary cannot
be downloaded at runtime. Every release therefore attaches `hwp-skill-claude-web.zip` —
`SKILL.md` (zip root), `bootstrap.sh`, and the Linux x86_64 `bin/hwp` bundled together:

1. Download `hwp-skill-claude-web.zip` from the
   [latest release](https://github.com/STAIxBWLB/hwp-cli/releases).
2. In claude.ai, open Settings → Capabilities → Skills and upload the zip.
3. In a chat with code execution, run `bash bootstrap.sh` once per session: it installs the
   bundled binary into `~/.local/bin` and prints the MCP registration snippet. Claude then
   drives `hwp` as a CLI inside the sandbox.

## Amazon Quick Suite

Quick Suite has no local MCP surface today. Convert the document first, then upload the
result:

```sh
hwp convert input.hwp -o output.docx   # or: -o output.pdf
```

A remote HTTP MCP endpoint (which Quick Suite could consume as an MCP-aware client) is
tracked separately in [#52](https://github.com/STAIxBWLB/hwp-cli/issues/52).

## Upstream skill vs downstream `hwpx` skill

This repo ships the **generic** skill at [`skills/hwp/SKILL.md`](../../skills/hwp/SKILL.md):
binary quick reference, MCP usage, safety rules. It is English-only by design (agents consume
it; one canonical language avoids bilingual double-maintenance).

The Korean official-document (공문서) skill `skills/hwpx` in the separate `STAIxBWLB/skills`
repository is **downstream**: it wraps this generic skill with workspace-specific templates
(기안문/보고서 presets, document conventions). It is intentionally not merged here — this repo
stays the format/toolkit layer, and downstream layers carry site-specific document policy.
