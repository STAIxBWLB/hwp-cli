[한국어](README.ko.md) · [English](README.md)

# 부분(part) 조합 예제 — 템플릿 + 부분 채우기

대규모 문서(사업보고서·결과보고서)를 부분별로 나누어 작성하고 조합하는 워크플로.
본문 산문은 markdown, 표·그림 등 비산문 블록은 HTML fragment(계약:
[docs/design/18](../../docs/design/18-html-fragment-contract.ko.md))로 작성한다.

## 파일

- `template.md` — 뼈대 문서. `{{이름}}`만 담긴 문단이 부분 앵커다.
- `part-overview.md` — markdown 부분(산문).
- `part-table.md` — md + HTML 표 부분(병합 셀 포함).
- `part-diagram.md` + `diagram.svg` — SVG 다이어그램 부분(래스터화 임베드).

## 조합

```bash
# 1) 템플릿을 hwpx로 만든다
hwp new --from template.md -o template.hwpx

# 2) 부분을 앵커에 이식한다 (필드 치환과 부분 교체를 한 번에)
hwp fill template.hwpx -o result.hwpx \
  --set 작성일=2026-08-01 \
  --set 개요=@part-overview.md \
  --set 실적표=@part-table.md \
  --set 구성도=@part-diagram.md

# 3) 검증
hwp validate result.hwpx
```

## 규칙

- 앵커 문단은 `{{이름}}`만 담겨 있어야 한다. 문장 중간의 `{{이름}}`은 블록 교체가
  성립하지 않으므로 필드 치환(평문 값)으로만 처리된다.
- 템플릿과 부분이 함께 쓰는 문자/문단 모양 팔레트는 hwp-cli 기본 계열이어야 한다
  (`hwp new`·`compose` 산출물). 부분은 템플릿의 타이포그래피를 상속한다.
- 부분 안 이미지의 상대 경로는 부분 파일 위치가 기준이다. SVG는 검증+결정론적 PNG
  래스터화로 임베드된다(스크립트·외부 참조가 있는 SVG는 거부).
