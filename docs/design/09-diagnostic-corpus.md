[한국어](09-diagnostic-corpus.ko.md) · [English](09-diagnostic-corpus.md)

# 09. Diagnostic corpus and verification harness

> A feature-isolating test corpus and self-verification harness for **precisely diagnosing** what
> currently works and what does not. Each file exercises exactly one feature, so a failure pinpoints
> which feature broke.

## Composition

**Harness:** `tools/diagnostic_corpus.py` generates hwp and hwpx from per-feature markdown cases and
runs self-verification over them together with the fixtures, printing a pass/fail matrix.

```
HWP_FONT_DIR=$PWD/fonts python3 tools/diagnostic_corpus.py [output directory]
```

**Generated cases (one feature each, in both hwp and hwpx):** single_para, multi_para, headings,
bullet_list, numbered_list, formatting, long_para, table_2x2, table_header_only, table_multiline,
table_empty_cells, multipage, special_chars, mixed, nested_list, blockquote, code_block, link
(becomes a hyperlink), deep_heading, hr_rule, table_wide, table_long. (Expanding the corpus is what
surfaced and fixed the link-to-hyperlink gap and the blockquote and code-block style gaps: diagnosis
-driven development.)

**Fixtures (real documents):** hello_world, work_report, annual_report, color_fill, outline,
bookmark (hwp5), minimal (hwpx).

## Check types (self-verifiable, automatic, no Hancom needed)

| Check | What it does | What it catches |
|---|---|---|
| Generation | `hwp new --from md` succeeds | Crashes generating a document from markdown |
| Structure | hwpx uses `validate` (mimetype, entries, XML); hwp5 uses CFB/olefile | Structural corruption |
| External parser | hwpx via zip + ElementTree; hwp5 via olefile streams | External tool compatibility |
| Render | `hwp render` does not crash | Render pipeline errors |
| Strict conversion | `convert --strict` (fails on opaque loss) | DROP and non-preservation |
| Text preservation | The original markdown text appears in `cat` of the generated file (≥90%) | Text loss |
| Cross-conversion | hwp5 → hwpx (or the reverse) plus structure and render of the result | Conversion pipeline |

## Current status (as of 2026-07)

**All 51 self-verifiable cases pass all 10 checks**, so there are no structural, conversion or render
level problems or regressions.

What this harness **cannot** catch is **Hancom-specific render behavior**. For example, the
placeholder text-box drop plus blank page on page 6 of the annual report (undocumented Hancom
heavy-content behavior; investigation closed and accepted, see
[07](07-hangul-compat-rules.md)). Such issues are decided only by **testing in genuine Hancom
Office**, using the checklist in `~/Documents/hwp-진단코퍼스/README-한글검토.md` where the user opens
each file in Hancom.

## How to extend it

- New feature case: add `name: markdown` to the `CASES` dictionary.
- New fixture: add it to the `FIXTURES` list.
- New check: write the function and add an entry to `checks` in the generation and fixture loops.
- To use it as a CI gate, extend the end of `main` to return a non-zero exit code on any failure.

## Diagnostic philosophy

There are two layers. **Self-verification** (automatic, fast, regression-preventing) covers structure,
conversion, rendering and text, while **Hancom testing** (manual, ground truth) covers the
Hancom-specific rendering and pagination that self-verification cannot see in principle. When a
problem appears, the isolated case identifies the responsible feature immediately.
