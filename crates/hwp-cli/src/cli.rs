//! `hwp` CLI 정의 — clap derive 기반 명령/플래그 선언.
//!
//! 이 모듈은 lib 타깃으로 노출된다(`hwp_cli::cli`). bin(`main.rs`)이 파싱·디스패치에
//! 쓰고, `tests/cli_reference.rs`가 `clap::CommandFactory`로 명령 트리를 introspect해
//! `docs/manual/cli-reference.md`를 자동 생성한다(코드-문서 동기화 게이트).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "hwp", version, about = "HWP/HWPX 문서 처리 도구")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
// Edit 변형이 편집 플래그(Vec<String> 다수)로 커서 다른 변형과 크기차가 크다.
// CLI 명령 enum은 시작 시 한 번만 파싱되므로 크기차는 무의미 — 박싱 대신 허용.
#[allow(clippy::large_enum_variant)]
pub enum Cmd {
    /// 파일 정보 표시: 포맷/버전/속성/스트림 목록
    Info {
        file: PathBuf,
        /// JSON으로 출력
        #[arg(long)]
        json: bool,
    },

    /// 텍스트 추출
    Cat {
        file: PathBuf,
        #[arg(long, value_enum, default_value = "plain")]
        format: TextFormat,
        /// 본문 파싱 없이 PrvText 미리보기만 출력
        #[arg(long)]
        preview: bool,
        /// 머리말/꼬리말 텍스트도 추출에 포함 (기본: 제외)
        #[arg(long = "with-header-footer")]
        with_header_footer: bool,
        /// 숨은 설명 텍스트도 추출에 포함 (기본: 제외)
        #[arg(long = "with-hidden")]
        with_hidden: bool,
        /// (markdown 전용) markdown과 함께 각 출력 문자 범위의 원본 좌표(섹션/문단)를
        /// 한 줄 JSON 봉투로 출력 — {"markdown": ..., "segments": [...]}
        #[arg(long = "with-segments")]
        with_segments: bool,
    },

    /// 포맷 변환
    Convert {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// 출력 포맷 (생략 시 확장자에서 추론)
        #[arg(long, value_enum)]
        to: Option<ConvertFormat>,
        /// 변환 중 보존 불가능한(opaque) 데이터 발견 시 실패 처리
        #[arg(long)]
        strict: bool,
        /// 줄 배치 캐시 보존 (무수정 왕복 전용 — 한글은 내용과 어긋난
        /// 줄 배치를 변조로 판정하므로 기본은 제거)
        #[arg(long)]
        preserve_layout: bool,
        /// JSON 출력 시 첨부 바이너리(이미지)를 base64로 임베드 (자급식 JSON)
        #[arg(long)]
        embed_bin: bool,
        /// (md) 이미지 추출 디렉터리 — 기본 "<출력스템>.media". 상대경로는 출력
        /// 파일 기준으로 해석하고 링크는 입력한 경로 그대로 쓴다 (예: figs)
        #[arg(long)]
        media_dir: Option<PathBuf>,
        /// (md) 머리말/꼬리말 텍스트도 포함 (기본: 제외)
        #[arg(long = "with-header-footer")]
        with_header_footer: bool,
        /// (md) 숨은 설명 텍스트도 포함 (기본: 제외)
        #[arg(long = "with-hidden")]
        with_hidden: bool,
        /// (pdf) 추가 폰트 디렉터리 (반복 가능, 기본: HWP_FONT_DIR 또는 fonts/)
        #[arg(long)]
        font_dir: Vec<PathBuf>,
    },

    /// 페이지 렌더링
    Render {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// 페이지 범위: "1", "1-3", "all"
        #[arg(long, default_value = "all")]
        pages: String,
        #[arg(long, default_value_t = 96.0)]
        dpi: f64,
        /// 출력 포맷 (생략 시 확장자에서 추론)
        #[arg(long, value_enum)]
        format: Option<RenderFormat>,
        /// 추가 폰트 디렉터리 (반복 가능)
        #[arg(long)]
        font_dir: Vec<PathBuf>,
    },

    /// 새 문서 생성
    New {
        #[arg(short, long)]
        output: PathBuf,
        /// 입력 markdown/JSON 파일 (생략 시 빈 문서)
        #[arg(long)]
        from: Option<PathBuf>,
        /// 메타데이터 설정 "키=값" (키: title|author|subject|keywords, 반복 가능)
        #[arg(long = "set-meta")]
        set_meta: Vec<String>,
    },

