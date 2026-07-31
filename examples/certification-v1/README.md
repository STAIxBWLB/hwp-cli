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
