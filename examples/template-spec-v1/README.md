[한국어](README.md) · [English](README.en.md)

# TemplateSpec/Data v1 예제

`report-template.yaml`은 typed 스칼라 바인딩, 조건 영역, bounded 목록 확장을 보여준다.
`report-data.json`은 `show_summary`와 둘째 항목의 `count`를 의도적으로 빼서 최상위 기본값과
목록 필드 기본값이 리포트에 모두 드러나게 한다.

```sh
hwp template examples/template-spec-v1/report-template.yaml \
  --data examples/template-spec-v1/report-data.json \
  -o report.hwpx --report
```

`--dry-run`은 게시 없이 확장 리포트만 돌려준다. compose와 참조 재생성 모드에서는 dry-run의 출력
의미·패키지 검증이 `not_run`이다. 패키지 외과적 `reference_hwpx` dry-run은 비공개 패키지를 실제로
만들어 검증하므로 두 상태가 모두 `passed`이며 목적지는 그대로다.