    /// 렌더 결과를 한글 기준 PNG와 비교해 오차 측정 (위치 오프셋·픽셀 차이율)
    Diff {
        input: PathBuf,
        /// 한글에서 같은 페이지를 같은 DPI로 내보낸 기준 PNG
        #[arg(long)]
        r#ref: PathBuf,
        /// 비교할 페이지 (1-기반)
        #[arg(long, default_value_t = 1)]
        page: usize,
        #[arg(long, default_value_t = 96.0)]
        dpi: f64,
        /// 차이 이미지 출력 경로 (생략 시 <ref>.diff.png)
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// 추가 폰트 디렉터리 (반복 가능)
        #[arg(long)]
        font_dir: Vec<PathBuf>,
        /// 채널 차이 허용 오차 (이하면 동일 취급)
        #[arg(long, default_value_t = 16)]
        tolerance: u8,
    },

    /// 기존 문서 편집 (텍스트 치환·표 셀 설정) — 이미지·서식 보존
    Edit {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// 텍스트 치환 "찾기=>바꾸기" (반복 가능, 모든 일치 치환)
        #[arg(long)]
        replace: Vec<String>,
        /// 표 셀 설정 "표:행:열=값" (반복 가능, 0-기반 인덱스)
        #[arg(long = "set-cell")]
        set_cell: Vec<String>,
        /// 필드/누름틀 채우기 "이름=값" (반복 가능 — hwp fields로 이름 확인)
        #[arg(long = "set-field")]
        set_field: Vec<String>,
        /// 메타데이터 설정 "키=값" (키: title|author|subject|keywords, 반복 가능)
        #[arg(long = "set-meta")]
        set_meta: Vec<String>,
        /// 누름틀 생성 "앵커=>이름" 또는 "앵커=>이름=값" — 앵커 텍스트 뒤에 %clk 필드 삽입 (반복 가능)
        #[arg(long = "create-field")]
        create_field: Vec<String>,
        /// 책갈피 생성 "앵커=>이름" — 앵커 텍스트 뒤에 bokm 지점 표식 삽입 (반복 가능)
        #[arg(long = "create-bookmark")]
        create_bookmark: Vec<String>,
        /// 하이퍼링크 생성 "앵커=>URL" 또는 "앵커=>표시=>URL" — 앵커 뒤에 %hlk 삽입 (반복 가능)
        #[arg(long = "create-hyperlink")]
        create_hyperlink: Vec<String>,
        /// 이미지 삽입 "앵커=>경로" 또는 "앵커=>경로@너비x높이"(mm) — 앵커 뒤에 그림 삽입 (반복 가능)
        #[arg(long = "insert-image")]
        insert_image: Vec<String>,
        /// 도장 날인 "앵커=>경로" 또는 "앵커=>경로@크기mm" — 앵커 문구 위에 도장 부유 배치 (반복 가능)
        #[arg(long = "seal")]
        seal: Vec<String>,
        /// 글자 서식 "찾기:속성=값,…" (예: "제목:bold=on,size=16,color=#FF0000")
        #[arg(long = "set-format")]
        set_format: Vec<String>,
        /// 문단 정렬 "찾기=정렬" (left/right/center/justify/distribute)
        #[arg(long = "set-align")]
        set_align: Vec<String>,
        /// 문단 삽입 "앵커=>텍스트" — 앵커가 있는 문단 뒤에 새 문단 (반복 가능)
        #[arg(long = "insert-para")]
        insert_para: Vec<String>,
        /// 문단 삽입(앞) "앵커=>텍스트" — 앵커가 있는 문단 앞에 새 문단 (반복 가능)
        #[arg(long = "insert-para-before")]
        insert_para_before: Vec<String>,
        /// 문단 삭제 "텍스트" — 텍스트가 있는 문단 삭제 (반복 가능)
        #[arg(long = "delete-para")]
        delete_para: Vec<String>,
        /// 표 행 추가 "표" — N번째 표 끝에 빈 행 (반복 가능, 0-기반; 병합 셀이 있는 표는 거부)
        #[arg(long = "add-row")]
        add_row: Vec<String>,
        /// 표 열 추가 "표"(끝에) 또는 "표:위치"(삽입) — 전체 폭 유지(기존 열 균등 축소). 병합 셀 표도 지원 (반복 가능, 0-기반)
        #[arg(long = "add-col")]
        add_col: Vec<String>,
        /// 표 행 삭제 "표:행" — N번째 표의 R행 (반복 가능, 0-기반; 병합 행은 거부)
        #[arg(long = "delete-row")]
        delete_row: Vec<String>,
        /// 표 열 삭제 "표:열" — N번째 표의 열 삭제. 전체 폭 유지(남은 열에 재분배). 병합 셀은 축소 (반복 가능, 0-기반)
        #[arg(long = "delete-col")]
        delete_col: Vec<String>,
        /// 셀 병합 "표:r1:c1:r2:c2" — 사각 영역을 좌상단 앵커로 병합 (반복 가능, 0-기반)
        #[arg(long = "merge-cells")]
        merge_cells: Vec<String>,
        /// 셀 분할 "표:행:열" — 병합 셀을 1×1로 분해 (반복 가능, 0-기반)
        #[arg(long = "split-cell")]
        split_cell: Vec<String>,
        /// 쓰기 후 재읽기로 검증
        #[arg(long)]
        verify: bool,
    },

