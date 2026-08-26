# CLAUDE.md

A Rust workspace that implements HWP 5.0 (binary) and HWPX (OWPML) **directly**, with no external
HWP library. Code comments are English by default (since 2026-08 - existing Korean comments move to
English as files are touched). User-facing strings (CLI output, error messages) keep their existing
Korean tone.

## Language policy

- **Everything an AI agent reads as development context is English only**: commit messages, PR
  titles/bodies, release notes (`CHANGELOG.md`, GitHub Release bodies), issue text, code comments,
  and internal working docs (`CLAUDE.md`).
- User-facing documentation stays bilingual, but **English is canonical**: `NAME.md` (English) and
  `NAME.ko.md` (Korean).
- Both files carry a **language link on the first line**:
  `[한국어](NAME.ko.md) · [English](NAME.md)`.
- Never edit one side alone. When content changes, **update both in the same commit**.
- `docs/manual/cli-reference.md` (English) and `cli-reference.ko.md` (Korean) are generated from the
  clap definitions. Do not hand-edit them; regenerate with
  `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`.

## CLI help localization

- **English is canonical.** Write the doc comments (= clap help) in `crates/hwp-cli/src/cli.rs` in
  English.
- Korean lives in the `KO` overlay table in `crates/hwp-cli/src/i18n.rs`, applied at runtime.
- Display language precedence: `--lang <en|ko>` → `HWP_LANG` → `LC_ALL` → `LC_MESSAGES` → `LANG` →
  English.
- Adding a command or flag means adding it to the `KO` table too. Missing or dead entries are caught
  by `tests/cli_reference.rs` (the gate that stops entries from silently staying English).

## Build · test

```bash
cargo build                    # debug build (bin: hwp)
scripts/check.sh               # local CI mirror = Rust + fixture gates (required before PR)
HWP_FONT_DIR=$PWD/fonts python3 tools/diagnostic_corpus.py   # diagnostic corpus + self-verification harness
```

- The CI gates (`.github/workflows/ci.yml`) and local runs **must use the same commands**:
  `cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` →
  `cargo test --workspace`. For a partial run (clippy only, test only), pick the command directly
  instead of `scripts/check.sh`.
  CI layout: fmt/clippy plus the structured-corpus gate run once in an ubuntu `lint` job; the
  3-OS `test` matrix runs only `cargo test --workspace`. Tag releases do not re-run the matrix —
  `release.yml` verifies the tagged commit's green CI via the check-runs API (job names there
  must stay in sync with ci.yml).
- Rust edition 2024, rust-version 1.93.
- Fonts: **none are bundled** (`/fonts/` is gitignored - the diagnostic corpus and golden comparison
  use HCR Batang/Dotum if you download them locally). CI render glyphs come from system fonts
  (fonts-nanum on ubuntu - glyf TTFs; the CFF-based noto-cjk made debug-build rendering ~100x
  slower - default CJK on macOS), so tests that run in CI must not assert on
  font-dependent output (glyphs, page counts).
- `HWP_GOLDEN=1` - opt-in golden render comparison against Hangul reference PNGs. `HWP_CORPUS_DIR` -
  soak test over a large in-the-wild corpus.

## Branch · PR policy

- Features, fixes, docs - **all work happens on a branch**: `feat/<topic>`, `fix/<topic>`,
  `docs/<topic>`. No direct pushes to main.
- The canonical repository is `STAIxBWLB/hwp-cli` (= origin). Push branches to origin and **open PRs
  against origin's main**. (The old fork → upstream setup is retired.)
- Submit as a PR and **squash merge once CI is green (ubuntu + macOS + windows all required)**,
  keeping the `(#N)` suffix convention in the merge commit title.
- The pre-PR local gate is `scripts/check.sh` - the same three Rust commands as CI (fmt → clippy
  --all-targets -D warnings → test), the PDF-runner tests, and the structured corpus gate. The
  public PDF oracle gate runs automatically when the pinned `pdfinfo version 24.02.0` is available;
  on other hosts use `HWP_PDF_PARITY=1` to require it explicitly. CI always runs that gate on
  `ubuntu-24.04`. Do not open a PR that does not pass the applicable gates.

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
  regression gate. Its provenance is Mac Hancom HWP 12.30.0 build 6446 on macOS 26.6.1 build
  25G76, Quartz PDFContext, default Save as PDF, A4, one page. This is a bounded local fixture,
  not a universal or Windows parity claim. Private or third-party Hancom artifacts, other oracle
  PDFs/PNGs, and private corpus documents remain forbidden.

## Design knowledge lives in docs/design/

- Start here: [docs/design/00-overview.md](docs/design/00-overview.md) (document index, design
  principles)
- **Required reading**: [07-hangul-compat-rules.md](docs/design/07-hangul-compat-rules.md) - the
  catalog of Hangul compatibility rules established only on real hardware. Touching the writer
  without knowing these rules produces files Hangul cannot open.
- Full format maps: [10-hwp5-structure-map.md](docs/design/10-hwp5-structure-map.md) (record/control
  catalog), [11-hwpx-structure-map.md](docs/design/11-hwpx-structure-map.md) (OWPML element catalog)
- Check [12-feature-gaps.md](docs/design/12-feature-gaps.md) first for unimplemented features.

## Invariants (do not break)

1. **hwp-model depends on no other internal crate** (hub and spoke). `hwp5` and `hwpx` do not depend
   on each other either; they go through the IR.
2. **Lossless round-trip gate**: hwp5 → hwp5 identity re-serialization must be byte-identical
   (`crates/hwp5/tests/identity.rs`). Do not drop unknown records; preserve them as `OpaqueRecord`.
3. **Ground-truth methodology - no guessing**: format behavior is established only by comparing
   against the bytes of genuine files saved by Hangul. The final verdict is whether Hangul
   (Hancom Office) opens the file.
4. No new external HWP-related crates (only infrastructure crates such as cfb/zip/quick-xml/
   tiny-skia).
