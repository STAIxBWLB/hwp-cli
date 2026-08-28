//! CLI 도움말 다국어.
//!
//! 기본은 영문이고, 로케일이 한국어면 한국어로 표시한다. `--lang <en|ko>`나 `HWP_LANG`으로
//! 명시 지정하면 로케일보다 우선한다.
//!
//! 영문은 [`crate::cli`]의 doc comment(= clap help)가 정본이고, 한국어는 이 모듈의 오버레이
//! 표가 정본이다. 표 항목이 없으면 조용히 영문으로 남으므로, `tests/cli_reference.rs`가
//! **모든 명령·인자에 한국어 항목이 있는지**를 게이트로 강제한다.

use clap::Command;

/// 도움말 표시 언어.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Ko,
}

impl Lang {
    /// `en`·`ko` 및 `ko_KR.UTF-8` 같은 로케일 문자열을 받는다. 모르는 값은 None.
    pub fn parse(value: &str) -> Option<Self> {
        let head = value
            .split(['.', '_', '-', '@'])
            .next()
            .unwrap_or(value)
            .to_ascii_lowercase();
        match head.as_str() {
            "ko" | "kor" | "korean" => Some(Self::Ko),
            "en" | "eng" | "english" | "c" | "posix" => Some(Self::En),
            _ => None,
        }
    }

    /// 실제 프로세스 환경에서 표시 언어를 정한다.
    ///
    /// 우선순위: `--lang <값>`(또는 `--lang=<값>`) → `HWP_LANG` → `LC_ALL` → `LC_MESSAGES`
    /// → `LANG` → 영문. clap이 도움말을 출력하기 *전에* 정해야 하므로 argv를 직접 훑는다.
    pub fn detect() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self::resolve(&args, |key| std::env::var(key).ok())
    }

    /// [`Lang::detect`]의 순수 함수 형태(테스트용).
    pub fn resolve<F>(args: &[String], env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let value = if let Some(rest) = arg.strip_prefix("--lang=") {
                Some(rest.to_string())
            } else if arg == "--lang" {
                iter.next().cloned()
            } else {
                None
            };
            // 잘못된 값은 여기서 조용히 넘기고 clap이 정식 오류를 내게 둔다.
            if let Some(parsed) = value.as_deref().and_then(Self::parse) {
                return parsed;
            }
        }
        for key in ["HWP_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Some(parsed) = env(key).as_deref().and_then(Self::parse) {
                return parsed;
            }
        }
        Self::En
    }
}

/// 명령 트리의 도움말을 해당 언어로 바꾼다. 영문이면 그대로 둔다.
pub fn localize(command: Command, lang: Lang) -> Command {
    match lang {
        Lang::En => command,
        Lang::Ko => translate(command, ""),
    }
}

/// `path` is the subcommand name (the root is the empty string). Nested subcommands
/// (`export` under `skill`, etc.) are also keyed by the bare name itself, regardless
/// of depth (e.g. `("export", …)`).
fn translate(command: Command, path: &str) -> Command {
    let command = match lookup(path, "") {
        Some(text) => command.about(text),
        None => command,
    };
    let command = command.mut_args(|arg| {
        let id = arg.get_id().as_str().to_string();
        match lookup(path, &id) {
            Some(text) => arg.help(text),
            None => arg,
        }
    });
    command.mut_subcommands(|sub| {
        let name = sub.get_name().to_string();
        translate(sub, &name)
    })
}

fn lookup(path: &str, arg: &str) -> Option<&'static str> {
    KO.iter()
        .find(|(cmd, id, _)| *cmd == path && *id == arg)
        .map(|(_, _, text)| *text)
}

