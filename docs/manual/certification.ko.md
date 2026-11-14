[한국어](certification.ko.md) · [English](certification.md)

# 인증과 구조 코퍼스

`ahwp certify`가 검사하는 항목과 구조 코퍼스 게이트 실행 방법을 다룬다. 작성한 파일이 고정 정책을 만족한다는 증거나 출력의 재현성이 필요한 사람을 위한 페이지다.

## 문서 인증과 구조 코퍼스 실행

`certify`는 package 검증, 반복 import, bounded native render, 선택적 독립 import를 고정 정책으로
인증하고 새 디렉터리를 원자적으로 게시한다. 정책은 폰트 identity 고정, 폰트 대체 금지, macro/external
reference 금지, bounds/collision/unresolved-field 실패를 강제한다. 정책은 선택적·fail-closed 증거
검사 둘도 고정할 수 있다: `preservation`은 `preservation-report-v1` 아티팩트를 무손실 예산으로
검사하고, `hancom_open`은 `hancom-verification-receipt-v1` attestation을 검사한다. 실패하거나
잘못된 증거는 `overall=failed`로 강제한다.

`corpus`는 자체 작성 한국어 7종 문서를 HWPX/HWP로 각각 2회 생성하고, 두 실행의 문서 바이트·의미 통계·
페이지 PNG 해시·render issue 해시·폰트 identity가 모두 같아야 통과시킨다.

```sh
bash scripts/fetch-corpus-fonts.sh    # 고정 OFL 폰트를 받아 SHA-256 검증 (최초 1회)
ahwp corpus --manifest corpus/structured-v1/manifest.json --report /new/path/corpus-report
```

`scripts/check-structured-corpus.sh`(CI 게이트)가 이 fetch를 먼저 실행하므로 별도 준비는 필요 없다.
계약 상세는 [17-structured-corpus-v1](../design/17-structured-corpus-v1.ko.md), 인증 계약은
[16-certification-v1](../design/16-certification-v1.ko.md).
