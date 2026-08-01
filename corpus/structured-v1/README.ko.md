[한국어](README.ko.md) · [English](README.md)

# Structured document smoke corpus v1

이 디렉터리는 외부 문서 표본이나 한컴 산출물을 포함하지 않는 자체 작성 코퍼스다. 한국어
공문, 결재 기안문, 보고서, 사업계획서, 회의록, 대학 교육문서, 인쇄 신청서의 대표 구조를
DocumentSpec v1 또는 TemplateSpec+Data로 생성한다.

폰트 바이트는 커밋하지 않는다(약 10 MB). `fonts/`는 gitignore이며 아래 스크립트가 manifest의
`source_url`에서 받아 SHA-256으로 검증한다. 해시가 맞는 파일이 이미 있으면 네트워크를 쓰지 않는다.

```bash
bash scripts/fetch-corpus-fonts.sh      # corpus/structured-v1/fonts/ 채우기 (최초 1회)

hwp corpus \
  --manifest corpus/structured-v1/manifest.json \
  --report /new/path/corpus-report
```

`scripts/check-structured-corpus.sh`(= CI 게이트)는 이 fetch를 먼저 실행하므로 별도 준비가 필요 없다.

러너는 기존 report 경로를 거부하고 비공개 sibling workspace에서 작업한 뒤 원자적으로
게시한다. 각 case의 HWPX/HWP를 같은 프로세스·플랫폼에서 두 번 생성하고, 두 파일을 모두
재개방·구조 검증·native-only 인증한다. 두 실행의 문서 bytes, 의미 통계, page PNG hash,
typed render-issue hash, font identity가 일치해야 통과한다. 렌더 hash는 현재 OS/architecture와
고정 profile의 관측값이며 cross-platform pixel equivalence 주장이 아니다.

고정 입력:

- manifest contract: `hwp-structured-corpus-v1`
- manifest SHA-256: `03ef22e59a45a03d49de5e611f95edebb268a76665feaa63e8fc5d2e92f30dc5`
- run contract: `hwp-structured-corpus-run-v1`
- artifact contract: `hwp-structured-corpus-artifacts-v1`
- manifest/run/artifact schema SHA-256: `b8057de94b15deebceb58f014071d57d96fd9bb61603d9cbc2fd94a4398b3b3a`,
  `416466f0c197ec31c64ed76035d3a7b34dbb694c08c459605c9eccfface22706`,
  `3f9effe9df788304ae39bd3f1f460a40bf4b979016d5bc4c5599db386d577c23`
- policy SHA-256: `2da9ef212ac3c5e10c85229d62e307e0c29a8e06a848e47feb039db1fd09fdb8`
- font: Google Fonts Noto Sans KR at revision `2796410152d4f9524b68ed46e69c1b60f8e0f7c3`
- font SHA-256: `194018e6b2b293a7964f037b25c0249ce1418bc9ab3c971060a03aa57861e252`
- license: SIL Open Font License 1.1, 원문과 METADATA도 hash 고정

시스템 폰트와 `HWP_FONT_DIR`는 사용하지 않는다. 폰트, 정책, spec/data는 secure single-open
snapshot으로 읽고 hash를 확인한 후 비공개 workspace에 복사한다. 인증 정책은 동일한 폰트
identity, substitution 금지, macro/external reference 금지, 전체 페이지 렌더, bounds/collision/
unresolved-field 실패를 강제한다.

HWPX/HWP는 `hwp-corpus-common-semantic-v1` target-neutral projection digest도 같아야 한다.
투영은 전체 JSON을 만들지 않고 length-framed field를 streaming hash하며, 100,000 nodes와
64 MiB로 제한한다. metadata/time, PageDef, header/footer/page number, paragraph style/shape/run/
list, table placement/cell/content, field/bookmark/equation, picture dimensions/description/content
hash를 포함한다. 정확한 제외 항목은 run schema의 closed `claims.limitations`가 정본이다.

범위 제한:

- 7개 fixture는 single-page bounded representative smoke case임
- category 이름은 해당 문서 유형의 모든 기능 지원을 뜻하지 않음
- chart, 고급 drawing, comments/revisions, 암호화·전자서명·보안 control을 다루지 않음
- 독립 LibreOffice oracle 및 한컴 렌더 parity를 주장하지 않음
- 수작업 확인은 성공 조건에 포함하지 않음

정본 schema는 `schemas/structured-corpus-v1.schema.json`,
`schemas/structured-corpus-run-v1.schema.json`,
`schemas/structured-corpus-artifacts-v1.schema.json`이다.
