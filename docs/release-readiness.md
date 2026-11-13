[한국어](release-readiness.ko.md) · [English](release-readiness.md)

# Release readiness checklist

Run from a clean checkout. This checklist records gates; it does not authorize commit, push, tag,
package upload or release publication.

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `scripts/check-structured-corpus.sh`
- [ ] Linux, macOS and Windows CI matrix all green from local source builds (the green signal
      can be the commit's own check runs or, for a main push that ci.yml deduplicated, a merged
      PR head's green runs on the identical tree)
- [ ] corpus manifest/run/artifact JSON valid against their frozen schemas
- [ ] all manifest pins and `corpus/structured-v1/TRACKED_FILES.txt` present in the Git index
- [ ] no ambient font install or `HWP_FONT_DIR` dependency in the corpus job
- [ ] `scripts/fetch-corpus-fonts.sh` reproduces the pinned font hashes from a clean `fonts/`
- [ ] release archive/license inventory includes Noto Sans KR OFL and metadata if the corpus ships
- [ ] independent oracle remains partial until a real digest-pinned image is built and attested
- [ ] private Hancom-open verification receipt recorded against the
      `hancom-verification-receipt-v1` schema (certification `hancom_open` evidence)
- [ ] private PDF parity run eligible; every manifest-declared `gate_exclusions` entry listed
      and justified in the release notes. Currently four: `fonts` (the oracle itself substitutes
      through the document's own `substFont`, so `substitution_free` is unreachable for that
      case — 21-pdf-parity §4.5), and `text`/`raster`/`roi` (measured and echoed, distance
      recorded in 21-pdf-parity §4.6)
- [ ] release copy states the excluded gates and their measured distance; no "Hancom parity"
      claim is made for a profile with exclusions
- [ ] `git status --short --untracked-files=all` reviewed; unrelated user changes excluded
- [ ] no commit, push, tag, package upload or release performed by the readiness run
- [ ] version-pinned examples in `README.md` / `README.ko.md` (the `install.sh --tag` snippets)
      bumped to the release being cut
- [ ] tool-count claims agree with the server: `ahwp mcp` `tools/list` length equals every "N tools"
      in `README*.md`, `docs/manual/*.md` and `skills/hwp/SKILL*.md`
- [ ] bundled `skills/hwp/SKILL.md` and `SKILL.ko.md` cover every command and MCP tool the release
      adds — an exported skill is what agents read, and nothing gates its content
- [ ] the retired downstream `STAIxBWLB/skills` `skills/hwpx` still matches the parity matrix in
      [design/23-hwpx-skill-absorption](design/23-hwpx-skill-absorption.md) — the parity record of
      what the retirement (RET-01, closed 2026-08-27) replaced
- [ ] `HWP_REQUIRE_FIXTURES=1 scripts/check.sh` passes locally with the local-only fixtures in
      place (`fixtures/hwp5/`, `fixtures/hwpx/`, `fixtures/pdf-parity/private/`; the optional
      sets in `fixtures/README.md` are only counted). CI and the readiness workflow have none by
      the data policy, so they do not set the flag (it would always fail); they print
      `skipped-for-missing-fixtures=N (optional=M)`: green CI covers what N skips leave (#275)
- [ ] `bash scripts/macos-release-signing.sh --dry-run` passes; this checks helper syntax and
      records the signing plan only, not Apple credentials or live signing/notarization

The release must not claim that the seven smoke fixtures cover every real document form, provide
Hancom pixel parity, or prove cross-platform-identical raster bytes.

## macOS signing and notarization

The macOS release legs require these repository Actions secrets under
`Settings > Secrets and variables > Actions`. Missing values stop the macOS leg before the upload
action; there is no ad-hoc fallback.

- `APPLE_CERTIFICATE`: base64-encoded Developer ID Application `.p12`
- `APPLE_CERTIFICATE_PASSWORD`: password for that `.p12`
- `KEYCHAIN_PASSWORD`: password for the temporary signing keychain
- `APPLE_API_KEY`: App Store Connect `.p8` PEM contents (base64 PEM is also accepted)
- `APPLE_API_KEY_ID`: App Store Connect API key ID
- `APPLE_API_ISSUER_ID`: App Store Connect issuer ID

Also set the repository Actions **variable** `APPLE_TEAM_ID` under
`Settings > Secrets and variables > Actions > Variables` to the 10-character Apple Developer team
ID. It is public signer metadata, not a secret. A missing or invalid value stops verification.

The workflow imports the certificate into a restricted temporary keychain and passes its one valid
Developer ID Application identity to the pinned upload action, with `codesign-options: runtime`.
Developer ID signing timestamps by default; the helper refuses a missing secure timestamp, hardened
runtime, or Developer ID Application authority before submitting a ZIP of the signed executable to
`notarytool submit --wait`. Only `Accepted` passes. The temporary certificate, API key and keychain
are removed by the helper and the always-run cleanup step.

After the Linux alias repack, a macOS job downloads each architecture's archive, compares `ahwp` and
`hwp` byte-for-byte and verifies both code signatures against the Apple anchor and the certificate
leaf team ID from `vars.APPLE_TEAM_ID`. It also runs `spctl --assess --type install --verbose=2` on
both commands, requiring a successful result with `accepted` and `source=Notarized Developer ID`.
Each assessment allows at most three attempts, with ten seconds between attempts (twenty seconds
of retry delay per command), for ticket propagation. Exhaustion fails the job and names the retry
bound and the next diagnostic step. Attestation and public release wait for that job. A bare executable cannot be stapled, so Gatekeeper retrieves the notarization ticket online.

Readiness runs only the credential-free `--dry-run` path. Shell mocks exercise failure paths without
Apple services; neither result proves that real credentials, notarization, or Gatekeeper work.
The first live acceptance check is the owner-authorized `v1.6.3` release. Record both macOS
`verify-macos-assets` jobs and assess both downloaded commands with Gatekeeper. In an empty temporary
directory, the Apple Silicon example is:

```bash
gh release download v1.6.3 --repo STAIxBWLB/agent-hwp --pattern 'ahwp-v1.6.3-aarch64-apple-darwin.tar.gz'
tar -xzf ahwp-v1.6.3-aarch64-apple-darwin.tar.gz
APPLE_TEAM_ID="$(gh variable get APPLE_TEAM_ID --repo STAIxBWLB/agent-hwp)"
signer_requirement="anchor apple generic and certificate leaf[subject.OU] = \"$APPLE_TEAM_ID\""
codesign --verify --strict --verbose=2 -R="$signer_requirement" ./ahwp
codesign -dv --verbose=4 ./ahwp
codesign --verify --strict --verbose=2 -R="$signer_requirement" ./hwp
codesign -dv --verbose=4 ./hwp
spctl --assess --type install --verbose=2 ./ahwp
spctl --assess --type install --verbose=2 ./hwp
```

Repeat signature and Gatekeeper assessment with the `x86_64-apple-darwin` archive. The authority must
be Developer ID Application, and the signatures must carry hardened runtime and a secure timestamp.
The expected Gatekeeper output is `accepted` with `source=Notarized Developer ID`. Readiness does
not create a tag or perform this live check.
