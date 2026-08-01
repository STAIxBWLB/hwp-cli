[한국어](README.ko.md) · [English](README.md)

# docs

Notes on the HWP 5.0 format specification material used when working on the parser and renderer.

## Layout

| Path | Contents |
|---|---|
| [design/](design/) | The design knowledge of record. Start at [00-overview](design/00-overview.md) |
| [manual/cli-reference.md](manual/cli-reference.md) | CLI reference generated from the clap definitions (do not edit by hand) |
| [release-readiness.md](release-readiness.md) | Pre-release gate checklist |
| [hancom-verification-checklist.md](hancom-verification-checklist.md) | Checklist for verifying files in Hancom Office |

User-facing documents are bilingual: English is canonical (`NAME.md`) and Korean is its pair
(`NAME.ko.md`).
Both `manual/cli-reference*.md` files are generated from the clap definitions in either language
(do not edit them by hand).

## The specification is not bundled with this repository

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

## Acknowledgment

This product was developed with reference to Hancom's HWP document file (.hwp) open specification.
