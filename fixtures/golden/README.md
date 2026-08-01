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
