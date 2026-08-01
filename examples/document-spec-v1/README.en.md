[한국어](README.md) · [English](README.en.md)

# DocumentSpec v1 examples

`basic.json` and `comprehensive.yaml` are self-contained:

```bash
hwp compose examples/document-spec-v1/basic.json -o /tmp/basic.hwpx
hwp compose examples/document-spec-v1/comprehensive.yaml -o /tmp/comprehensive.hwpx
```

`image-block.yaml` demonstrates asset-relative image authoring. Put a valid PNG at
`examples/document-spec-v1/assets/logo.png`, or change `path` to an existing PNG/JPEG/GIF/BMP.
The generated document needs no manual editing; the asset file is embedded and an omitted
`height_mm` preserves its intrinsic aspect ratio.
