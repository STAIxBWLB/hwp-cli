[한국어](README.md) · [English](README.en.md)

# Certification v1 예제

네이티브·결정론적 인증 프로파일을 새 디렉터리로 실행한다.

```sh
hwp certify input.hwpx \
  --policy examples/certification-v1/native-policy.json \
  --report certification-report
```

report 경로는 미리 존재하면 안 된다. 성공하면 `report.json`, `manifest.json`, 선택된
`pages/page-NNNNNN.png` 산출물을 원자적으로 게시한다.

이 예제는 독립 LibreOffice 오라클을 의도적으로 비활성화한다. bounded hwp-cli 파서·렌더러 계약만
인증하며 한컴 렌더 동등성을 주장하지 않는다.
