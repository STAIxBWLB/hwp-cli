[한국어](release-readiness.md) · [English](release-readiness.en.md)

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
- [ ] `git status --short --untracked-files=all` reviewed; unrelated user changes excluded
- [ ] no commit, push, tag, package upload or release performed by the readiness run

The release must not claim that the seven smoke fixtures cover every real document form, provide
Hancom pixel parity, or prove cross-platform-identical raster bytes.
