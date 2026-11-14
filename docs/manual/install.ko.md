[한국어](install.ko.md) · [English](install.md)

# 설치와 업데이트

`ahwp`를 설치하는 모든 방법, 렌더링에 필요한 폰트 준비, 업데이트와 되돌리기를 다룬다. README에는 짧은 경로만 있고, 서버리스·컨테이너 빌드를 포함한 상세는 이 페이지에 있다.

## ahwp 설치하기

명령 이름은 `ahwp`다. 2.0에서 1.x의 `hwp` 호환 이름을 제거했으므로 스크립트와 MCP 클라이언트 설정의 `hwp`를 `ahwp`로 바꾼다.

### 한 줄 스크립트 (macOS / Linux)

Rust 툴체인 없이 릴리스 바이너리를 설치한다. 설치 후에는 `ahwp update`로 자체 갱신된다.

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh | sh
```

기본 위치는 `~/.local/bin`이다(PATH에 없으면 안내를 출력한다). 위치·버전은 인자나 환경변수로 바꾼다:

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh \
  | sh -s -- --dir /usr/local/bin --tag v1.6.0
HWP_INSTALL_DIR=~/bin sh scripts/install.sh
```

아카이브는 `.sha256` 자산과 대조한 뒤에만 설치한다.

### Homebrew (macOS / Linux)

저장소 자체가 tap이다(별도 `homebrew-*` 저장소 없음).

```sh
brew tap staixbwlb/hwp https://github.com/STAIxBWLB/agent-hwp
brew install ahwp
ahwp --version
```

업그레이드는 `brew update && brew upgrade ahwp`. 예전 `hwp` 포뮬러가 남은 기기는 다음
`brew update` 때 `formula_renames.json`으로 `ahwp`로 이전되며, `brew install ahwp`로도 옮길 수 있다. 지원 플랫폼은 macOS(Apple Silicon·Intel)와 Linux x86_64다.

### 사전 빌드 바이너리

각 [릴리스](https://github.com/STAIxBWLB/agent-hwp/releases)에 플랫폼별 아카이브와 `.sha256`이 붙는다.

| 플랫폼 | 아카이브 |
|---|---|
| Linux x86_64 | `ahwp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `ahwp-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `ahwp-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `ahwp-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `ahwp-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

압축을 풀어 `ahwp`를 PATH에 둔다(검증: `shasum -a 256 -c ahwp-*.sha256`).

### 서버리스·컨테이너 (Vercel, AWS Lambda, Docker)

Linux 아카이브는 **glibc 2.17** 기준으로 크로스 빌드한다. 그래서 Amazon Linux 2·2023, Debian 8+,
RHEL/CentOS 7+ 및 그 이후 배포판에서 그대로 돌아간다. 빌드 러너보다 glibc가 낮은 Vercel Node
런타임과 AWS Lambda도 포함된다.

바이너리를 커밋하지 말고 빌드 단계에서 받는다. 그러면 갱신할 것은 고정한 태그 하나뿐이다.

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh \
  | sh -s -- --tag v1.6.0 --dir ./bin
```

플랫폼이 배포 번들을 수집하기 전에 바이너리가 있어야 하므로(Vercel `includeFiles`, Next.js
`outputFileTracingIncludes`, Docker 레이어) 요청 시점이 아니라 빌드 커맨드나 `prebuild`
스크립트에서 실행한다. 폰트는 동봉되지 않으니 렌더 계열 명령에는 폰트를 함께 올리고
`--font-dir`(또는 `HWP_FONT_DIR`)로 지정한다.

### 소스 빌드

```sh
git clone git@github.com:STAIxBWLB/agent-hwp.git && cd agent-hwp
cargo build --release
cargo install --path crates/hwp-cli   # `ahwp` 바이너리 설치
```

Rust edition 2024, `rust-version = 1.93` 이상이 필요하다.

### 폰트

**저장소에 폰트를 동봉하지 않는다**(`fonts/`는 gitignore). 텍스트 추출·변환은 폰트 없이 동작하고,
렌더·PDF·hwp 바이너리 쓰기(미리보기 이미지)에만 CJK 글리프가 필요하다.

| 사용처 | 폰트 지정 |
|---|---|
| `render` / `diff` / `mcp` | `--font-dir <dir>`(반복 지정 가능) |
| `convert` / 테스트 | 환경변수 `HWP_FONT_DIR`(미설정 시 프로젝트 `fonts/`) |

구조 코퍼스 게이트는 시스템 폰트를 쓰지 않고 해시로 고정된 OFL 폰트만 쓴다. 폰트 바이트는 커밋하지
않으며 `scripts/fetch-corpus-fonts.sh`가 manifest의 고정 URL에서 받아 SHA-256으로 검증한다.

### 업데이트

```sh
ahwp update            # 최신 릴리스로 자체 교체 (체크섬 대조 후 원자적 교체)
ahwp update --check    # 교체 없이 현재/최신 버전만 확인
ahwp update --tag v1.6.2   # 특정 버전으로 되돌리기(v1.6.0 이상)
```

설치 방식을 스스로 판별한다. 한 줄 스크립트·릴리스 아카이브·`cargo install`로 설치했으면 실행 중인
바이너리를 제자리 교체하고(sha256 대조 → 같은 디렉터리 임시 파일 → rename, 실패 시 원본 복원),
Homebrew(Cellar) 설치본은 `brew upgrade ahwp`에 위임한다(brew는 버전 고정이 안 되므로 `--tag`는 거부).
