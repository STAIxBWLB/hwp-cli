[한국어](amazon-quick-desktop.ko.md) · [English](amazon-quick-desktop.md)

# Amazon Quick Desktop: HWP MCP setup and troubleshooting

This runbook is for a person or an AI assistant configuring `hwp` as a local MCP connector in
Amazon Quick Desktop. It covers the complete Windows path that was verified in practice: install
one current binary, install the HWP skill, register the connector, prove an actual file write, and
recover the connector when Quick disables or loses it.

Quick UI labels and internal file names can change between Desktop releases. Prefer the UI and the
import JSON in this guide over editing Quick's internal configuration files.

## What a working setup contains

| Component | Purpose | Known-good Windows value |
|---|---|---|
| `hwp.exe` | Runs the MCP stdio server | One current binary at a stable absolute path |
| HWP MCP connector | Exposes the 16 HWP tools | `hwp.exe mcp ...` |
| HWP skill | Tells the Quick agent when and how to use the tools | `skills/hwp/SKILL.md` in the active Quick profile |
| Exchange root | Shared file boundary between Quick and the MCP child | `C:\TEMP` |
| Font directory | Supplies Windows fonts for rendering | `C:\Windows\Fonts` |

The connector and the skill are separate. Installing the skill does not install the binary or
create the connector. A connector can also show 16 tools while later file writes fail, so a real
create-and-validate smoke test is required.

## 1. Install and verify one current `hwp.exe`

Download the Windows x86_64 archive and its `.sha256` file from the
[latest release](https://github.com/STAIxBWLB/hwp-cli/releases):

- `hwp-vX.Y.Z-x86_64-pc-windows-msvc.zip`
- `hwp-vX.Y.Z-x86_64-pc-windows-msvc.sha256`

Verify the archive before extracting it. Replace the example paths with the downloaded version:

```powershell
$Archive = "C:\path\to\hwp-vX.Y.Z-x86_64-pc-windows-msvc.zip"
$Expected = ((Get-Content -LiteralPath "$Archive.sha256") -split '\s+')[0].ToLowerInvariant()
$Actual = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($Actual -ne $Expected) { throw "hwp archive checksum mismatch" }
```

Extract `hwp.exe` to a stable location that Quick can execute and do not move it after registering
the connector. A layout verified with Quick on Windows is:

```text
%USERPROFILE%\.quickwork\profiles\PROFILE_ID\skills\hwp\bin\hwp.exe
```

`PROFILE_ID` is the active entry in `%USERPROFILE%\.quickwork\profiles.json`. Another stable,
Quick-accessible location is also valid. Record the exact absolute path and verify the binary in
PowerShell:

```powershell
$Hwp = "C:\absolute\path\to\hwp.exe"
& $Hwp --version
```

Use this same path in Quick. Do not rely on the PATH seen by a different terminal, because Quick
can otherwise start an older duplicate installation. If the first file write still fails with a
Windows verbatim path such as `\\?\C:\...`, upgrade to a release containing the Amazon Quick
Windows canonical-path normalization fix.

## 2. Create the Windows exchange root

Quick's built-in file tools and its local MCP child do not necessarily receive the same filesystem
permissions. A user-profile folder added under **Local folders and access permissions** can still
be rejected by the MCP child. Start with the verified sandbox exchange directory:

```powershell
New-Item -ItemType Directory -Path C:\TEMP -Force
```

Use `C:\TEMP` as the connector's `--root`. Copy input `.hwp`, `.hwpx`, Markdown, JSON, images, and
templates into that directory before calling an HWP tool. Keep every MCP input and output path
under that root, then use Quick's file tools or Explorer to copy the final artifact to its intended
destination.

`--root` is a security boundary as well as a compatibility setting. Do not remove it to work around
a permission error.

## 3. Install the HWP skill into the active Quick profile

Run the current binary:

```powershell
& $Hwp skill export --install amazon-quick
```

The command reads `%USERPROFILE%\.quickwork\profiles.json`, selects its valid `last_active` profile
or the only valid profile, and writes only `skills\hwp\SKILL.md` inside it. It does not copy
`hwp.exe`, register the MCP connector, create an agent, or publish anything.

If Quick has several profiles or automatic selection is ambiguous, pass the profile ID or absolute
profile directory:

```powershell
& $Hwp skill export --install amazon-quick --quick-profile enterprise-example
& $Hwp skill export --install amazon-quick --quick-profile "C:\absolute\path\to\quick\profile"
```

Restart or refresh Quick after installing or replacing the skill.

## 4. Register the local MCP connector

In Quick Desktop, open **Settings → Capabilities → Connectors → + Create → MCP server → Local**.
The labels can vary slightly by release. Importing JSON is recommended because it preserves the
argument boundaries exactly:

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
        "C:\\TEMP"
      ]
    }
  }
}
```

Replace only `command`. In JSON, doubled backslashes encode ordinary Windows backslashes; they do
not change the real path.

For manual entry, use:

| Field | Value |
|---|---|
| Name | `hwp` |
| Command | the exact absolute `hwp.exe` path verified above |
| Arguments | `mcp --font-dir C:\Windows\Fonts --root C:\TEMP` |
| Description | `Read, write, edit, render, validate, and convert HWP/HWPX documents.` |
| Timeout | `30` seconds |

The Arguments field is not a shell. Do not type single or double quote characters around paths.
For example, this is wrong:

```text
mcp --font-dir 'C:\Windows\Fonts' --root 'C:\TEMP'
```

Quick can pass those quote characters literally, causing `hwp` to look for a directory whose name
contains quotes. The root then fails fast and the MCP handshake closes. The JSON form avoids this
failure by keeping every token as a separate array item.

Select **Test connection**, approve Quick's command-execution confirmation, and expect
**Connected** and **16 tools available**. Then select **Add MCP**, approve the confirmation, refresh
connections, and verify that `hwp` is enabled and reports **16 tools, Connected**.

## 5. Run an end-to-end smoke test

Do not stop at “16 tools available.” Start a new Quick chat and paste this prompt:

```text
Use the HWP MCP tools, not a shell command.
1. Call hwp_new to create C:\TEMP\quick-hwp-smoke.hwpx from this Markdown:
   # Quick MCP smoke test

   Amazon Quick can create HWPX files through hwp MCP.
