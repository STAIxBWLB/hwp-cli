# AGENTS.md

A Rust workspace that implements HWP 5.0 (binary) and HWPX (OWPML) **directly**, with no external
HWP library. Background for revising these rules: [docs/agents-background.md](docs/agents-background.md).

## Language policy

- **Everything an AI agent reads as development context is English only**: commit messages, PR
  titles/bodies, release notes (`CHANGELOG.md`, GitHub Release bodies), issue text, code comments,
  and internal working docs (`AGENTS.md`, `docs/agents-background.md`, `REVIEW.md`). Move existing
  Korean comments to English in files you touch.
- User-facing strings (CLI output, error messages) keep their existing Korean tone.
- User-facing documentation stays bilingual, but **English is canonical**: `NAME.md` (English) and
  `NAME.ko.md` (Korean). Both carry a **language link on the first line**:
  `[한국어](NAME.ko.md) · [English](NAME.md)`. Never edit one side alone: **update both in the same
  commit**.
- `docs/manual/cli-reference.md` (English) and `cli-reference.ko.md` (Korean) are generated from the
  clap definitions. Do not hand-edit them; regenerate with
  `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`.

## CLI help localization

- **English is canonical.** Write the doc comments (= clap help) in `crates/hwp-cli/src/cli.rs` in
  English.
- Korean lives in the `KO` overlay table in `crates/hwp-cli/src/i18n.rs`, applied at runtime.
- Display language precedence: `--lang <en|ko>` → `HWP_LANG` → `LC_ALL` → `LC_MESSAGES` → `LANG` →
  English.
- Adding a command or flag means adding it to the `KO` table too; `tests/cli_reference.rs` fails on
  missing or dead entries.

## Build · test

```bash
cargo build                    # debug build (bin: hwp)
HWP_FONT_DIR=$PWD/fonts python3 tools/diagnostic_corpus.py   # diagnostic corpus + self-verification harness
```

- Local runs **must use the same commands** as the CI gates (`.github/workflows/ci.yml`):
  `cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` →
  `cargo test --workspace`. For a partial run during development (clippy only, test only), call one
  of them directly; the full `scripts/check.sh` is what gates the PR.
- `release.yml` checks the tagged commit's CI by job name: rename a `ci.yml` job only together with
  `release.yml`.
- Rust edition 2024, rust-version 1.93.
- Fonts: **none are bundled**. `/fonts/` is gitignored and holds the HCR Batang/Dotum you download
  locally for the diagnostic corpus and golden comparison.
- CI render glyphs come from system fonts (ubuntu: fonts-nanum glyf TTFs, never a CFF font such as
  noto-cjk; macOS: default CJK), so tests that run in CI must not assert on font-dependent output
  (glyphs, page counts).
- `HWP_GOLDEN=1` - opt-in golden render comparison against Hangul reference PNGs. `HWP_CORPUS_DIR` -
  soak test over a large in-the-wild corpus.

## Verifying your work

```bash
scripts/check.sh               # the one gate: fmt -> clippy -> test -> fixture/doc/release gates
```

- A successful run ends with exactly one of these two lines (`<N>` and `<M>` are always numbers,
  `0` included):

  ```
  == check: OK (fmt/clippy/test/crate-edges/pdf-runner/structured-corpus/claims/doc-surface/release-block/readiness-selfcheck/skip-accounting/public-parity=ran) skipped-for-missing-fixtures=<N> (optional=<M>) ==
  == check: OK (fmt/clippy/test/crate-edges/pdf-runner/structured-corpus/claims/doc-surface/release-block/readiness-selfcheck/skip-accounting/public-parity=skipped) skipped-for-missing-fixtures=<N> (optional=<M>) ==
  ```

  Any other ending means the run failed; the script keeps going after a failing gate so one run reports all of them.
