[한국어](README.ko.md) · [English](README.md)

# DocumentSpec v2 example

The example uses the closed SVG subset and explicitly opts into deterministic PNG fallback for both
targets:

```bash
hwp compose examples/document-spec-v2/basic.json -o /tmp/document-spec-v2.hwpx --report
hwp compose examples/document-spec-v2/basic.json -o /tmp/document-spec-v2.hwp --report
hwp compose examples/document-spec-v2/native-text-box.json -o /tmp/native-text-box.hwpx --report
```

`visual.svg` is validated, canonicalized, and rasterized. Only the generated PNG is embedded.
