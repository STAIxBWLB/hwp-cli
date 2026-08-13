[한국어](README.ko.md) · [English](README.md)

# Golden (reference) render images, for comparison against Hancom

The `hwp diff` and `golden` tests measure error by comparing our render against **reference images
exported from Hancom**. Put the per-page reference PNGs in this directory (the images are gitignored;
only the recipe is committed).

## How to make a reference image (in Hancom)

1. Open the target document in Hancom.
2. **File → Print → Save as PDF** (or **File → Save As → PDF**).
3. Rasterize the PDF to PNG at a fixed DPI. **150 DPI** is recommended (characters stay legible):

   ```sh
   # macOS: sips or pdftoppm (brew install poppler)
   pdftoppm -png -r 150 document.pdf document          # document-1.png, document-2.png ...
   ```

   - Hancom's "save as image" also works, but the DPI and scale must be fixed.
4. Name the file `<fixture name>.p<page>.ref.png`, for example `work_report.p1.ref.png`.

## Comparing against our render

Render at the same DPI and compare (the dimensions must match):

```sh
HWP_FONT_DIR=$PWD/fonts \
  ./target/release/hwp diff fixtures/hwp5/work_report.hwp \
  --ref fixtures/golden/work_report.p1.ref.png --page 1 --dpi 150 -o /tmp/diff.png
```

Output: `bad_pixel_pct` (the pixel difference ratio), `MAE` and `dx/dy` (the position offset), plus a
difference image (red = ours only, blue = reference only, grey = matching).

## Pinning fonts

Getting the same character widths and line breaking as Hancom requires the same fonts. Put HCR Batang
and HCR Dotum in `fonts/` (gitignored) and point `HWP_FONT_DIR` at it. Documents such as
annual_report may also need NanumGothic and NanumMyeongjo (without them the HCR fonts substitute and
glyph shape error grows, which is separate from position error and is measured by `dx/dy`).

## The golden test

`HWP_GOLDEN=1 cargo test -p hwp-render golden` compares the `*.ref.png` files in this directory
automatically (passing or skipping when an image is absent). Tighten the thresholds step by step to
prevent regressions. It is skipped by default in CI, which has no fonts (the structural smoke test in
`tests/render.rs` always runs).

## PDF parity baselines (issue #79)

The batch runner `scripts/pdf-parity.sh` scores our PDF against Hancom's own PDF with the
five-metric set of [docs/design/21-pdf-parity.md](../../docs/design/21-pdf-parity.md) §3
(`pdffonts`, `pdfinfo`, per-page `pdftotext -layout`, and `dx/dy` + `bad_pixel_pct`/`MAE`
after rasterizing both PDFs with the same `pdftoppm -png -r 150`).

Per-case baseline procedure (owner, on Windows Hancom Office 2024):

1. Author or anonymize the source document and commit it under
   `fixtures/pdf-parity/public/source/` (HWP/HWPX only — the only committable artifacts).
2. In Hancom: **File → Save as PDF** with default settings; record the exact Hancom build,
   Windows version and PDF settings in `fixtures/pdf-parity/public/manifest.json` (`pins`),
   plus the SHA-256 of the pinned fonts (HCR Batang/Dotum in `fonts/`). Set `HWP_FONT_DIR` when
   the pinned font directory is not the repository's `fonts/` directory.
3. Keep the exported PDF local — put it in `$HWP_PDF_PARITY_ORACLE_DIR` (never committed;
   the whole oracle tree is gitignored).
4. Add the case to the manifest: `{name, source, source_sha256, oracle, oracle_sha256}`.
5. Run:

   ```sh
   scripts/pdf-parity.sh run --oracle-dir "$HWP_PDF_PARITY_ORACLE_DIR"
   ```

   The scoreboard (`public/scoreboard/<case>.json`, `scoreboard.json`, `scoreboard.csv`)
   contains names, SHA-256 hashes and numbers only — no paths, no oracle bytes — and is the
   only output that gets committed. Before rendering, the runner validates the closed manifest
   schema and verifies the Poppler version, pinned font files, and every source/oracle digest.
   Missing font coverage, any substitution, a page-count delta, or a PDF font that is not
   embedded/subset/Unicode-capable records the case as `"scored": false`.

`scripts/pdf-parity.sh selftest` checks the harness itself (a fixture against its own PDF
must produce perfect metrics) and needs no Hancom baseline.
