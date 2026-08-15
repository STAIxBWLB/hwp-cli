[한국어](release-readiness.ko.md) · [English](release-readiness.md)

# Release readiness checklist

Run from a clean checkout. This checklist records gates; it does not authorize commit, push, tag,
package upload or release publication.

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `scripts/check-structured-corpus.sh`
- [ ] Linux, macOS and Windows CI matrix all green from local source builds
- [ ] corpus manifest/run/artifact JSON valid against their frozen schemas
- [ ] all manifest pins and `corpus/structured-v1/TRACKED_FILES.txt` present in the Git index
- [ ] no ambient font install or `HWP_FONT_DIR` dependency in the corpus job
- [ ] `scripts/fetch-corpus-fonts.sh` reproduces the pinned font hashes from a clean `fonts/`
- [ ] release archive/license inventory includes Noto Sans KR OFL and metadata if the corpus ships
- [ ] independent oracle remains partial until a real digest-pinned image is built and attested
- [ ] private Hancom-open verification receipt recorded against the
      `hancom-verification-receipt-v1` schema (certification `hancom_open` evidence)
- [ ] private PDF parity run eligible; every manifest-declared `gate_exclusions` entry listed
      and justified in the release notes (currently: `fonts` excluded — approved faces not yet
      pinned)
- [ ] residual failing parity gates (`text`, `raster`, `roi`) documented and treated as
      release-blocking until resolved
- [ ] `git status --short --untracked-files=all` reviewed; unrelated user changes excluded
- [ ] no commit, push, tag, package upload or release performed by the readiness run
- [ ] downstream `STAIxBWLB/skills` `skills/hwpx` reviewed for CLI-surface drift (that repo's
      `upstream-hwp-cli` workflow files an issue within a day, but a release that changes the CLI
      surface should not wait for the cron)

The release must not claim that the seven smoke fixtures cover every real document form, provide
Hancom pixel parity, or prove cross-platform-identical raster bytes.
