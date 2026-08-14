# Public PDF parity recipe: `public-safety-rfp-p1`

This is the provenance and reproduction record for the single committed public PDF-parity
profile. The source is owner-authored and anonymized; it is not a private or third-party Hancom
artifact.

## Provenance

- Source content: one A4 page containing a title, summary table, comparison table, requirements,
  and a page number. The HWP and HWPX files carry the same anonymized content. Both public source
  files were saved through Mac Hancom HWP so their cached line geometry represents Regime A.
- Oracle producer: Mac Hancom HWP 12.30.0, build 6446.
- Host: macOS 26.6.1, build 25G76.
- Export path: default 파일 → PDF로 저장하기 (Save as PDF) settings.
- PDF producer: `macOS 버전 26.6.1(빌드 25G76) Quartz PDFContext`.
- Output geometry: A4 (595 x 842 pt), one page, no password or encryption.
- Public profile: `hancom-hwp12-macos-12.30.0-build-6446-quartz`.

This is a bounded regression fixture for the named Mac profile. It makes no universal,
cross-platform, or Windows Hancom parity claim. Private Hancom files, third-party documents,
and every other oracle PDF or PNG remain out of scope and must not be committed.

## Pinned hashes

| artifact | repository path | SHA-256 |
|---|---|---|
| HWP source | `fixtures/pdf-parity/public/source/public-safety-rfp-p1.hwp` | `8c4e62fb8166828eaddd2d0d304732acd88484ba8526692545b316985e0c0aba` |
| HWPX source | `fixtures/pdf-parity/public/source/public-safety-rfp-p1.hwpx` | `a5b6bb59bc4492f81deeada58cd4b6c0a13579d06afe39e0aaec2687a3eaaf5c` |
| PDF oracle | `fixtures/pdf-parity/public/oracle/public-safety-rfp-p1.pdf` | `8fe8a4a4f3f6640248a1efde26421c7134374b4203363acca1a5967b2c0602e7` |
| Noto Sans CJK KR 2.004 Regular | fetched into `fixtures/pdf-parity/fonts/NotoSansCJKkr-Regular.otf` | `6bcb2a0703aa137e874fc2dffa85f6c21ba9a67fa329e81b8c801663af7e992a` |
| Noto Sans CJK KR 2.004 Bold | fetched into `fixtures/pdf-parity/fonts/NotoSansCJKkr-Bold.otf` | `26d0c6748500a0444844280b308f5b62c7ae92ac6c6ac88148e502dd211eb52a` |

The two Noto files are exact OFL bytes fetched from the `Sans2.004` upstream revision by
`scripts/fetch-pdf-parity-fonts.sh`; they are deliberately not committed.

## Reproduction and CI gate

On the Mac source host, open each public source in Mac Hancom HWP and verify that the default
Save as PDF export is A4 and one page. Compare the resulting export's producer and digest with
the provenance above before changing the committed oracle.

On the fixed Linux runner, the gate is:

```bash
sudo apt-get update
sudo apt-get install --no-install-recommends -y poppler-utils
test "$(pdfinfo -v 2>&1 | sed -n '1p')" = "pdfinfo version 24.02.0"
bash scripts/fetch-pdf-parity-fonts.sh
cargo build --locked -p hwp-cli
HWP_FONT_DIR=fixtures/pdf-parity/fonts \
  bash scripts/pdf-parity.sh run --oracle-dir fixtures/pdf-parity/public/oracle
```

The workflow runs these steps as the `pdf-parity` job on `ubuntu-24.04`. The v2 manifest repeats
the Poppler, source/oracle, and font pins; the runner fails closed on any mismatch and evaluates
both the HWP and HWPX cases against the one committed PDF.

On a non-Linux development host, `scripts/check.sh` skips this environment-specific gate when
the pinned Poppler binary is unavailable. Set `HWP_PDF_PARITY=1` to make the check required and
see the exact version failure instead of skipping it.