- **A green run with N > 0 did not check what the skipped tests check** (#275): tests that need a
  local-only fixture (`fixtures/hwp5/`, `fixtures/hwpx/`, `fixtures/pdf-parity/private/`) skip
  without it and still report `ok`, invariant 2's identity gate among them; `target/fixture-skips.log`
  lists them. `HWP_REQUIRE_FIXTURES=1 scripts/check.sh` turns every such skip into a failure, except
  the `<M>` optional ones (ground-truth sets `fixtures/README.md` lists as not currently held,
  guarded by `optional_fixture_missing`). CI and the release-readiness workflow never set it; the
  strict local run is a release-readiness checklist item. After a failed test step the count stops
  early and the `check: FAILED` line labels it `(partial)`.
- New fixture guards go through `crates/hwp-cli/tests/common/fixture_skip.rs` (`fixture_missing`),
  never a bare `exists()`.
- **Run it before reporting a task complete, and paste the output.**
- When a test fails, fix the code, not the test. Do not skip, delete, or weaken a gate to make a run pass.
- `public-parity=skipped` is expected unless the host has the pinned `pdfinfo version 24.02.0`;
  `HWP_PDF_PARITY=1` makes that gate required.

## Branch · PR policy

- Features, fixes, docs - **all work happens on a branch**: `feat/<topic>`, `fix/<topic>`,
  `docs/<topic>`. No direct pushes to main.
- The canonical repository is `STAIxBWLB/hwp-cli` (= origin). Push branches to origin and **open PRs
  against origin's main**.
- Open a PR only once `scripts/check.sh` passes. **Squash merge once CI is green (ubuntu + macOS +
  windows all required)**, keeping the `(#N)` suffix convention in the merge commit title.

## Data policy (important)

- `fixtures/hwp5/*.hwp` and `fixtures/hwpx/*.hwpx` are gitignored (local only). Without them tests
  skip rather than fail. Sources are listed in `fixtures/README.md`.
- `fixtures/samples/` **is committed as an exception** - only owner-authored documents with
  university names pseudonymized (anonymization recipe in `fixtures/README.md`). Never commit the
  originals.
- **Never commit the ground-truth corpus** (genuine Hangul files such as `~/Documents/hwp_samples`).
- **Never commit the Hancom specification or derivatives** (extracted text, page captures) - see
  `docs/README.md`. Cite the spec by section number only (e.g. `한글문서파일형식 5.0 §4.2.6`). The
  local `docs/spec.txt` (gitignored) is for reference while working.
- **Narrow PDF-parity exception:** the owner-authored, anonymized one-page source and exactly
  `fixtures/pdf-parity/public/oracle/public-safety-rfp-p1.pdf` may be committed for the public
  regression gate. It is a bounded local fixture, never a universal or Windows parity claim.
  Private or third-party Hancom artifacts, other oracle PDFs/PNGs, and private corpus documents
  remain forbidden.

## Design knowledge lives in docs/design/

- Start here: [docs/design/00-overview.md](docs/design/00-overview.md) (document index, design
  principles)
- **Required reading before touching the writer**:
  [07-hangul-compat-rules.md](docs/design/07-hangul-compat-rules.md) - the catalog of Hangul
  compatibility rules established only on real hardware.
- Full format maps: [10-hwp5-structure-map.md](docs/design/10-hwp5-structure-map.md) (record/control
  catalog), [11-hwpx-structure-map.md](docs/design/11-hwpx-structure-map.md) (OWPML element catalog)
- Check [12-feature-gaps.md](docs/design/12-feature-gaps.md) first for unimplemented features.

## Code navigation (ripwire)

`ripwire` is optional. Check `command -v ripwire` and `ripwire --version` in the current session
before using it. If it is missing or cannot run, report that once and continue with `rg`,
`rg --files`, and source reads; it never blocks development, tests, or review, and is not a reason
to install host tooling in a repository fix. When it runs, query it before reading files whole or
sweeping with `rg`. CLI only: never start `--mcp`. Optional extra context for workspace sessions:
the workspace `dev/AGENTS.md`.

```bash
ripwire . --for="<the change you are about to make, in words>"   # entry points, with a confidence attr
ripwire . --callers=SYM      # who calls it; --uses=SYM lists call sites as file:line
ripwire . --impact=SYM       # transitive blast radius, tested/untested split
ripwire . --situ             # after editing: changed symbols, blast radius, tests to run
ripwire . --edit-check=SYM   # did the edit change a contract (arity, public surface)
ripwire . --quality-delta    # the "am I done" checkpoint: what got WORSE vs HEAD (exit 2 = a
                             # pre-existing symbol regressed materially; new-symbol rows are advisory)
ripwire . --recall="<topic>" # the docs/design and .ripwire_notes rows that answer it
```

- Counts are floors (name-based static extraction; ambiguous calls are listed as `declined`): a zero
  means "none found", never "none exists".
- Known floors on this repo: `#[test]` functions read as `dead-code` in `--quality-delta`, and
  CLI-level tests that run the built binary are invisible to `tested=`.
- `--quality-delta` is advisory, not a gate; `scripts/check.sh` stays the only gate. Do not add
  `.ripwire_quality_baseline` to a commit.
- `.ripwire_notes` is committed: pin a hardware-verified gotcha with
  `ripwire . --note-add="<path or symbol>: <text>"` when it is not yet in `docs/design/`. Keep the
  formal record in `docs/design/07-hangul-compat-rules.md`.

## Invariants (do not break)

1. **hwp-model depends on no other internal crate** (hub and spoke). `hwp5` and `hwpx` do not depend
   on each other either; they go through the IR. `hwp-convert` and `hwp-render` do not depend on
   each other. These are normal-dependency edges, direct or transitive; dev-dependencies are
   allowed (hwp5's tests use hwpx, hwp-render's use hwp-convert). `scripts/check-crate-edges.sh`
   checks them.
2. **Lossless round-trip gate**: hwp5 → hwp5 identity re-serialization must be byte-identical
   (`crates/hwp5/tests/identity.rs`). Do not drop unknown records; preserve them as `OpaqueRecord`.
3. **Ground-truth methodology - no guessing**: format behavior is established only by comparing
   against the bytes of genuine files saved by Hangul. The final verdict is whether Hangul
   (Hancom Office) opens the file.
4. No new external HWP-related crates (only infrastructure crates such as cfb/zip/quick-xml/
   tiny-skia).

## Things the agent gets wrong

- 2026-09-26 (#370) and 2026-09-27 (#384): new regression tests could not catch their regression (on the old code
  one failed before reaching its assertion; another walked only section paragraphs while the change
  also wrote cell, caption and text-box paragraphs). Run a new test against the pre-fix code, confirm
  it fails at the assertion it was written for, and make it visit every path the change touches.
  Revisit when: reviews stop reporting it, or the model changes.
- 2026-09-26, #368, then the same class in #369's review: under `set -o pipefail`, a pipeline whose
  consumer exits early (`grep -q`, `head`) fails when the producer dies of SIGPIPE, so under load a
  match reads as a miss or the script aborts. In `pipefail` scripts let `grep` read all of its input
  (`grep ... >/dev/null`, not `grep -q`), and add `|| true` where `| head` may legitimately cut the
  stream. Revisit when: the scripts stop using `pipefail`.
- 2026-09-26, #363, #371 and #372: prose lagged the final code (a doc comment kept the old behavior,
  a stale or missing CHANGELOG entry, a PR body grouped differently from the CHANGELOG). Before review
  and after each fix round, re-read the changed functions' doc comments, the CHANGELOG entry and the
  PR body against the final diff. Revisit when: reviews stop reporting it, or the model changes.