2. Call hwp_validate on C:\TEMP\quick-hwp-smoke.hwpx.
3. Call hwp_read on the same file in plain format.
4. Report the exact output path and the validation result. Do not claim success unless valid is true.
```

A working installation produces the file and returns validation equivalent to:

```json
{
  "valid": true,
  "errors": [],
  "warnings": []
}
```

If visual output matters, follow with `hwp_render` for page 1 and write its output under
`C:\TEMP`. This checks font access and rendering separately from document creation.

## 6. Give a Quick agent durable instructions

Use one HWP-focused agent rather than several agents with duplicate names or stale connector
instructions. Enable the HWP connector and installed HWP skill, then add instructions like these:

```text
Use the installed hwp skill and HWP MCP tools for every .hwp or .hwpx task.
On Windows, use only paths under C:\TEMP unless the active connector explicitly exposes another root.
Copy inputs into C:\TEMP before an HWP operation and return the final artifact path under C:\TEMP.
After hwp_new, hwp_edit, hwp_fill, hwp_convert, hwp_compose, or hwp_template, always call hwp_validate.
When page appearance matters, also call hwp_render and inspect the requested pages.
Never hard-code the generated MCP server prefix; select tools by their hwp_new/hwp_read/etc. names.
If access is denied, report the attempted path and configured root. Do not remove the root restriction.
Do not claim success from connector discovery alone; require the requested operation and validation to pass.
```

OneDrive or SharePoint connectors are optional. Use them only to copy source or completed files into
and out of `C:\TEMP`; they do not replace the local HWP MCP connector.

## Daily workflow

1. Copy every source file and referenced asset into `C:\TEMP`.
2. Give Quick the exact input and output paths. For example: “Read
   `C:\TEMP\input.hwpx`, replace Draft with Final, and write `C:\TEMP\final.hwpx`.”
3. Require `hwp_validate` after any write. Require `hwp_render` as well when layout matters.
4. Confirm the returned path and validation result, then open or inspect the artifact.
5. Copy the verified output from `C:\TEMP` to the approved destination. Clean up exchange files only
   after confirming the destination copy.

Useful prompts include:

- “Summarize `C:\TEMP\input.hwp` and list its tables.”
- “Convert `C:\TEMP\input.hwpx` to `C:\TEMP\input.md`.”
- “Create `C:\TEMP\report.hwpx` from this Markdown, validate it, and render page 1.”
- “Fill the slots in `C:\TEMP\template.hwpx`, write `C:\TEMP\filled.hwpx`, and validate it.”

## Troubleshooting by symptom

| Symptom | Likely cause | Recovery |
|---|---|---|
| `hwp.exe` does not start or `--version` fails | Wrong binary, wrong architecture, blocked or incomplete extraction | Re-download, verify SHA-256, extract the Windows x86_64 archive, and test the exact absolute command path |
| Test connection shows no tools or the server exits immediately | Missing/unreadable `--root`, quote characters in Arguments, typo, or stale command path | Ensure `C:\TEMP` exists; import the JSON above; remove shell quotes; verify `hwp.exe --version` |
| **Connected, 16 tools** but `hwp_new` returns `Access is denied (os error 5)` | Transport works, but the MCP child cannot use the requested filesystem path, or an older binary passes `\\?\...` to Quick's sandbox | Put both input and output under `C:\TEMP`; keep `--root C:\TEMP`; upgrade `hwp`; restart and run the smoke test |
| A path added to **Local folders and access permissions** still fails | That setting controls Quick's built-in file tools, not necessarily the local MCP child | Use the built-in tool to copy the file into `C:\TEMP`, then call HWP tools there |
| `os error 2` | The path does not exist; Desktop may have moved to OneDrive | Check the real path, create the intended directory, or stage the file under `C:\TEMP` |
| Connector becomes disabled after repeated failures | Quick auto-disabled a connector whose startup/handshake repeatedly failed | Correct the command/root, save, explicitly enable the connector, refresh, and restart Quick if needed |
| `Unknown tool` after editing or re-importing the connector | Quick generated a new internal connector/tool prefix while the conversation retained old tool names | Refresh connections and start a new chat; reload the HWP skill/tools instead of reusing the old generated name |
| `assetDescriptor contains prohibited HTML/script content` while publishing an agent | An old exported skill contains angle-bracket placeholders that Quick classifies as markup | Reinstall the skill from a current `hwp` binary, refresh Quick, and publish again |
| Creation works but rendering fails | Font directory is missing or inaccessible | First test without `--font-dir`; then add `C:\Windows\Fonts` and retry `hwp_render` |
| Path is outside allowed roots | The MCP root policy is correctly rejecting it | Copy the asset into `C:\TEMP` or add a genuinely supported root; do not disable all roots |

### Why the Windows path fix is necessary

Rust's Windows canonicalization can return an equivalent verbatim path such as
`\\?\C:\TEMP\quick-hwp-smoke.hwpx`. Quick can accept the ordinary `C:\TEMP` root during the MCP
handshake yet reject the verbatim spelling later when `hwp` creates its private atomic staging
directory. Current `hwp` normalizes verbatim drive and UNC paths back to ordinary Windows spelling
before downstream file I/O while preserving fail-closed root containment checks.

This distinction explains the misleading state where connector discovery succeeds but the first
write fails. Always verify the data plane with `hwp_new` plus `hwp_validate`.

## Optional local diagnostics

Use these only for diagnosis. Do not edit Quick's internal files while Quick is running.

- Active profile registry: `%USERPROFILE%\.quickwork\profiles.json`
- Per-profile MCP snapshot: `%USERPROFILE%\.quickwork\profiles\PROFILE_ID\mcp_config.json`
- Typical Windows backend log: `%LOCALAPPDATA%\Temp\quickwork-backend.log`

The stored connector key can be generated (for example, `hwp-...`) rather than exactly `hwp`, and
Quick can add internal `_quick` metadata including an `autoDisabled` flag. Treat those as Quick
implementation details. Repair and enable the connector in the UI instead of hand-editing them.

An optional log check is:

```powershell
Get-Content "$env:LOCALAPPDATA\Temp\quickwork-backend.log" -Tail 300 |
  Select-String "UserMCP|Loaded.*servers|total tools|hwp"
```

A healthy startup contains messages equivalent to “Started ... with 16 tools” and “Loaded 1/1
servers (0 failed), 16 total tools.” Log wording and location are not stable API contracts.

## Completion checklist

- The connector command is one verified absolute `hwp.exe` path.
- The connector uses separate JSON arguments and has no embedded shell quotes.
- `C:\TEMP` exists and is the Windows MCP root.
- The current HWP skill is installed in the active Quick profile.
- Test connection reports **Connected** and **16 tools available**.
- The connector stays enabled after refresh or restart.
- `hwp_new`, `hwp_validate`, and `hwp_read` succeed on `C:\TEMP\quick-hwp-smoke.hwpx`.
- `hwp_validate` returns `valid: true`; rendering is also tested when appearance matters.
- The agent validates after every write and returns the final artifact path.

Amazon Quick Web cannot run this local stdio workflow. See
[AI client integrations](ai-integrations.md#amazon-quick-web) for the current conversion/upload path
and the planned remote MCP architecture.
