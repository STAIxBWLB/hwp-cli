[한국어](development.ko.md) · [English](development.md)

# Development

The crate layout and how to build and test, for contributors. The rules that gate a pull request are in [AGENTS.md](../../AGENTS.md).

## Find your way around the workspace

| Crate | Role |
|---|---|
| `hwp-model` | Shared document model (IR): the single contract every crate depends on, lossless preservation (opaque/tail), unit conversion |
| `hwp5` | HWP 5.0 binary reader/writer (CFB container, record streams, compression) |
| `hwpx` | HWPX reader/writer (ZIP package, OWPML XML) |
| `hwp-convert` | IR ↔ markdown / HTML / JSON, in-memory editing, field scanning |
| `hwp-render` | IR → PNG / SVG / PDF / JPEG / WebP renderer and layout geometry, line layout synthesis, shaping, font subsetting and embedding, render diff |
| `agent-hwp` | The `ahwp` binary (CLI and MCP server) |

An interactive map of these crates and the MCP deployment path is in
[docs/architecture/](../architecture/): open `hwp-cli-rendered.html` in a browser.

## Build and test

```sh
cargo build --all-targets
scripts/check.sh     # local CI mirror: fmt + clippy + test + structured corpus (required before a PR)
```

CI (`.github/workflows/ci.yml`) runs the platform-independent gates — `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and `scripts/check-structured-corpus.sh` —
once in an ubuntu `lint` job, and runs `cargo test --workspace` on **ubuntu + macOS + windows**
(all required). The ubuntu test job installs `fonts-nanum` (glyf TTFs; the CFF `fonts-noto-cjk`
made debug-build rendering ~100x slower). The local mirror `scripts/check.sh` runs the same gates.

Tests cover byte-identical hwp5 round-trips (identity/roundtrip/synth), semantically equivalent hwpx
round-trips, IR JSON and markdown round-trips, editing and field correction, render layout, tables and
diff metrics, and structured corpus determinism. Genuine fixtures are not in the repository; when they
are absent the corresponding tests skip rather than fail.

Two checklists sit outside the automated gates because they need Hancom Office on real hardware:
[docs/hancom-verification-checklist.md](../hancom-verification-checklist.md) for verifying written
files, and [docs/release-readiness.md](../release-readiness.md) for the pre-release gate.
