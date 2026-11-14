[한국어](certification.ko.md) · [English](certification.md)

# Certification and the structured corpus

What `ahwp certify` checks and how the structured corpus gate runs, for anyone who needs evidence that a written file meets a frozen policy or that output is reproducible.

## Certify a document and run the structured corpus

`certify` runs package validation, repeated import, bounded native rendering and an optional
independent import under a frozen policy, then publishes a new directory atomically. The policy pins
font identity and forbids font substitution, macros and external references, and it fails on bounds,
collision and unresolved-field problems. The policy may also pin two optional, fail-closed evidence
checks: `preservation` ingests a `preservation-report-v1` artifact with a zero-loss budget, and
`hancom_open` ingests a `hancom-verification-receipt-v1` attestation; failed or invalid evidence
forces `overall=failed`.

`corpus` generates seven self-authored Korean documents as both HWPX and HWP, twice each, and passes
only when the document bytes, semantic statistics, page PNG hashes, render-issue hashes and font
identities all agree across the two runs.

```sh
bash scripts/fetch-corpus-fonts.sh    # fetch and SHA-256 verify the pinned OFL font (once)
ahwp corpus --manifest corpus/structured-v1/manifest.json --report /new/path/corpus-report
```

`scripts/check-structured-corpus.sh` (the CI gate) runs that fetch first, so no separate setup is
needed. See [17-structured-corpus-v1](../design/17-structured-corpus-v1.md) for the corpus contract
and [16-certification-v1](../design/16-certification-v1.md) for certification.
