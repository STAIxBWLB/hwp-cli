[한국어](README.md) · [English](README.en.md)

# TemplateSpec/Data v1 examples

`report-template.yaml` demonstrates typed scalar binding, a conditional region,
and bounded list expansion. `report-data.json` intentionally omits `show_summary`
and the second item's `count` so both top-level and list-field defaults are visible
in the report.

```sh
hwp template examples/template-spec-v1/report-template.yaml \
  --data examples/template-spec-v1/report-data.json \
  -o report.hwpx --report
```

Use `--dry-run` to return the expansion report without publishing. In compose and
reference-regeneration modes, output semantic/package validation is `not_run` in
dry-run. Package-surgical `reference_hwpx` dry-run materializes and validates a
private package, so both statuses are `passed` while the destination remains untouched.
