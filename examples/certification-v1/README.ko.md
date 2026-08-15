[한국어](README.ko.md) · [English](README.md)

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

## 선택적 증거 검사

이 정책은 내용 없는(content-free) 선택적 증거 섹션 두 가지도 함께 보여 준다.

- `document.preservation`은 `preservation-report-v1` 산출물(예: `preservation-report.json`,
  `hwp convert --loss-report`로 생성)을 읽어 손실 합계가 `max_loss_codes`를 넘으면 실패한다.
- `document.hancom_open`은 한컴오피스가 복구·손상 경고 없이 문서를 열었다는
  `hancom-verification-receipt-v1` 산출물(`hancom-receipt.json` 참고)을 읽고,
  `require_pass`가 참일 때 receipt 결과가 `pass`가 아니면 실패한다.

여기 포함된 두 동반 파일은 플레이스홀더다. 문서마다 다시 생성해야 하며, 산출물이 없거나
유효하지 않으면 인증은 닫힌 실패(fail closed)를 반환한다. 정책에서 섹션을 빼면 검사를
걸어넘고, 리포트는 이전 형태를 그대로 유지한다.
