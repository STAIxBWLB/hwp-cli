# AGENTS.md background

Human companion. Agent instruction source: [AGENTS.md](../AGENTS.md). Section headings match
AGENTS.md. This file holds rationale and history only, no directives.

## Language policy

- Code comments have been English by default since 2026-08. Existing Korean comments move to English
  as files are touched rather than in one sweep, so mixed files are expected for a while.

## CLI help localization

- `tests/cli_reference.rs` is the gate that stops a new command or flag from silently staying
  English when the display language is Korean.

## Build · test

- CI layout: fmt, clippy and the structured-corpus gate run once in an ubuntu `lint` job; the 3-OS
  `test` matrix runs only `cargo test --workspace`. Tag releases do not re-run the matrix:
  `release.yml` verifies the tagged commit's green CI through the check-runs API, which is why its
  job names have to match `ci.yml`.
- CI fonts: ubuntu uses fonts-nanum (glyf TTFs) because the CFF-based noto-cjk made debug-build
  rendering about 100x slower. Because CI glyphs depend on whatever the runner provides, CI-run
  tests cannot assert on font-dependent output.

## Verifying your work

- The `public-parity` field of the summary line reports the public parity gate; the script prints
  one word there, never both.
- CI and the release-readiness workflow hold no fixtures by the data policy, so
  `HWP_REQUIRE_FIXTURES=1` would always fail there. They only print the skip count.

## Branch · PR policy

- The repository used to work as a fork that sent PRs to an upstream. That setup is retired;
  `STAIxBWLB/hwp-cli` (origin) is the canonical repository.
- CI always runs the public PDF oracle gate on `ubuntu-24.04`, which carries the pinned
  `pdfinfo version 24.02.0`.

## Data policy (important)

- Provenance of the public PDF-parity oracle `public-safety-rfp-p1.pdf`: Mac Hancom HWP 12.30.0
  build 6446 on macOS 26.6.1 build 25G76, Quartz PDFContext, default Save as PDF, A4, one page.

## Design knowledge lives in docs/design/

- Touching the writer without knowing the rules in `07-hangul-compat-rules.md` produces files
  Hangul cannot open.

## Code navigation (ripwire)

- `ripwire` (redhat-et/ripwire) was adopted on 2026-09-19. Installation and PATH availability vary
  between development hosts, which is why AGENTS.md asks for a version check first.
- Every answer opens with a legend comment block (`<!-- ... -->`, 2 to 4 KB); the data rows follow.
- `#[test]` functions read as `dead-code` because they have no in-process caller.
- `--quality-delta` is not a CI gate because CI does not carry the binary.
  `.ripwire_quality_baseline` is pinned to one HEAD and gitignored.
- `--recall` resurfaces `.ripwire_notes` entries, which is why a gotcha can be pinned there before
  it reaches `docs/design/`.
- MCP mode stays off because its verb schemas would sit in every session's context.

## Invariants (do not break)

- `hwp-convert` and `hwp-render` stay apart because the segment id rule is duplicated in both
  (`crates/hwp-convert/src/segment_id.rs`, `crates/hwp-render/src/segment_id.rs`) instead of
  living in a shared crate. A normal dependency between them would make that duplication
  pointless. Dev-dependencies are out of scope: `hwp-render`'s tests already use `hwp-convert` to
  build documents without putting either crate in the other's build graph.
