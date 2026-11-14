[한국어](markdown-export.ko.md) · [English](markdown-export.md)

# Markdown 내보내기

`ahwp`가 HWP/HWPX를 GFM으로 바꾸는 규칙과 `--with-segments`가 더하는 정보를 설명한다. 검색·RAG용 추출이나 원본 문단 단위 인용이 필요한 에이전트를 위한 페이지다.

## 문서를 Markdown으로 내보내기

hwp/hwpx를 GFM으로 변환한다. `ahwp cat --format markdown`은 stdout 전용이라 이미지를 추출하지 않는다.

```sh
ahwp convert report.hwp -o report.md                    # 이미지는 report.media/에 추출
ahwp convert report.hwp -o report.md --media-dir figs   # figs/에 추출하고 figs/... 로 링크
ahwp convert report.hwp -o full.md --with-header-footer --with-hidden
```

| HWP 요소 | markdown |
|---|---|
| `개요 N` 스타일 문단 | `#` × N 헤딩 |
| 굵게 / 기울임 / 취소선 / 밑줄 / 첨자 | `**굵게**` / `*기울임*` / `~~취소선~~` / `<u>` / `<sup>`·`<sub>` |
| 하이퍼링크(%hlk) | `[표시텍스트](URL)` |
| 이미지 | `![image](<media>/imageN.png)` (확장자는 매직 바이트로 판별) |
| 표(병합 없음) | GFM 파이프 표 |
| 표(병합 셀·중첩 표·블록 수식) | HTML `<table>`(colspan/rowspan) |
| 글머리표 / 번호 문단 | `- ` / `N. ` 목록(번호 형식은 numbering 정의에서 합성) |
| 각주 / 미주 | `[^N]` / `[^eN]` GFM 풋노트 |
| 수식(eqed) | `$스크립트$` / `$$스크립트$$` (HWP 수식 스크립트 원문, LaTeX 아님) |
| 머리말/꼬리말·숨은 설명 | 기본 제외, `--with-header-footer` / `--with-hidden`으로 포함 |

**표 직렬화 보장**: HTML 표의 각 `<tr>...</tr>`과 GFM 파이프 표의 각 행은 항상 한 줄로 직렬화된다
(셀 안 중첩 표도 그 줄에 인라인으로 얹힌다). 소비자가 행 단위로 안정적으로 인용·파싱할 수 있도록
테스트로 고정돼 있다.

한계: 헤딩 인식은 `개요 N` 스타일뿐이고, 떠 있는 개체의 위치·z-order는 반영하지 않으며, 역방향
(md → hwp)은 표·굵게 등 기본 구문만 복원된다.

### `--with-segments` (추출 근거 좌표)

`ahwp cat <file> --format markdown --with-segments`는 markdown과 함께 각 출력 문자 범위가 어느 원본
문단에서 왔는지를 한 줄 JSON 봉투로 낸다.

```json
{"markdown": "...", "segments": [
  {"kind": "para", "section": 0, "para": 12, "start": 345, "end": 512}
]}
```

- **오프셋은 유니코드 스칼라(문자) 단위** Python `str` 인덱싱과 같다. `markdown[start:end]`로 그대로 슬라이스된다.
- **좌표는 IR 인덱스** `section`/`para`는 `--format json`의 `sections[]`/`paragraphs[]` 인덱스라 재디코드에도 안정적이다.
- **정렬·비중첩** 세그먼트는 `start` 오름차순이며 겹치지 않는다. 어느 문단에도 귀속되지 않는 출력은 간극으로 남는다.
- 표가 만든 줄은 표를 담은 문단 인덱스를 상속하고, 각주/미주 정의는 참조 문단에 귀속된다.
- `markdown` 필드는 `--with-segments` 없이 실행한 출력과 바이트 단위로 동일하다.
