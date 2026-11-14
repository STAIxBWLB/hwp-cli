[한국어](commands.ko.md) · [English](commands.md)

# 명령 한눈에 보기

`ahwp`의 모든 명령을 한 화면에 요약했다. 명령을 고르는 사람이나 작업을 명령에 대응시키는 에이전트를 위한 페이지다. 플래그 단위 전체 레퍼런스는 자동 생성된 [cli-reference.ko.md](cli-reference.ko.md)다.

## 명령 찾아보기

전체 자동 생성 레퍼런스는 [docs/manual/cli-reference.ko.md](cli-reference.ko.md)(영문:
[cli-reference.md](cli-reference.md))에 있다. clap 정의에서 두 언어 모두 생성되며
코드와의 동기화를 CI 테스트가 강제한다. 아래는 요약이다.

도움말 표시 언어는 로케일을 따르고(한국어 로케일이면 한국어, 그 외 영문), `--lang en|ko`나
`HWP_LANG`으로 바꿀 수 있다.

| 명령 | 설명 |
|---|---|
| `info <file>` | 포맷/버전/속성/스트림 진단 |
| `cat <file>` | 본문 추출(plain/markdown/json/html/csv). `--with-segments`로 추출 근거 좌표 동반 |
| `grep <pattern> <file>` | 문단 텍스트를 grep 방식으로 검색(일치하는 것이 없으면 0이 아닌 종료 코드) |
| `convert <input> -o <output>` | 포맷 변환. `--to`로 `docx`·`odt`·`csv`·`txt` 출력도 지정할 수 있다. 출력이 `.pdf`면 렌더 경로로 위임. `--strict`는 보존 불가 데이터 발견 시 게시하지 않고 실패 |
| `render <input> -o <output>` | 페이지를 PNG/SVG(페이지별 파일)·PDF(단일 멀티페이지)로 렌더 |
| `merge <inputs...> -o <output>` | 문서 두 개 이상을 인자 순서대로 합침(입력 하나가 Section 하나). `--strict`·`--loss-report`는 `convert`와 같음 |
| `split <input> --out-dir <dir>` | 기본은 Section 하나당 조각 하나. `--pages`는 한글이 저장한 배치 캐시에서 추정한 쪽 범위로 나눔 |
| `new -o <output>` | markdown, JSON IR, 또는 내장 공문서 템플릿 8종(`--template`, `--list-templates`)에서 새 문서 생성 |
| `compose <spec> -o <output>` | DocumentSpec v1/v2를 결정론적으로 합성. `--dry-run`으로 쓰지 않고 검증 |
| `template <template> --data <data> -o <output>` | TemplateSpec/Data v1의 typed AST를 bounded expansion |
| `edit <input> -o <output>` | 텍스트·서식·구조 편집(문단, 표 행/열, 셀 병합/분할, 필드, 이미지, 도장) |
| `fields` / `bookmarks` / `slots` `<file>` | 필드·누름틀 / 책갈피 / `{{name}}` 슬롯 목록 |
| `styles <file>` | 이름 있는 스타일 목록 (해석된 속성, `used_by` 문단 세그먼트 id; `--format json`은 `hwp-styles-v1`) |
| `fill <input> -o <output>` | 충실도 보존 템플릿 채우기(hwpx 패키지 보존) |
| `validate <file>` | 구조 검증(mimetype·필수 엔트리·XML 파싱) |
| `lint <file>` | 공문서 표기법·구조 규칙 10개를 검사. 기본은 권고이며, `--strict`는 오류 등급이 있을 때 1로 종료하고 `--json`은 `hwp-lint-report-v1`을 출력 |
| `certify <input> --policy <file> --report <dir>` | 인증 리포트를 원자적으로 게시. [Certification v1](../design/16-certification-v1.ko.md) |
| `corpus --manifest <file> --report <dir>` | 고정 구조 코퍼스 게이트. [코퍼스 계약](../design/17-structured-corpus-v1.ko.md) |
| `diff <input> --ref <png>` | 렌더 결과를 한글 기준 PNG와 비교(잉크·오프셋·픽셀 오차·MAE) |
| `compare <a> <b>` | 문서 두 개의 문단·구조 차이를 보고하며 두 입력 모두 수정하지 않음. 종료 코드는 diff(1) 관례(0 동일, 1 차이 발견, 2 실행 실패) |
| `mcp` | MCP stdio 서버 실행 |
| `serve` | 같은 MCP 도구를 컨테이너 배포용 HTTP(`POST /mcp`)로 제공. [원격 MCP 배포](../design/22-remote-mcp-deployment.ko.md) 참고 |
| `skill export` | Amazon Quick 프로필 자동 탐색을 포함한 번들 에이전트 스킬 내보내기/설치 |
| `update` | 자체 업데이트(brew 설치본은 `brew upgrade`에 위임) |
| `dump <file>` | [개발자용] 레코드/패키지 구조 덤프 |

