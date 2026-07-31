[한국어](17-structured-corpus-v1.md) · [English](17-structured-corpus-v1.en.md)

# Structured corpus v1

## Purpose

The corpus is a checked-in, open-source-only release gate for representative Korean structured
authoring. It replaces the legacy diagnostic Python script as a success signal. It does not use
external HWP/HWPX samples and does not require manual inspection.

Specs, policy and manifest are checked in. The OFL font, its license and its metadata are not:
`scripts/fetch-corpus-fonts.sh` provisions them from the manifest's pinned `source_url` into the
gitignored `corpus/structured-v1/fonts/` and verifies each against the manifest SHA-256, so the
repository stays free of a large binary while font identity stays pinned. The fetcher accepts only
`https` from the pinned host, only bounded responses, and only relative in-corpus destinations; the
runner re-verifies every byte it reads regardless.

## Fixed cases

| ID | Representative category | Source contract | Outputs |
|---|---|---|---|
| `official-letter` | Korean official letter | DocumentSpec v1 | HWPX, HWP |
| `approval-memo` | approval/draft memo layout | DocumentSpec v1 | HWPX, HWP |
| `report` | performance report | DocumentSpec v1 | HWPX, HWP |
| `business-plan` | business plan | DocumentSpec v1 | HWPX, HWP |
| `meeting-minutes` | meeting minutes | DocumentSpec v1 | HWPX, HWP |
| `academic-education` | academic/education plan | DocumentSpec v1 | HWPX, HWP |
| `print-form` | print application form | TemplateSpec+Data v1 | HWPX, HWP |

These labels describe fixture intent, not complete support for every feature used by real documents
in the category. The machine summary repeats that limitation.

## Gate sequence

For each format, the runner:

1. reads the frozen manifest, policy, font/license/metadata and source through bounded contained
   snapshots;
2. generates run A and run B using only the pinned font file;
3. requires byte identity within that process and platform;
4. reopens and structurally validates both files;
5. checks fixed required Korean text and bounded semantic counts for both;
   the common HWPX/HWP structural projection is domain-separated as
   `hwp-corpus-common-semantic-v1` and must have the same digest;
6. certifies both with the frozen native-only policy;
7. compares page count, selected page PNG hashes, typed render-issue hashes and resolved font
   identities across the two certifications;
8. publishes a closed summary and content-addressed artifact list atomically.

There is no global expected PNG hash. Raster hashes may differ across OS/architecture, so the
contract records the platform profile and checks paired determinism only within one run.

## Bounds and privacy

- manifest and summary: 1 MiB each
- cases: exactly 7, implementation ceiling 32
- artifact files: at most 255 before `artifacts.json`, 256 including it
- directories: 128; depth: 8; relative path: 512 ASCII bytes
- one artifact file: 128 MiB; total tree: 512 MiB
- semantic projection: 100,000 nodes and 64 MiB of length-framed projected fields
- no symlink, reparse point or multiply-linked input/artifact
- no raw document text, child output, absolute input path or exception message in the summary

The command returns zero only when every case and format passes. A failed completed run still
publishes the bounded report and returns nonzero. Manifest/input contract rejection publishes
nothing.

## Coverage limitations

The v1 fixtures do not certify advanced drawings/charts, equations, images, notes/citations,
TOC/index, mixed page sections, revisions/comments, encryption/signatures/macros, accessibility
tagging, or independent office-suite import. Those require additional frozen cases and policies;
they must not be inferred from the seven category labels.

The common semantic digest is a bounded, streaming, target-neutral projection. It covers
representable metadata and timestamps, section page definitions, header/footer/page-number
controls, paragraph text/control order, resolved paragraph/style/character shapes and list
definitions, table placement and cell geometry/content, modeled fields/bookmarks/equations, and
picture dimensions/description/content hashes. It intentionally excludes line-layout caches,
opaque records, raw format-specific control payloads, pass-through XML, and advanced drawing
geometry/style. It also excludes strike and underline-shape details that the HWP5 reader
deliberately cannot interpret unambiguously; common underline presence/kind remains covered. Those
exclusions are repeated as a closed list in `summary.json`; digest equality must not be interpreted
as byte-for-byte IR equality outside the profile.
