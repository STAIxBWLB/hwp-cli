[한국어](README.ko.md) · [English](README.md)

# DocumentSpec v1 예제

`basic.json`과 `comprehensive.yaml`은 자급식이다.

```bash
hwp compose examples/document-spec-v1/basic.json -o /tmp/basic.hwpx
hwp compose examples/document-spec-v1/comprehensive.yaml -o /tmp/comprehensive.hwpx
```

`image-block.yaml`은 자산 상대경로 이미지 저작을 보여준다. `examples/document-spec-v1/assets/logo.png`에
유효한 PNG를 두거나 `path`를 기존 PNG/JPEG/GIF/BMP로 바꿔라. 생성된 문서는 수동 편집이 필요 없다.
자산 파일은 임베드되며 `height_mm`을 생략하면 고유 종횡비가 유지된다.