출력 포맷은 대부분 출력 파일 확장자에서 추론된다. `convert`/`render`는 `--to`/`--format`으로 명시할 수도 있다.

## 자주 쓰는 명령 실행하기

```sh
# 진단: 포맷/버전/속성/스트림
ahwp info report.hwp

# 본문 추출
ahwp cat report.hwp                       # plain text
ahwp cat report.hwp --format markdown     # markdown
ahwp cat report.hwp --format json         # 전체 IR(JSON)
ahwp cat report.hwp --format csv          # 표를 CSV로

# 본문 검색 (grep 방식. 일치하는 것이 없으면 1로 종료)
ahwp grep "예산" report.hwp

# 변환 (출력 확장자로 포맷 추론)
ahwp convert report.hwp   -o report.hwpx  # hwp → hwpx (표·이미지·머리말 보존)
ahwp convert report.hwpx  -o report.hwp   # hwpx → hwp 바이너리
ahwp convert report.hwp   -o report.md    # 이미지는 report.media/에 추출
ahwp convert report.hwp   -o doc.json --embed-bin   # 이미지까지 임베드한 자급식 JSON

# 렌더링
ahwp render report.hwp -o page.png --dpi 150
ahwp render report.hwp -o report.pdf --font-dir ./fonts   # 단일 멀티페이지 PDF(검색 가능)

# 새 문서 생성
ahwp new -o out.hwpx --from notes.md
ahwp new -o out.hwp  --from doc.json

# 구조 문서 합성
ahwp compose spec.yaml -o report.hwpx --report
ahwp template report-template.yaml --data report-data.json -o report.hwpx

# 공문서
ahwp new --list-templates                              # 내장 템플릿 8종
ahwp new -o gian.hwpx --template gian-internal         # 기안문. 두문·결문 포함
ahwp slots gian.hwpx                                   # 아직 채우지 않은 {{슬롯}} 확인
ahwp fill gian.hwpx -o final.hwpx --set "제목=예산 집행 계획"
ahwp lint final.hwpx --strict                          # 표기법·구조 규칙 검사

# 편집 (이미지·서식·미해석 레코드 보존)
ahwp fields form.hwp                        # 채울 수 있는 필드/누름틀 이름 확인
ahwp edit form.hwp -o filled.hwp \
    --replace "초안=>최종" \
    --set-cell "0:1:2=12,300원" \
    --set-field "수신처=홍길동" --verify

# 구조 편집 (문단 삽입/삭제·표 행/열·셀 병합/분할)
ahwp edit report.hwp -o out.hwp \
    --insert-para "개요=>추가 설명 문단입니다." \
    --add-row "0" --add-col "1" --merge-cells "0:1:1:2:2" --verify

# 렌더 충실도 비교 (한글 기준 PNG와 잉크/오프셋/픽셀 오차)
ahwp diff report.hwp --ref hancom_p1.png --page 1 --dpi 150 --font-dir ./fonts

# MCP stdio 서버
ahwp mcp --font-dir ./fonts
```
