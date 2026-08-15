[한국어](README.ko.md) · [English](README.md)

# Certification v1 example

Run the native, deterministic certification profile into a new directory:

```sh
hwp certify input.hwpx \
  --policy examples/certification-v1/native-policy.json \
  --report certification-report
```

The report path must not exist. A successful run atomically publishes `report.json`,
`manifest.json`, and the selected `pages/page-NNNNNN.png` artifacts.

This example intentionally disables the independent LibreOffice oracle. It certifies the
bounded hwp-cli parser and renderer contract only; it does not claim Hancom rendering parity.

## Optional evidence checks

The policy also demonstrates the two optional, content-free evidence sections:

- `document.preservation` loads a `preservation-report-v1` artifact (see
  `preservation-report.json`, produced for example by `hwp convert --loss-report`) and fails
  when the aggregated loss total exceeds `max_loss_codes`.
- `document.hancom_open` loads a `hancom-verification-receipt-v1` artifact (see
  `hancom-receipt.json`) attesting that Hancom Office opened the document without repair or
  damage warnings, and fails when the receipt result is not `pass` while `require_pass` holds.

Both companion files here are placeholders. Regenerate them per document: a missing or invalid
artifact fails the certification closed. Omit either section from the policy to skip the check;
reports then keep their previous shape.
