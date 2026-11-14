[한국어](README.ko.md) · [English](README.md)

# docs

The map of every documentation page in this repository: what each page answers and when to read it.
Agents should start here to find the one page that answers the current question. User-facing pages
are bilingual: English is canonical (`NAME.md`) and Korean is its pair (`NAME.ko.md`).

## Find the page you need

| Path | What it answers | Read it when |
|---|---|---|
| [../README.md](../README.md) | What `ahwp` is, how to install it, a quickstart | You are new to the project |
| [manual/capabilities.md](manual/capabilities.md) | The feature list, implementation status, limitations and roadmap | You need to know whether a capability exists before using it |
| [manual/install.md](manual/install.md) | Every install path, fonts, serverless and container builds, update and rollback | You install on a platform or build step the README does not cover |
| [manual/commands.md](manual/commands.md) | One-line summary of every command, with worked examples | You are choosing which command to run |
| [manual/cli-reference.md](manual/cli-reference.md) | Every flag of every command, generated from the clap definitions (do not edit by hand) | You need the exact flag or argument |
| [manual/markdown-export.md](manual/markdown-export.md) | How HWP maps to Markdown and what `--with-segments` returns | You extract text for search, RAG or provenance |
| [manual/authoring.md](manual/authoring.md) | DocumentSpec/TemplateSpec composition, official documents (공문서) and regeneration from JSON IR | You create a new document rather than edit one |
| [manual/certification.md](manual/certification.md) | What `certify` checks and how the structured corpus gate runs | You need evidence a written file meets a policy or is reproducible |
| [manual/mcp-server.md](manual/mcp-server.md) | The MCP tools, the client configuration and the read → edit → rewrite loop | You drive `ahwp` from an AI agent over MCP |
| [manual/agent-conventions.md](manual/agent-conventions.md) | Workflows, refusal codes, the preservation ledger, exit codes, passwords and environment variables | An agent edits, fills, binds or restyles documents, or handles a refusal |
| [manual/ai-integrations.md](manual/ai-integrations.md) | Per-client setup (Claude, Codex, Kiro, Kimi, claude.ai, Amazon Quick) and the agent skill | You register `ahwp` with a specific AI client |
| [manual/amazon-quick-desktop.md](manual/amazon-quick-desktop.md) | Amazon Quick Desktop setup, acceptance test, agent instructions and troubleshooting | You set up Amazon Quick Desktop |
| [manual/development.md](manual/development.md) | Crate layout, build and test commands | You contribute code |
| [design/00-overview.md](design/00-overview.md) | The index of the design record and its principles | You change the reader, writer or renderer |
| [design/07-hangul-compat-rules.md](design/07-hangul-compat-rules.md) | Hangul compatibility rules established on real hardware | Before touching the writer (required) |
| [design/10-hwp5-structure-map.md](design/10-hwp5-structure-map.md) | The HWP 5.0 record and control catalog | You work on HWP5 records |
| [design/11-hwpx-structure-map.md](design/11-hwpx-structure-map.md) | The OWPML element catalog | You work on HWPX elements |
| [design/12-feature-gaps.md](design/12-feature-gaps.md) | Unimplemented features with evidence and difficulty | Before assuming a feature is missing |
| [design/22-remote-mcp-deployment.md](design/22-remote-mcp-deployment.md) | The shared `ahwp serve` HTTP adapter and the Cloudflare and AgentCore tiers | You deploy the MCP server remotely |
| [design/23-hwpx-skill-absorption.md](design/23-hwpx-skill-absorption.md) | Parity record of the retired `hwpx` skill against the native commands | You migrate from the old `hwpx` skill |
| [../deploy/aws/README.md](../deploy/aws/README.md) | The AWS AgentCore deployment procedure | You deploy on AWS |
| [../deploy/cloudflare/README.md](../deploy/cloudflare/README.md) | The Cloudflare container deployment procedure | You deploy on Cloudflare |
| [release-readiness.md](release-readiness.md) | The pre-release gate checklist | You cut a release |
| [hancom-verification-checklist.md](hancom-verification-checklist.md) | How to verify written files in Hancom Office | You confirm a file opens in Hangul |
| [architecture/](architecture/) | Interactive architecture map of the workspace and the MCP deployment path: open `hwp-cli-rendered.html` in a browser. Generated from `hwp-cli.architecture.json` with Archify; the `visual-check` files are its browser evidence | You want a visual overview of the crates |
| [agents-background.md](agents-background.md) | The background behind the rules in AGENTS.md | You revise AGENTS.md |

Both `manual/cli-reference*.md` files are generated from the clap definitions in either language
(do not edit them by hand).

## About the HWP 5.0 specification material

Notes on the HWP 5.0 format specification material used when working on the parser and renderer.

### The specification is not bundled with this repository

The HWP 5.0 file format specification ("한글 문서 파일 형식 5.0 / HWP Document File Formats 5.0") is
copyrighted by Hancom Inc. Its open-document license permits free viewing, copying and distribution
but restricts distribution to the **unmodified original or copies thereof**. Derivatives such as
extracted text or page captures therefore fall outside what may be redistributed, so this repository
does **not** bundle the specification and links only to the official distribution point.

- **Official download**: <https://store.hancom.com/etc/hwpDownload.do>
  (HWP and PDF, `한글문서파일형식_5.0_revision1.3`)
- Code comments cite the specification by section number (for example
  `한글문서파일형식 5.0 §4.x`), which is fair use.

> You may keep the original specification files (`hwp5_spec.pdf`,
> `한글문서파일형식_5.0_revision1.3.hwp` and so on) under `docs/` locally; the root `.gitignore`
> keeps them untracked.

### Acknowledgment

This product was developed with reference to Hancom's HWP document file (.hwp) open specification.
