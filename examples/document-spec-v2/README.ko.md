[한국어](README.ko.md) · [English](README.md)

# DocumentSpec v2 예제

이 예제는 닫힌 SVG 부분집합을 쓰고 두 타겟 모두에 대해 결정론적 PNG fallback을 명시적으로 선택한다.

```bash
hwp compose examples/document-spec-v2/basic.json -o /tmp/document-spec-v2.hwpx --report
hwp compose examples/document-spec-v2/basic.json -o /tmp/document-spec-v2.hwp --report
hwp compose examples/document-spec-v2/native-text-box.json -o /tmp/native-text-box.hwpx --report
```

`visual.svg`는 검증·정규화·래스터화된다. 임베드되는 것은 생성된 PNG뿐이다.