    /// 필드/누름틀 목록 표시 (이름·종류·값)
    Fields {
        file: PathBuf,
        /// JSON으로 출력
        #[arg(long)]
        json: bool,
    },

    /// 책갈피 목록 표시 (이름)
    Bookmarks {
        file: PathBuf,
        /// JSON으로 출력
        #[arg(long)]
        json: bool,
    },

    /// `{{name}}` 텍스트 자리표시자(템플릿 슬롯) 목록 표시
    Slots {
        file: PathBuf,
        /// JSON으로 출력
        #[arg(long)]
        json: bool,
    },

    /// 충실도 보존 템플릿 채우기 (hwpx의 `{{name}}` 치환, 패키지 보존)
    Fill {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// 자리표시자 채우기 "이름=값" (반복 가능; `{{이름}}` 치환)
        #[arg(long)]
        set: Vec<String>,
        /// 이름→값 JSON 객체 파일 (일괄 채우기)
        #[arg(long)]
        data: Option<PathBuf>,
        /// 치환 요약을 JSON으로 출력 ({output, replaced, counts})
        #[arg(long)]
        json: bool,
    },

    /// 구조 검증 (mimetype/필수 엔트리/XML 파싱) — 유효하면 종료코드 0
    Validate {
        file: PathBuf,
        /// JSON으로 출력
        #[arg(long)]
        json: bool,
    },

    /// MCP(Model Context Protocol) stdio 서버 — AI 에이전트용 도구 인터페이스
    Mcp {
        /// 렌더/diff 도구의 기본 폰트 디렉터리 (반복 가능)
        #[arg(long)]
        font_dir: Vec<PathBuf>,
    },

    /// 자체 업데이트 — GitHub 릴리스에서 최신 `hwp`를 받아 실행 중인 바이너리를 교체
    Update {
        /// 교체 없이 현재/최신 버전만 확인
        #[arg(long)]
        check: bool,
        /// 특정 릴리스로 고정 (예: "v0.2.0" — 이전 버전으로 되돌릴 때)
        #[arg(long)]
        tag: Option<String>,
        /// 같은 버전이어도 다시 받아 교체 (손상된 설치 복구용)
        #[arg(long)]
        force: bool,
        /// JSON으로 출력
        #[arg(long)]
        json: bool,
    },

    /// [개발자용] 레코드/패키지 구조 덤프
    Dump {
        file: PathBuf,
        /// 대상 스트림/엔트리 (예: "DocInfo", "BodyText/Section0", "Contents/header.xml")
        #[arg(long)]
        stream: Option<String>,
        /// 레코드 페이로드를 hex로 출력
        #[arg(long)]
        raw: bool,
        /// JSON으로 출력
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum TextFormat {
    Plain,
    Markdown,
    Json,
    Html,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ConvertFormat {
    Hwp,
    Hwpx,
    Md,
    Json,
    Html,
    Pdf,
    Odt,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RenderFormat {
    Png,
    Svg,
    Pdf,
}
