# Review instructions

Read by `/codex:review`, `/code-review`, and human reviewers alike. English only, per the language
policy in `CLAUDE.md`.

## Passes

Run these passes and tag every finding with its pass:

- **Bugs**: logic errors, broken edge cases, regressions in parsing, writing, rendering, or conversion.
- **Security**: path traversal on archive entries, unchecked sizes and offsets on untrusted input,
  panics reachable from a malformed document, secrets or absolute local paths in committed files.
- **Compliance**: the change matches the issue spec and the approved plan, and respects the
  invariants and data policy below.

## Repo focus (from CLAUDE.md)

- **Crate direction**: `hwp-model` depends on no other internal crate; `hwp5` and `hwpx` never depend
  on each other and go through the IR.
- **Lossless round-trip**: hwp5 -> hwp5 identity re-serialization stays byte-identical; unknown
  records are preserved as `OpaqueRecord`, never dropped.
- **Ground truth, no guessing**: format behavior is established against bytes written by Hangul, and
  the verdict is whether Hancom Office opens the file. Flag any claim about the format that is not
  backed by a genuine file or a spec section number.
- **No new external HWP crates**; infrastructure crates only.
- **Data policy**: never commit the ground-truth corpus, the Hancom specification or derivatives, or
  private fixtures. The narrow committed exceptions are listed in `CLAUDE.md`.
- **Bilingual docs**: user-facing `NAME.md` and `NAME.ko.md` change in the same commit; the `KO`
  overlay in `crates/hwp-cli/src/i18n.rs` gains an entry whenever a command or flag is added.
- **No font-dependent assertions in CI-run tests**: CI render glyphs come from system fonts, so a
  test that asserts on glyphs or page counts is a finding, not a nit. Gate it behind `HWP_GOLDEN=1`
  or an explicit font directory instead.

## What Important means here

Reserve Important for findings that break behavior, corrupt or lose document data, breach the data
policy, or violate an invariant above. Style and naming are nits.

## Cap the nits

Report at most 5 nits per review; summarize the rest as a count.

## Do not report

- Generated files: `docs/manual/cli-reference.md` and `cli-reference.ko.md` (regenerate with
  `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`).
- Anything CI already enforces: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`, the PDF-runner tests, the Hancom regression gate, the
  structured-corpus gate, claim lint, the documentation-surface gate, the release-readiness
  self-checks, and the public PDF parity oracle.

Font-dependent test expectations are the opposite case: report them, see the focus list above.

## Feedback into CLAUDE.md

When the same finding appears twice, the correction goes into `CLAUDE.md` in the same PR, under
conventions or the invariants list.

---

Findings do not approve or block on their own: a PR is squash merged once CI is green on all three
operating systems, and merges happen on the owner's instruction
(`_meta/rules/development-lifecycle.md` §2, §6).