/// 한국어 도움말 오버레이: `(서브커맨드, 인자 id, 텍스트)`.
/// 서브커맨드가 빈 문자열이면 루트, 인자 id가 빈 문자열이면 그 명령의 about이다.
pub const KO: &[(&str, &str, &str)] = &[
    ("", "", "HWP/HWPX 문서 처리 도구"),
    (
        "",
        "lang",
        "도움말 표시 언어 (기본: 로케일, 없으면 영문). HWP_LANG로도 지정 가능",
    ),
    // info
    ("info", "", "파일 정보 표시: 포맷/버전/속성/스트림 목록"),
    ("info", "file", "대상 HWP/HWPX 파일"),
    ("info", "json", "JSON으로 출력"),
    // cat
    ("cat", "", "텍스트 추출"),
    ("cat", "file", "대상 HWP/HWPX 파일"),
    ("cat", "format", "출력 포맷"),
    ("cat", "password", "명령줄에서 직접 입력할 암호"),
    (
        "cat",
        "password_stdin",
        "표준 입력에서 UTF-8 암호 한 줄 읽기",
    ),
    ("cat", "preview", "본문 파싱 없이 PrvText 미리보기만 출력"),
    (
        "cat",
        "with_header_footer",
        "머리말/꼬리말 텍스트도 추출에 포함 (기본: 제외)",
    ),
    (
        "cat",
        "with_hidden",
        "숨은 설명 텍스트도 추출에 포함 (기본: 제외)",
    ),
    (
        "cat",
        "with_segments",
        "(markdown 전용) markdown과 함께 각 출력 문자 범위의 원본 좌표(섹션/문단)를 한 줄 JSON 봉투로 출력 — {\"markdown\": ..., \"segments\": [...]}",
    ),
    // grep
    (
        "grep",
        "",
        "문단 텍스트 검색 (grep 의미 — 일치 없으면 종료 코드 1)",
    ),
    ("grep", "pattern", "검색 패턴 (부분 문자열 일치)"),
    ("grep", "file", "대상 HWP/HWPX 파일"),
    ("grep", "ignore_case", "대소문자 무시 일치"),
    // convert
    ("convert", "", "포맷 변환"),
    ("convert", "password", "명령줄에서 직접 입력할 암호"),
    (
        "convert",
        "password_stdin",
        "표준 입력에서 UTF-8 암호 한 줄 읽기",
    ),
    (
        "convert",
        "inputs",
        "입력 HWP/HWPX 파일들 (\"-\"는 stdin; 여러 입력은 --out-dir 필요)",
    ),
    (
        "convert",
        "output",
        "출력 파일 경로 (\"-\"는 텍스트 포맷(md/json/html/txt/csv)에 한해 stdout; 단일 입력에서 필수)",
    ),
    (
        "convert",
        "out_dir",
        "여러 입력의 출력 디렉터리 (파일명은 \"<스템>.<확장자>\", --to 필요)",
    ),
    ("convert", "to", "출력 포맷 (생략 시 확장자에서 추론)"),
    (
        "convert",
        "strict",
        "변환 중 보존 불가능한(opaque) 데이터 발견 시 실패 처리",
    ),
    (
        "convert",
        "loss_report",
        "typed 보존 ledger(hwp-preservation-report-v1)를 JSON으로 기록 — 무손실 성공 시에도 작성 (단일 입력 전용). 보존 검사는 hwp/hwpx 출력에서만 실행되므로 그 외 포맷(docx, md 등)에서는 항상 빈 ledger",
    ),
    (
        "convert",
        "preserve_layout",
        "줄 배치 캐시 보존 (무수정 왕복 전용 — 한글은 내용과 어긋난 줄 배치를 변조로 판정하므로 기본은 제거)",
    ),
    (
        "convert",
        "embed_bin",
        "JSON 출력 시 첨부 바이너리(이미지)를 base64로 임베드 (자급식 JSON)",
    ),
    (
        "convert",
        "media_dir",
        "(md) 이미지 추출 디렉터리 — 기본 \"<출력스템>.media\". 상대경로는 출력 파일 기준으로 해석하고 링크는 입력한 경로 그대로 쓴다 (예: figs)",
    ),
    (
        "convert",
        "with_header_footer",
        "(md) 머리말/꼬리말 텍스트도 포함 (기본: 제외)",
    ),
    (
        "convert",
        "with_hidden",
        "(md) 숨은 설명 텍스트도 포함 (기본: 제외)",
    ),
    (
        "convert",
        "font_dir",
        "(pdf) 추가 폰트 디렉터리 (반복 가능, 기본: HWP_FONT_DIR 또는 fonts/)",
    ),
    // merge
    (
        "merge",
        "",
        "여러 HWP5/HWPX 입력을 하나로 병합 (입력마다 구역 하나, 인자 순서대로 이어붙임; 쪽/각주/차례 번호는 각 입력의 시작/이어달기 설정을 그대로 유지하므로 병합 후 수동 조정이 필요할 수 있음)",
    ),
    ("merge", "password", "명령줄에서 직접 입력할 암호"),
    (
        "merge",
        "password_stdin",
        "표준 입력에서 UTF-8 암호 한 줄 읽기",
    ),
    (
        "merge",
        "inputs",
        "입력 HWP/HWPX 파일들, 2개 이상, 이어붙이는 순서대로 (표준 입력 \"-\"는 지원하지 않음 — 파일 경로를 직접 지정)",
    ),
    (
        "merge",
        "output",
        "출력 파일 경로 (\".hwp\"는 HWP5, \".hwpx\"는 HWPX로 저장)",
    ),
    (
        "merge",
        "strict",
        "병합 중 보존 불가능한(opaque) 데이터 발견 시 실패 처리",
    ),
    (
        "merge",
        "loss_report",
        "typed 보존 ledger(hwp-preservation-report-v1)를 JSON으로 기록 — 무손실 성공 시에도 작성",
    ),
    // render
    ("render", "", "페이지 렌더링"),
    ("render", "password", "명령줄에서 직접 입력할 암호"),
    (
        "render",
        "password_stdin",
        "표준 입력에서 UTF-8 암호 한 줄 읽기",
    ),
    ("render", "input", "입력 HWP/HWPX 파일"),
    ("render", "output", "출력 파일 경로"),
    ("render", "pages", "페이지 범위: \"1\", \"1-3\", \"all\""),
    ("render", "dpi", "해상도 DPI (유한한 36..=600)"),
    ("render", "format", "출력 포맷 (생략 시 확장자에서 추론)"),
    (
        "render",
        "report",
        "기계 판독 렌더 보고서(JSON)를 원자적으로 기록",
    ),
    ("render", "font_dir", "추가 폰트 디렉터리 (반복 가능)"),
    // new
    ("new", "", "새 문서 생성"),
    ("new", "output", "출력 HWP/HWPX 경로"),
    ("new", "from", "입력 markdown/JSON 파일 (생략 시 빈 문서)"),
    (
        "new",
        "template",
        "내장 문서 템플릿을 영문 슬러그 또는 한국어 별칭으로 사용 (--list-templates 참고). 자체 프로필과, 템플릿의 {{슬롯}}을 기본값으로 갖는 네이티브 두문/결문 프레임을 함께 가져온다. --from과는 함께 쓸 수 없고, --preset과 프레임 플래그는 그 기본값을 하나씩 덮어쓴다",
    ),
    (
        "new",
        "list_templates",
        "내장 문서 템플릿(슬러그·한국어 별칭)을 모두 나열하고 종료; -o 불필요",
    ),
    (
        "new",
        "set_meta",
        "메타데이터 설정 \"키=값\" (키: title|author|subject|keywords, 반복 가능)",
    ),
    (
        "new",
        "preset",
        "공문서 프로필 (markdown 입력 전용): official/report/plan/notice/minutes/press. 기존·한국어 별칭은 하나의 프로필로 정규화",
    ),
    ("new", "margin_top", "위쪽 페이지 여백(mm, 0..=200)"),
    ("new", "margin_bottom", "아래쪽 페이지 여백(mm, 0..=200)"),
    ("new", "margin_left", "왼쪽 페이지 여백(mm, 0..=200)"),
    ("new", "margin_right", "오른쪽 페이지 여백(mm, 0..=200)"),
    (
        "new",
        "strict",
        "markdown import가 내용을 드롭하면(HTML 블록 계약 위반) 실패 처리. 기본: 경고 후 진행 (종료 코드 0)",
    ),
    (
        "new",
        "doc_head",
        "공문서 두문 블록 \"키=값\" (키: 기관명|수신|경유, 반복 가능)",
    ),
    (
        "new",
        "doc_foot",
        "공문서 결문 블록 \"키=값\" (키: 발신명의|기안자|검토자|결재자|협조자|시행번호|시행일자|접수번호|접수일자|주소|홈페이지|전화|팩스|이메일|공개구분|수신자, 반복 가능. 수신자는 두문이 \"수신자 참조\"인 다수 수신 문서의 수신처 목록으로, 지정할 때만 결문 마지막 줄로 나온다)",
    ),
    (
        "new",
        "notice_head",
        "공고문 머리 블록 \"키=값\" (키: 기관명|공고번호, 반복 가능)",
    ),
    (
        "new",
        "notice_foot",
        "공고문 꼬리 블록 \"키=값\" (키: 공고일자|발신명의, 반복 가능)",
    ),
    (
        "new",
        "press_head",
        "보도자료 머리 블록 \"키=값\" (키: 기관명|보도시점|배포일|담당부서|담당자|연락처, 반복 가능)",
    ),
    // compose
    (
        "compose",
        "",
        "DocumentSpec v1/v2(JSON/YAML)에서 구조 문서를 deterministic 합성",
    ),
    (
        "compose",
        "spec",
        "DocumentSpec v1/v2 입력 파일(.json, .yaml, .yml)",
    ),
    ("compose", "output", "출력 HWP/HWPX"),
    (
        "compose",
        "format",
        "입력 포맷 (생략 시 spec 확장자에서 추론)",
    ),
    (
        "compose",
        "dry_run",
        "검증·컴파일 보고서만 생성하고 파일은 쓰지 않음",
    ),
    ("compose", "report", "실행 보고서를 JSON으로 출력"),
    (
        "compose",
        "allow_visual_fallback",
        "[deprecated] v1 호환 전용 — v2는 이 정책 덮어쓰기를 거부한다",
    ),
    // template
    (
        "template",
        "",
        "TemplateSpec/Data v1에서 typed native HWP/HWPX 생성",
    ),
    (
        "template",
        "template",
        "TemplateSpec v1 입력 파일(.json, .yaml, .yml)",
    ),
    (
        "template",
        "data",
        "TemplateData v1 입력 파일(.json, .yaml, .yml)",
    ),
    ("template", "output", "출력 HWP/HWPX"),
    (
        "template",
        "template_format",
        "TemplateSpec 입력 포맷 (생략 시 확장자에서 추론)",
    ),
    (
        "template",
        "data_format",
        "TemplateData 입력 포맷 (생략 시 확장자에서 추론)",
    ),
    (
        "template",
        "dry_run",
        "실제 확장·writer·검증 경로를 실행하되 결과 파일은 게시하지 않음",
    ),
    (
        "template",
        "report",
        "preservation/expansion 보고서를 JSON으로 출력",
    ),
    // diff
    (
        "diff",
        "",
        "렌더 결과를 한글 기준 PNG와 비교해 오차 측정 (위치 오프셋·픽셀 차이율)",
    ),
    ("diff", "input", "입력 HWP/HWPX 파일"),
    (
        "diff",
        "ref",
        "한글에서 같은 페이지를 같은 DPI로 내보낸 기준 PNG",
    ),
    ("diff", "page", "비교할 페이지 (1-기반)"),
    ("diff", "dpi", "해상도 DPI (유한한 36..=600)"),
    (
        "diff",
        "out",
        "차이 이미지 출력 경로 (생략 시 <ref>.diff.png)",
    ),
    ("diff", "font_dir", "추가 폰트 디렉터리 (반복 가능)"),
    (
        "diff",
        "tolerance",
        "채널 차이 허용 오차 (이하면 동일 취급)",
    ),
    (
        "diff",
        "format",
        "리포트 출력 형식 (json = 기계 판독, parity 배치 러너용)",
    ),
    (
        "diff",
        "ours_png",
        "문서 렌더 대신 이 래스터(우리 PDF의 pdftoppm 결과)를 --ref와 비교; 입력 경로는 리포트 기록용",
    ),
    // edit
    (
        "edit",
        "",
        "기존 문서 편집 (텍스트 치환·표 셀 설정) — 이미지·서식 보존",
    ),
    ("edit", "input", "입력 HWP/HWPX 파일"),
    ("edit", "output", "출력 파일 경로"),
    (
        "edit",
        "replace",
        "텍스트 치환 \"찾기=>바꾸기\" (반복 가능, 모든 일치 치환)",
    ),
    (
        "edit",
        "set_cell",
        "표 셀 설정 \"표:행:열=값\" (반복 가능, 0-기반 인덱스)",
    ),
    (
        "edit",
        "set_cell_by_label",
        "양식 레이블의 값 셀 설정 \"레이블=값\" (반복 가능, 정확 일치)",
    ),
    (
        "edit",
        "label_table",
        "모든 --set-cell-by-label 요청을 이 0-기반 재귀 표 인덱스로 제한",
    ),
    (
        "edit",
        "set_field",
        "필드/누름틀 채우기 \"이름=값\" (반복 가능 — hwp fields로 이름 확인)",
    ),
    (
        "edit",
        "set_meta",
        "메타데이터 설정 \"키=값\" (키: title|author|subject|keywords, 반복 가능)",
    ),
    (
        "edit",
        "create_field",
        "누름틀 생성 \"앵커=>이름\" 또는 \"앵커=>이름=값\" — 앵커 텍스트 뒤에 %clk 필드 삽입 (반복 가능)",
    ),
    (
        "edit",
        "create_bookmark",
        "책갈피 생성 \"앵커=>이름\" — 앵커 텍스트 뒤에 bokm 지점 표식 삽입 (반복 가능)",
    ),
    (
        "edit",
        "create_hyperlink",
        "하이퍼링크 생성 \"앵커=>URL\" 또는 \"앵커=>표시=>URL\" — 앵커 뒤에 %hlk 삽입 (반복 가능)",
    ),
    (
        "edit",
        "insert_image",
        "이미지 삽입 \"앵커=>경로\" 또는 \"앵커=>경로@너비x높이\"(mm) — 앵커 뒤에 그림 삽입 (반복 가능)",
    ),
    (
        "edit",
        "seal",
        "도장 날인 \"앵커=>경로\" 또는 \"앵커=>경로@크기mm\" — 앵커 문구 위에 도장 부유 배치 (반복 가능)",
    ),
    (
        "edit",
        "set_format",
        "글자 서식 \"찾기:속성=값,...\" (예: \"제목:bold=on,size=16,color=#FF0000\")",
    ),
    (
        "edit",
        "set_align",
        "문단 정렬 \"찾기=정렬\" (left/right/center/justify/distribute)",
    ),
    (
        "edit",
        "insert_para",
        "문단 삽입 \"앵커=>텍스트\" — 앵커가 있는 문단 뒤에 새 문단 (반복 가능)",
    ),
    (
        "edit",
        "insert_para_before",
        "문단 삽입(앞) \"앵커=>텍스트\" — 앵커가 있는 문단 앞에 새 문단 (반복 가능)",
    ),
    (
        "edit",
        "delete_para",
        "문단 삭제 \"텍스트\" — 텍스트가 있는 문단 삭제 (반복 가능)",
    ),
    (
        "edit",
        "add_row",
        "표 행 추가 \"표[:위치[:개수[:템플릿행]]]\" — 위치 생략·end면 끝, 숫자면 그 행 앞에 삽입 (반복 가능, 0-기반; 병합 표도 지원)",
    ),
    (
        "edit",
        "add_col",
        "표 열 추가 \"표[:위치[:개수]]\" — 위치 생략·end면 끝, 숫자면 그 열 앞에 삽입. 전체 폭 유지(기존 열 균등 축소). 병합 표도 지원 (반복 가능, 0-기반)",
    ),
    (
        "edit",
        "delete_row",
        "표 행 삭제 \"표:행\" — N번째 표의 R행 (반복 가능, 0-기반; 병합 행은 거부)",
    ),
    (
        "edit",
        "delete_col",
        "표 열 삭제 \"표:열\" — N번째 표의 열 삭제. 전체 폭 유지(남은 열에 재분배). 병합 셀은 축소 (반복 가능, 0-기반)",
    ),
    (
        "edit",
        "merge_cells",
        "셀 병합 \"표:r1:c1:r2:c2\" — 사각 영역을 좌상단 앵커로 병합 (반복 가능, 0-기반)",
    ),
    (
        "edit",
        "split_cell",
        "셀 분할 \"표:행:열\" — 병합 셀을 1×1로 분해 (반복 가능, 0-기반)",
    ),
    (
        "edit",
        "add_table",
        "표 삽입 \"앵커=>행JSON\" — 앵커 문단 뒤에 균일 표 삽입. 행JSON은 문자열 배열의 배열 (반복 가능)",
    ),
    (
        "edit",
        "clone_table",
        "표 복제 \"원본표=>앵커[=>blank|keep]\" — N번째 표(0-기반, 재귀 순서)를 깊은 복제해 앵커 문단 뒤에 삽입. blank(기본)는 구조·서식만 남기고 셀 내용을 비우고, keep은 지원 콘텐츠(중첩 표·그림)까지 복제(id 재부여) (반복 가능)",
    ),
    (
        "edit",
        "set_para",
        "문단 모양 \"찾기=>키:값\" — 키: line-spacing(% 또는 Npt), indent, left, right, top, bottom (mm) (반복 가능)",
    ),
    (
        "edit",
        "set_page",
        "페이지 설정 \"키:값\" — 키: width, height, margin-left, margin-right, margin-top, margin-bottom (mm), orientation (portrait|landscape) (반복 가능)",
    ),
    (
        "edit",
        "delete_image",
        "그림 삭제 \"앵커\" — 앵커 문단의 그림 삭제 (반복 가능)",
    ),
    (
        "edit",
        "delete_table",
        "표 삭제 \"n\"(0-기반 인덱스) 또는 \"앵커\"(앵커 문단의 표) (반복 가능)",
    ),
    (
        "edit",
        "delete_field",
        "필드 삭제 \"이름\" (반복 가능; 이름은 hwp fields로 확인)",
    ),
    (
        "edit",
        "delete_bookmark",
        "책갈피 삭제 \"이름\" (반복 가능; 이름은 hwp bookmarks로 확인)",
    ),
    (
        "edit",
        "style_tables",
        "공문서 프리셋으로 모든 적용 대상 표 스타일링(헤더 셰이딩·굵게·가운데 정렬, 내용비례 폭) — official|report|plan|notice|minutes|press. 1열 표(테두리 블록)는 건너뜀. 두 번 적용해도 바이트 동일",
    ),
    ("edit", "verify", "쓰기 후 재읽기로 검증"),
    (
        "edit",
        "allow_partial",
        "일부 요청이 대상을 찾지 못해도 일치한 편집만 게시 (기본: 하나라도 미적용이면 실패)",
    ),
    // fields / bookmarks / slots
    ("fields", "", "필드/누름틀 목록 표시 (이름·종류·값)"),
    ("fields", "file", "대상 HWP/HWPX 파일"),
    ("fields", "json", "JSON으로 출력"),
    ("bookmarks", "", "책갈피 목록 표시 (이름)"),
    ("bookmarks", "file", "대상 HWP/HWPX 파일"),
    ("bookmarks", "json", "JSON으로 출력"),
    (
        "slots",
        "",
        "`{{name}}` 텍스트 자리표시자(템플릿 슬롯) 목록 표시",
    ),
    ("slots", "file", "대상 HWP/HWPX 파일"),
    ("slots", "json", "JSON으로 출력"),
    // fill
    (
        "fill",
        "",
        "충실도 보존 템플릿 채우기 (hwpx의 `{{name}}` 치환, 패키지 보존)",
    ),
    ("fill", "input", "입력 HWPX 템플릿"),
    ("fill", "output", "출력 파일 경로"),
    (
        "fill",
        "set",
        "자리표시자 채우기 \"이름=값\" (반복 가능; `{{이름}}` 치환). \"이름=@부분.md\"이면 `{{이름}}` 앵커 문단을 부분 파일(md+HTML 표 블록, 계약 docs/design/18)로 교체 — 대규모 문서의 부분별 조합. \"@@\"는 리터럴 '@'",
    ),
    (
        "fill",
        "data",
        "이름→값 JSON 객체 파일 (일괄 채우기; \"parts\": {\"이름\": \"경로\"} 부분 파일 교체, \"tables\": [...] 표 행 채우기)",
    ),
    (
        "fill",
        "json",
        "치환 요약을 JSON으로 출력 ({output, replaced, counts})",
    ),
    (
        "fill",
        "allow_partial",
        "일부 요청이 자리를 찾지 못해도 일치한 값만 게시 (기본: 하나라도 미치환이면 실패)",
    ),
    // validate
    (
        "validate",
        "",
        "구조 검증 (mimetype/필수 엔트리/XML 파싱) — 유효하면 종료코드 0",
    ),
    ("validate", "file", "대상 HWP/HWPX 파일"),
    ("validate", "json", "JSON으로 출력"),
    // lint
    (
        "lint",
        "",
        "공문서 표기법·구조 규칙 검사 — 기본은 권고(advisory)이며 항상 종료코드 0. --strict는 오류 심각도 지적이 있을 때만 종료코드 1",
    ),
    (
        "lint",
        "file",
        "검사 대상 .md/.hwp/.hwpx 파일 (\"-\"는 stdin을 markdown으로 읽음)",
    ),
    (
        "lint",
        "profile",
        "린트 프로필: gongmun(기본) 또는 report — v1에서는 같은 규칙 표 사용",
    ),
    ("lint", "json", "hwp-lint-report-v1 JSON 리포트로 출력"),
    (
        "lint",
        "strict",
        "오류 심각도 지적이 있으면 종료 코드 1 (기본: 항상 종료 코드 0)",
    ),
    // certify
    (
        "certify",
        "",
        "versioned policy로 package/semantic/native render/independent import 인증",
    ),
    ("certify", "input", "인증할 HWP/HWPX 입력"),
    ("certify", "policy", "hwp-certification-policy-v1 JSON/YAML"),
    (
        "certify",
        "report",
        "새로 만들 원자적 artifact 디렉터리(기존 경로 거부)",
    ),
    // corpus
    (
        "corpus",
        "",
        "버전 고정 구조 문서 코퍼스를 2회 생성·재개방·native 인증",
    ),
    (
        "corpus",
        "manifest",
        "hwp-structured-corpus-v1 manifest JSON",
    ),
    (
        "corpus",
        "report",
        "새로 만들 원자적 실행 보고서 디렉터리(기존 경로 거부)",
    ),
    // mcp
    (
        "mcp",
        "",
        "MCP(Model Context Protocol) stdio 서버 — AI 에이전트용 도구 인터페이스",
    ),
    (
        "mcp",
        "font_dir",
        "렌더/diff 도구의 기본 폰트 디렉터리 (반복 가능)",
    ),
    (
        "mcp",
        "root",
        "모든 파일 접근을 이 디렉터리 아래로 제한 (반복 가능). 기본: 제한 없음",
    ),
    // update
    (
        "update",
        "",
        "자체 업데이트 — GitHub 릴리스에서 최신 `hwp`를 받아 실행 중인 바이너리를 교체",
    ),
    ("update", "check", "교체 없이 현재/최신 버전만 확인"),
    (
        "update",
        "tag",
        "특정 릴리스로 고정 (예: \"v0.2.0\" — 이전 버전으로 되돌릴 때)",
    ),
    (
        "update",
        "force",
        "같은 버전이어도 다시 받아 교체 (손상된 설치 복구용)",
    ),
    ("update", "json", "JSON으로 출력"),
    // skill (the nested export subcommand is keyed by the bare name "export")
    (
        "skill",
        "",
        "번들된 에이전트 스킬(AI 코딩 어시스턴트용 SKILL.md) 관리",
    ),
    (
        "export",
        "",
        "임베드된 스킬 트리(SKILL.md, SKILL.ko.md, 공문서 안내, references/, templates/)를 디렉터리에 기록 (기본 ./hwp)",
    ),
    (
        "export",
        "output",
        "스킬 트리 출력 디렉터리 (--install과 동시 사용 불가)",
    ),
    (
        "export",
        "install",
        "대신 알려진 에이전트 스킬 디렉터리에 설치",
    ),
    (
        "export",
        "quick_profile",
        "Amazon Quick 프로필 ID 또는 절대 프로필 디렉터리 (Amazon Quick 설치 전용)",
    ),
    // dump
    ("dump", "", "[개발자용] 레코드/패키지 구조 덤프"),
    ("dump", "file", "대상 HWP/HWPX 파일"),
    (
        "dump",
        "stream",
        "대상 스트림/엔트리 (예: \"DocInfo\", \"BodyText/Section0\", \"Contents/header.xml\")",
    ),
    ("dump", "raw", "레코드 페이로드를 hex로 출력"),
    ("dump", "json", "JSON으로 출력"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn explicit_flag_wins_over_environment_and_locale() {
        let env = |key: &str| match key {
            "HWP_LANG" => Some("en".to_string()),
            "LANG" => Some("en_US.UTF-8".to_string()),
            _ => None,
        };
        assert_eq!(
            Lang::resolve(&args(&["hwp", "--lang", "ko", "info", "a.hwp"]), env),
            Lang::Ko
        );
        assert_eq!(
            Lang::resolve(&args(&["hwp", "--lang=ko", "info"]), env),
            Lang::Ko
        );
    }

    #[test]
    fn environment_wins_over_locale_and_locale_wins_over_default() {
        let with = |key: &'static str, value: &'static str| {
            move |k: &str| (k == key).then(|| value.to_string())
        };
        assert_eq!(
            Lang::resolve(&args(&["hwp"]), with("HWP_LANG", "ko")),
            Lang::Ko
        );
        assert_eq!(
            Lang::resolve(&args(&["hwp"]), with("LANG", "ko_KR.UTF-8")),
            Lang::Ko
        );
        assert_eq!(
            Lang::resolve(&args(&["hwp"]), with("LC_ALL", "ko_KR.UTF-8")),
            Lang::Ko
        );
        // C 로케일과 미지원 로케일은 영문으로 떨어진다.
        assert_eq!(Lang::resolve(&args(&["hwp"]), with("LANG", "C")), Lang::En);
        assert_eq!(
            Lang::resolve(&args(&["hwp"]), with("LANG", "fr_FR.UTF-8")),
            Lang::En
        );
        assert_eq!(Lang::resolve(&args(&["hwp"]), |_| None), Lang::En);
    }

    #[test]
    fn unknown_flag_value_falls_through_to_environment() {
        // 잘못된 --lang 값은 여기서 무시하고 clap이 정식 오류를 내게 둔다.
        assert_eq!(
            Lang::resolve(&args(&["hwp", "--lang", "de"]), |k| (k == "LANG")
                .then(|| "ko_KR.UTF-8".to_string())),
            Lang::Ko
        );
    }

    #[test]
    fn korean_overlay_has_no_duplicate_keys() {
        let mut seen = std::collections::BTreeSet::new();
        for (cmd, id, _) in KO {
            assert!(seen.insert((*cmd, *id)), "중복 항목: ({cmd}, {id})");
        }
    }
}
