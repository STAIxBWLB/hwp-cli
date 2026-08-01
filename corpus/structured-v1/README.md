[한국어](README.ko.md) · [English](README.md)

# Structured document smoke corpus v1

This directory is a self-authored corpus containing no external document samples and no Hancom
output. It generates the representative structures of a Korean official letter, an approval draft, a
report, a business plan, meeting minutes, a university education document and a print application
form, from DocumentSpec v1 or TemplateSpec plus Data.

The font bytes are not committed (about 10 MB). `fonts/` is gitignored and the script below fetches
them from the manifest's `source_url` and verifies each against its SHA-256. When a file with the
right hash is already present, no network is used.

```bash
bash scripts/fetch-corpus-fonts.sh      # populate corpus/structured-v1/fonts/ (once)

hwp corpus \
  --manifest corpus/structured-v1/manifest.json \
  --report /new/path/corpus-report
```

`scripts/check-structured-corpus.sh` (the CI gate) runs that fetch first, so no separate setup is
needed.

The runner refuses an existing report path, works in a private sibling workspace and publishes
atomically. Each case's HWPX and HWP are generated twice in the same process and platform, and both
files are reopened, structurally validated and natively certified. The document bytes, semantic
statistics, page PNG hashes, typed render-issue hashes and font identities of the two runs must all
match. The render hashes are observations for the current OS and architecture under a fixed profile,
not a claim of cross-platform pixel equivalence.

Fixed inputs:

- manifest contract: `hwp-structured-corpus-v1`
- manifest SHA-256: `03ef22e59a45a03d49de5e611f95edebb268a76665feaa63e8fc5d2e92f30dc5`
- run contract: `hwp-structured-corpus-run-v1`
- artifact contract: `hwp-structured-corpus-artifacts-v1`
- manifest, run and artifact schema SHA-256:
  `b8057de94b15deebceb58f014071d57d96fd9bb61603d9cbc2fd94a4398b3b3a`,
  `416466f0c197ec31c64ed76035d3a7b34dbb694c08c459605c9eccfface22706`,
  `3f9effe9df788304ae39bd3f1f460a40bf4b979016d5bc4c5599db386d577c23`
- policy SHA-256: `2da9ef212ac3c5e10c85229d62e307e0c29a8e06a848e47feb039db1fd09fdb8`
- font: Google Fonts Noto Sans KR at revision `2796410152d4f9524b68ed46e69c1b60f8e0f7c3`
- font SHA-256: `194018e6b2b293a7964f037b25c0249ce1418bc9ab3c971060a03aa57861e252`
- license: SIL Open Font License 1.1, with the text and METADATA hash-pinned as well

System fonts and `HWP_FONT_DIR` are not used. The font, policy and spec/data are read through a
secure single-open snapshot, hash-checked and copied into the private workspace. The certification
policy enforces identical font identity, forbids substitution, forbids macros and external
references, renders every page, and fails on bounds, collision and unresolved fields.

HWPX and HWP must also share the `hwp-corpus-common-semantic-v1` target-neutral projection digest.
The projection streams a hash of length-framed fields rather than building the whole JSON, bounded at
100,000 nodes and 64 MiB. It covers metadata and time, PageDef, header/footer/page number, paragraph
style/shape/run/list, table placement/cell/content, fields/bookmarks/equations, and picture
dimensions/description/content hashes. The exact exclusions are defined authoritatively by the closed
`claims.limitations` of the run schema.

Scope limits:

- the seven fixtures are single-page bounded representative smoke cases
- a category name does not imply full feature support for that document type
- charts, advanced drawings, comments and revisions, and encryption, signature and security controls
  are not covered
- no independent LibreOffice oracle and no Hancom render parity is claimed
- manual inspection is not part of the success condition

The normative schemas are `schemas/structured-corpus-v1.schema.json`,
`schemas/structured-corpus-run-v1.schema.json` and
`schemas/structured-corpus-artifacts-v1.schema.json`.
