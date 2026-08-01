[한국어](09-diagnostic-corpus.md) · [English](09-diagnostic-corpus.en.md)

# 09. 진단 코퍼스 & 검증 하네스

> 현재 무엇이 되고 안 되는지 **정밀 진단**하기 위한 기능 격리 테스트 코퍼스와 자체 검증
> 하네스. 각 파일이 한 기능만 담아, 실패 시 어느 기능인지 정확히 핀포인트된다.

## 구성

**하네스:** `tools/diagnostic_corpus.py` — 기능별 markdown 케이스를 hwp/hwpx로 생성하고
fixtures와 함께 자체 검증을 돌려 pass/fail 매트릭스를 출력한다.

```
HWP_FONT_DIR=$PWD/fonts python3 tools/diagnostic_corpus.py [출력디렉토리]
```

**생성 케이스(각 = 한 기능, hwp+hwpx 양쪽):** single_para, multi_para, headings, bullet_list,
numbered_list, formatting, long_para, table_2x2, table_header_only, table_multiline,
table_empty_cells, multipage, special_chars, mixed, nested_list, blockquote, code_block,
link(→하이퍼링크), deep_heading, hr_rule, table_wide, table_long. (코퍼스 확장이 링크→하이퍼링크,
인용문·코드블록 스타일 갭을 발견·수정하게 함 — 진단 주도 개발.)

**fixtures(실제 문서):** hello_world, work_report, annual_report, color_fill, outline,
bookmark(hwp5), minimal(hwpx).

## 검사 종류 (self-verifiable — 한글 없이 자동)

| 검사 | 내용 | 잡는 문제 |
|---|---|---|
| 생성 | `hwp new --from md` 성공 | markdown→문서 생성 크래시 |
| 구조 | hwpx=`validate`(mimetype/엔트리/XML), hwp5=CFB/olefile | 구조 손상 |
| 외부파서 | hwpx=zip+ET 파싱, hwp5=olefile 스트림 | 외부 도구 호환 |
| 렌더 | `hwp render` 크래시 없음 | 렌더 파이프라인 오류 |
| strict변환 | `convert --strict`(opaque 손실 시 실패) | DROP·미보존 |
| 텍스트보존 | 원본 md 텍스트가 생성 파일 cat에 포함(≥90%) | 텍스트 유실 |
| 교차변환 | hwp5→hwpx(또는 역) + 결과 구조·렌더 | 변환 파이프라인 |

## 현재 상태 (2026-07 기준)

**self-verifiable 검사 51케이스 × 10검사 전부 ✅ — 구조/변환/렌더 수준 문제·회귀 없음.**

이 하네스로 **못 잡는** 것 = **한글(한컴오피스) 특정 렌더 동작**. 예: annual 6쪽 자리표시자
글상자 드롭 + 빈 페이지(한글 미문서화 heavy-content 동작 — 조사 종결·수용, [07](07-hangul-compat-rules.md)).
이런 이슈는 **정품 한글 실기**로만 판정된다 → `~/Documents/hwp-진단코퍼스/README-한글검토.md`의
체크리스트로 사용자가 각 파일을 한글에서 열어 확인.

## 확장 방법

- 새 기능 케이스: `CASES` 딕셔너리에 `이름: markdown` 추가.
- 새 fixture: `FIXTURES` 리스트에 추가.
- 새 검사: 함수 작성 후 생성/fixture 루프의 `checks`에 항목 추가.
- CI 게이트로 쓰려면 실패(❌) 시 종료코드 비0 반환하도록 `main` 말미 보강.

## 진단 철학

두 층으로 나눈다: **(1) 자체 검증**(자동·빠름·회귀 방지)은 구조·변환·렌더·텍스트를 커버하고,
**(2) 한글 실기**(수동·정답지)는 자체 검증이 원리적으로 못 보는 한글 특정 렌더/페이지네이션을
커버한다. 문제가 나오면 격리된 케이스라 원인 기능이 즉시 특정된다.
