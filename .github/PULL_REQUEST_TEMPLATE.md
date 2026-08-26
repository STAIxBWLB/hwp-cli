<!-- hwp-cli PR checklist — do not delete; tick each item after checking it -->

## Summary

<!-- The purpose and scope of the change, in two or three lines -->

## Checklist

- [ ] `scripts/check.sh` passes (fmt → clippy --all-targets -D warnings → test; the same gates as CI)
- [ ] Does this change need verification in Hancom Office? (writer or compatibility-rule impact)
  - If it does: [ ] the result is recorded in the PR body (opens without a corruption dialog, layout checked)
  - If it does not: [ ] one line of justification (for example: read-only, tests or docs only)
- [ ] Data policy respected (no fixtures committed beyond the fixtures/samples exception, no Hancom specification or derivatives bundled — CLAUDE.md "Data policy")
- [ ] Design documents updated where applicable: `docs/design/12-feature-gaps.md` statuses, the structure maps (10 and 11), README and CLAUDE.md
- [ ] New features come with tests (round-trip, golden or CLI surface, whichever path applies)
- [ ] User-facing documentation updated on both sides (`NAME.md` and `NAME.ko.md`) in the same commit

## Notes

- Branch and PR policy: CLAUDE.md "Branch · PR policy". No direct pushes to main; squash merge once CI is green.
