[한국어](22-remote-mcp-deployment.ko.md) · [English](22-remote-mcp-deployment.md)

# Remote MCP deployment 설계

> **상태:** 설계 전용. HTTP adapter, container image, Worker, cloud runtime 중 어느 것도 아직
> 존재하지 않는다. 출시된 제품은 여전히 stdio 기반 `hwp mcp`뿐이다.

[Doc 20](20-remote-mcp.ko.md)은 Remote MCP service가 *무엇을* 보장해야 하는지를 정의한다.
endpoint 계약, 인증, 격리, limit, release gate가 거기에 해당한다. 이 문서는 그것을 *어떻게*
만들고 *어디에서* 실행하는지를 정의한다. runtime dependency를 선택하고(doc 20 §8 gate),
platform에 중립적인 Rust HTTP adapter 하나를 규정하며, 그 adapter를 공유하는 deployment tier
두 가지를 규정한다.

[Issue #52](https://github.com/STAIxBWLB/hwp-cli/issues/52)는 remote service의 consumer, 담당자,
OAuth 정책, 예산이 명명되지 않았다는 이유로 deferred 상태로 닫혔으며, 그 조건이 갖추어지면 범위를
좁힌 새 delivery issue를 열라고 요구한다. 이 문서가 그 조건을 제공하며, 이 문서와 함께 등록하는
delivery issue가 activation record가 된다.

## 1. 결정 요약

| 질문 | 결정 |
|---|---|
| 문서 처리는 어디에서 실행하는가 | container 안에서 동작하는 native `hwp serve` HTTP mode. `hwp mcp`를 shell-out하는 bridge도 아니고 wasm도 아니다 |
| HTTP dependency (doc 20 §8) | 동기 방식 `tiny_http`. workspace의 no-tokio, no-SDK 기조를 유지한다 |
| service는 어디에서 실행하는가 | 하나의 binary를 공유하는 두 tier. **Tier A**는 Cloudflare Workers + Containers, **Tier B**는 Amazon Quick Suite connector 뒤의 AWS Bedrock AgentCore |
| token은 누가 발행하는가 | Tier A는 `@cloudflare/workers-oauth-provider`를 사용해 Worker 자신이 발행하며 Google은 upstream IdP다. Tier B는 Amazon Cognito가 발행하며 Google은 federated IdP다 |
| 첫 구현의 file 전송 방식 | doc 20 §3.2의 artifact model이 아니라 session workspace. §7에 amendment로 기록한다 |

tier의 구현 순서는 이 문서에서 고정하지 않는다. 공유 Rust 작업이 완료되면 두 tier는 같은
binary로부터 각각 독립적으로 배포할 수 있다.

## 2. Platform 평가

세 가지 hosting model을 요구사항에 대조해 비교했다. service는 사용자를 인증하고, token을
발행하며, Google 로그인을 지원해야 하고, filesystem과 font file을 필요로 하는 native Rust
binary를 실행해야 한다.

| 기준 | Cloudflare Workers + Containers | AWS AgentCore + Quick | Vercel Functions |
|---|---|---|---|
| native binary 실행 | Worker가 호출하는 Containers(microVM) | container 자체가 배포 단위 | function에 binary를 동봉하거나 beta Rust runtime 사용 |
| MCP client용 OAuth authorization server | first-party로 제공. `workers-oauth-provider`가 `/authorize`, `/token`, RFC 7591 dynamic client registration을 구현한다 | Cognito가 JWT를 발행하지만 dynamic client registration이 없어서 client를 관리자가 등록한다 | first-party 제공이 없어서 외부 IdP를 붙이거나 직접 구현해야 한다 |
| session affinity | Durable Object와 Container class로 직접 조립한다 | 관리형으로 제공한다. runtime이 `Mcp-Session-Id`를 주입하고 session을 전용 microVM으로 라우팅한다 | 제공하지 않는다. instance를 지정할 수 없다 |
| `/mcp` 외의 추가 HTTP route | 사용할 수 있으므로 file upload sideband를 둘 수 있다 | 사용할 수 없다. `0.0.0.0:8000/mcp`가 계약의 전부다 | 사용할 수 있다 |
| CPU architecture | amd64 | **arm64 필수** | amd64 |
| doc 20과의 정합성 | backlog 1번과 3번을 진전시킨다 | 여기에 더해 doc 20 §1이 지목한 Quick pilot, 즉 backlog 4번에 도달한다 | auth server가 없다는 점을 제외하면 Cloudflare와 같다 |
| 비용 구조 | Workers Paid 구독료와 container 사용량 | 구독료 없이 vCPU와 memory를 초 단위로 과금 | Pro seat 구독료와 사용량 |

**Vercel은 채택하지 않는다.** first-party authorization server가 없어서 가입과 token 발행을
third-party identity service에 의존해야 하고, instance affinity가 없어서 doc 20 §3.2의 완전한
artifact model이 후속 phase가 아니라 선결 조건이 된다. 다른 platform이 두 문제를 모두 없애 주는
상황에서 이 비용을 치를 이유가 없다.

**남은 두 platform은 모두 규정한다.** 서로 다른 consumer를 담당하기 때문이다. Tier A는 누구나
Google 계정으로 가입하고 임의의 MCP client로 접속하는 공개 service를 담당한다. Tier B는 관리자가
Amazon Quick Suite 안에 connector 하나를 등록하는 조직을 담당하며, 이것이 doc 20 §1이 처음부터
지목한 consumer다. Rust 작업은 두 tier에서 동일하고, adapter의 설정과 주변 cloud resource만
다르다.

## 3. 공유 Rust 작업

### 3.1 Protocol core 추출

HTTP 작업을 시작하기 전에 `crates/hwp-cli/src/commands/mcp.rs`를 module directory로 분리한다.
doc 20 §3.1이 요구하는 사항이다.

| 파일 | 내용 |
|---|---|
| `commands/mcp/mod.rs` | protocol core. `handle_request`, protocol negotiation, tool registry, schema, dispatch |
| `commands/mcp/authority.rs` | `FileAuthority` trait와 `LocalFsContext`. 기존 canonical root 검사와 font directory 처리 |
| `commands/mcp/stdio.rs` | 기존 newline framing loop. zeroizing read buffer와 password scrubbing 포함 |
| `commands/mcp/http.rs` | `hwp serve` adapter |

**doc 20 §3.1에 대한 amendment.** doc 20은 "library-visible module"을 요구한다. 그런데 protocol
core는 binary-private command module 16개를 약 70개 지점에서 참조하므로, 이를 library로 올리면
현재 binary 밖에 consumer가 하나도 없는 상태에서 `commands/`의 대부분을 `hwp_cli` library로
옮기게 된다. 따라서 첫 구현은 core를 binary 내부에 두고, 같은 `hwp` binary에 컴파일되는 두
adapter가 이를 공유하도록 한다. doc 20 자신도 현재 함수들을 "starting seams, not the final
public API"라고 서술하고 있다. library로의 승격은 binary 밖의 consumer가 실제로 생길 때 수행한다.

stdio adapter는 현재 동작을 byte 단위로 그대로 유지한다. tool이 정확히 20개임을 확인하는 기존
stdio process test가 이 분리 작업 전체의 regression gate 역할을 한다.

### 3.2 `hwp serve` 계약

새 subcommand가 protocol core를 HTTP로 노출한다. 이 server는 private hop이다. 두 tier 모두에서
신뢰된 edge가 TLS를 종단하고, 호출자를 인증하고, origin을 검증하며, request body 크기를 제한한
뒤에야 요청이 이 server에 도달한다.

| Route | 동작 |
|---|---|
| `POST /mcp` | body를 1 MiB로 제한하며, 이는 stdio의 `MAX_REQUEST_LINE_BYTES`와 같은 값이다. request는 `200 application/json`을 반환하고, notification은 protocol 출력이 없으므로 빈 body와 함께 `202`를 반환한다 |
| `GET /mcp` | `405`. server가 push하지 않으므로 SSE stream을 제공하지 않는다 |
| `GET /healthz` | listener가 bind되면 `200`. container platform이 readiness를 확인하는 지점이다 |
| `POST /files/{name}` | `--files`로만 활성화한다. upload를 workspace root로 streaming한다 |
| `GET /files/{name}` | `--files`로만 활성화한다. workspace file을 streaming으로 반환한다 |

flag는 `--addr`(기본값 `0.0.0.0:8080`, Tier B는 `0.0.0.0:8000`), `--root`(**필수**. root가 없으면
경고만 하는 stdio와 다르다), `--font-dir`, `--files`다.

adapter가 강제하는 규칙은 다음과 같다.

- `--root` 없이는 기동을 거부한다. remote deployment는 filesystem 권한이 제한되지 않은 상태로
  실행되지 않는다.
- 들어오는 `Mcp-Session-Id`는 받아들이되 무시한다. Tier B의 platform이 이 header를 주입하며,
  session affinity는 document server가 아니라 platform의 책임이다.
- `/files` route에서는 `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`에 일치하는 이름만 받고, root 아래로
  해석하며, 파일 하나를 64 MiB로 workspace 전체를 256 MiB로 제한하고, 제한을 넘으면 부분 업로드
  파일을 삭제한다.
- 요청을 한 번에 하나씩 처리한다. container 하나가 MCP session 하나를 담당하므로 악용할 동시성도
  없고 framing이 뒤섞일 여지도 없다.

### 3.3 Remote-safe inline content

Tier B는 `/mcp` 외의 route를 노출하지 않으므로 문서가 JSON-RPC 안으로 이동해야 한다. 이를
도구 두 개가 담당한다. `hwp_put_file {name, content}`은 base64 콘텐츠를 세션 워크스페이스의
파일로 쓰고, `hwp_get_file {path}`은 워크스페이스 파일을 base64로 반환한다. 상한은 복호 기준
512 KiB이며, 인코딩하면 약 699 KB라서 1 MiB인 줄 상한 안에 JSON-RPC 봉투가 들어갈 여유가 남는다.

**이 절은 원래 기존 tool schema에 inline mode를 추가하는 방식을 규정했으나, 구현 단계에서
기각되었다.** 도구별 inline은 17개 도구에 인자 약 30개를 더하고, 각 인자를 그 도구의 알 수 없는
인자 배열에도 넣어야 하며, 비밀번호 범위 도구 6개에서는 `take_scoped_password` 허용 목록에도
추가해야 한다. 여기에 crate에서 가장 안전이 중요한 파일 안에 발행하지 않고 스테이징만 하는 경로가
필요했다. 그러고도 `hwp_split`·`hwp_certify`·다중 페이지 `hwp_render`·`hwp_convert --media-dir`의
디렉터리 및 다중 파일 출력은 표현하지 못해 모두 제외해야 했다. 도구 두 개는 파일 하나로 끝나고,
기존 20개 스키마를 한 바이트도 바꾸지 않으며, 모든 경로 인자를 균일하게 다룬다. doc 20 §3.2가
열거하는 이미지·폰트·정책·부품 경로까지 포함되는데, 문서 입출력만 다루는 도구별 범위였다면
이것들은 빠졌을 것이다. §7이 이미 이 형태를 규정하고 있다. 도구는 계속 경로 인자를 받고, 경로는
하나의 사설 워크스페이스 안의 상대 이름이다.

이 도구들은 Tier A의 공백도 함께 메운다. `/files`는 `Mcp-Session-Id`를 실은 별도 HTTP 요청이
필요한 경로이고 MCP 클라이언트는 그런 요청을 보낼 수 없다. 따라서 이 도구들이 생기기 전까지
배포된 서비스는 텍스트에서 문서를 만들 수는 있어도 기존 문서를 받을 수는 없었다.

우회하지 말고 명시해 둘 제약이 둘 있다. 512 KiB는 이미지가 많은 문서에는 작다. Tier A의
`/files`는 64 MiB까지 받지만 Tier B에는 대응물이 없다. 그리고 받아 온 문서의 base64는 클라이언트
메시지 스트림에 그대로 쌓이며 대부분의 클라이언트가 이를 모델에 넘긴다. 상한을 올리려면 분할
전송이 필요하고, 분할 전송은 곧 doc 20 §3.2의 artifact model이다. tenant 소유 upload, immutable
output, 보존 기간, object store 기반 signed download URL을 포함하는 그 모델은 두 tier 모두에서
후속 phase로 남으며, 여기서 즉석으로 만들 프로토콜이 아니다.

## 4. Dependency 결정 기록

이 절은 async runtime이나 MCP SDK를 부수적 선택으로 추가하는 것을 금지하는 doc 20 §8의 decision
gate를 충족한다.

### 4.1 선택지

**선택지 1: tokio와 axum을 쓰는 Rust MCP SDK.** 완전한 Streamable HTTP 적합성, SSE, session
처리를 이미 구현되고 검증된 상태로 얻는다. 반대 근거는 이렇다. 이 architecture에서는 edge가 이미
인증, session, origin 검증, body limit을 담당하고 SSE stream도 제공하지 않으므로, SDK가 주는
것의 대부분이 중복이거나 사용되지 않는다. 또한 tokio를 의도적으로 배제해 온 workspace에 tokio를
들이게 되고, 20개 tool을 SDK API에 맞춰 다시 등록해야 하며, compile time과 binary 크기와 MSRV
부담이 늘어난다. doc 20 §3.1은 동기 문서 작업을 async I/O executor 위에서 실행하지 말라고
경고하는데, 이를 지키려면 전반에 걸쳐 `spawn_blocking` 규율이 필요해진다.

**선택지 2: SDK 없이 async HTTP stack만 사용.** 선택지 1의 tokio 비용은 그대로 치르면서
hand-written protocol core를 유지하므로, 선택지 1의 적합성 이점을 전혀 얻지 못한다.

**선택지 3: `tiny_http` 기반 동기 server.** 유지보수되는 작은 dependency 하나를 thread 모델로
사용하며 async runtime이 없다. 기존 protocol core를 그대로 재사용하고, 블로킹 문서 작업이 request
thread 위에서 자연스럽게 실행된다.

### 4.2 결정

**선택지 3을 채택한다.** doc 20 §8은 적합성, cancellation, streaming, 보안 test가 안전하지 않은
framework를 재발명하지 않고도 통과하는 경우에만 no-tokio 기조 유지를 허용한다. 이 조건은 노력이
아니라 범위 축소로 충족된다. container 내부의 HTTP 표면은 route가 최대 다섯 개인 private hop이고
신뢰된 edge를 통하지 않으면 도달할 수 없으므로, 적대적 입력 parsing, TLS, slow client 방어,
origin 검증은 edge의 책임이다. streaming은 명시적으로 제공하지 않는다. cancellation은 edge의
deadline과 뒤이은 container 종료로 처리한다.

`TcpListener`를 직접 parsing하는 대신 `tiny_http`를 고른 이유는, doc 20 §8이 custom parsing보다
유지보수되는 HTTP primitive를 선호하기 때문이다.

**재검토 조건.** client가 SSE나 resumable stream을 요구하거나, 하나의 process가 여러 session을
동시에 담당해야 하는 상황이 오면 이 결정은 선택지 1로 뒤집히며, 새 기록이 이 절을 대체한다.

## 5. Tier A: Cloudflare 공개 service

```text
MCP client
   |  HTTPS
   v
Worker (TypeScript)
   |  workers-oauth-provider: /authorize, /token, /register (dynamic client registration)
   |  Google upstream IdP. 최초 login 시 user record 생성
   |  personal access token middleware, body limit, origin 검사, audit
   v
Durable Object HwpSession. 이름은 principal과 session 식별자에서 파생
   |  Container class: idle sleep, 최대 수명 alarm, deadline, egress 차단
   v
microVM: hwp serve --addr 0.0.0.0:8080 --root /work --files --font-dir <fonts>
```

### 5.1 가입, 로그인, token 발행

MCP client가 상대하는 OAuth authorization server는 Worker 자신이고, Google은 upstream identity
provider일 뿐이다. 이것이 MCP `2025-06-18` specification의 third-party authorization flow다.
client는 Google token을 받지 않고 이 service가 발행한 token만 받는다.

1. 인증 없이 `POST /mcp`를 보내면 `401`과 `WWW-Authenticate` challenge를 받는다.
2. client가 protected-resource metadata와 authorization-server metadata를 읽고 `/register`에서
   스스로 등록한다. dynamic client registration이 있으므로 관리자가 미리 준비할 것이 없다.
3. client가 browser로 `/authorize`를 연다. Worker가 동의 화면을 표시한 뒤 `openid email profile`
   scope로 Google에 redirect한다.
4. Google이 `/callback`으로 돌아온다. Worker가 code를 교환하고 identity claim을 읽은 뒤,
   **`sub` claim이 처음 보는 값이면 user record를 생성한다. 최초 Google 로그인이 곧 가입이다.**
   이어서 `mcp:tools` scope로 authorization을 완료한다.
5. client가 PKCE로 `/token`에서 code를 교환해 이 service의 access token을 받는다. grant와 token은
   KV namespace에 hash 형태로 저장하며, provider library는 이 binding 이름이 `OAUTH_KV`일 것을
   요구한다.

OAuth flow 대신 고정 header로 설정하는 client를 위해 dashboard가 personal access token을
발행한다. token은 `hwp_pat_` 뒤에 32 byte 난수를 base64url로 붙인 형태이며, 생성 시 한 번만
보여 주고 저장은 SHA-256 hash로만 한다. OAuth provider 앞의 middleware가 이 prefix를 인식해
hash를 조회하고 동등한 identity로 같은 MCP handler를 호출하므로, 이후 authorization 경로는
정확히 하나다.

### 5.2 데이터 모델

```sql
CREATE TABLE users (
  id            TEXT PRIMARY KEY,
  google_sub    TEXT NOT NULL UNIQUE,
  email         TEXT NOT NULL,
  name          TEXT,
  created_at    INTEGER NOT NULL,
  last_login_at INTEGER
);

CREATE TABLE pats (
  id           TEXT PRIMARY KEY,
  user_id      TEXT NOT NULL REFERENCES users(id),
  token_hash   TEXT NOT NULL UNIQUE,
  label        TEXT,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER,
  revoked_at   INTEGER
);

CREATE TABLE audit (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  ts          INTEGER NOT NULL,
  request_id  TEXT,
  user_id     TEXT,
  session_ref TEXT,
  tool        TEXT,
  outcome     TEXT NOT NULL,
  duration_ms INTEGER,
  bytes_in    INTEGER,
  bytes_out   INTEGER
);

CREATE INDEX idx_pats_user ON pats(user_id);
CREATE INDEX idx_audit_ts ON audit(ts);
```

identity key는 email 주소가 아니라 Google `sub` claim이다. email 주소는 다른 사람에게 재배정될
수 있기 때문이다. `session_ref`에는 session 식별자 자체가 아니라 그 hash를 저장하며, audit table은
tool argument, path, 문서 내용, token을 담지 않는다. doc 20 §7의 요구사항이다.

### 5.3 Session 수명 주기

| 사건 | 처리 방식 |
|---|---|
| `initialize` | Worker가 난수 session 식별자를 만들고 principal과 그 식별자로 Durable Object 이름을 정한다. container가 기동해 readiness 확인을 통과하면 response에 `Mcp-Session-Id`를 실어 보낸다 |
| 이후 호출 | 같은 header가 같은 object에 도달하므로 같은 microVM, 같은 workspace, 같은 process를 사용한다 |
| 30분 유휴 | container class가 instance를 정지시킨다. object가 session을 dead로 표시하므로 다음 호출은 `404`가 되고 client가 재초기화한다 |
| `DELETE /mcp` | container를 종료하고 session을 dead로 표시하며 alarm을 해제한 뒤 `204`를 반환한다. 멱등하다 |
| 최대 수명 8시간 | alarm이 session을 종료시켜 재초기화를 강제한다. doc 20 §7의 요구사항이다 |
| deadline 초과 | 기본 120초, rendering과 conversion과 certification은 300초다. Worker가 요청을 중단하고 container를 종료한 뒤 timeout을 반환한다 |
| crash 또는 eviction | process 종료가 container를 멈추고 object가 session을 dead로 표시하므로 이후 호출은 `404`가 된다 |
| workspace 정리 | teardown이 보장한다. workspace는 영속되지 않는 container 로컬 disk이므로 회수할 잔여물이 남지 않는다 |

**다른 principal의 접근은 구조적으로 차단된다.** object 이름이 인증된 principal에서 파생되므로,
남의 session 식별자를 제시하면 한 번도 초기화된 적 없는 다른 object로 해석되고, 응답은 식별자의
소유자에 대해 아무것도 알려 주지 않는 평범한 `404`가 된다.

### 5.4 Resource 구성

`deploy/cloudflare/` project가 Worker source, wrangler 설정, D1 schema, container 정의를 담는다.
설정은 container class와 그 Durable Object migration, `OAUTH_KV` namespace, D1 database,
dashboard 정적 asset을 binding한다. secret은 Google client id와 secret, 그리고 cookie 서명
key다.

container image는 `serve`가 릴리스에 포함되기 전까지 repository source에서 빌드한다. `serve`를
담은 릴리스가 나온 뒤에는 checksum을 검증한 릴리스 tarball을 slim base에 내려받는 방식으로
전환한다. 그러면 배포 경로에서 Rust toolchain이 빠지고 cold start가 짧아진다. font는 배포판의
Nanum package를 사용하며, 이는 CI가 이미 쓰고 있는 font 기준선과 일치한다.

## 6. Tier B: Amazon Quick connector 뒤의 AgentCore

```text
Amazon Quick Suite
   |  관리자가 MCP integration 하나를 등록하고, 각 사용자가 개별적으로 인가한다
   v
Amazon Cognito user pool: 셀프서비스 가입, Google federation, JWT 발행
   v
AgentCore Runtime: JWT를 검증하고 Mcp-Session-Id를 주입 및 라우팅하며
   session마다 전용 microVM을 제공한다
   v
arm64 container: hwp serve --addr 0.0.0.0:8000 --root /work --font-dir <fonts>
```

### 6.1 Platform 계약

AgentCore Runtime은 container가 **arm64** image로 `0.0.0.0:8000/mcp`에서 Streamable HTTP를
제공할 것을 요구하며, `Mcp-Session-Id`를 스스로 관리한다. 요청에 해당 header가 없으면 추가하고,
session을 그 전용 microVM으로 라우팅한다. 따라서 server는 platform이 부여한 session 식별자를
거부하지 않고 수용해야 하는데, 이는 §3.2가 이미 요구하는 사항이다.

여기에서 두 가지가 따라 나온다. 첫째, Tier A가 Durable Object와 container class로 직접 조립하는
격리 계층을 platform이 제공하므로 Tier B에는 자체 edge code가 필요 없다. 둘째, runtime이 MCP
endpoint만 노출하므로 `/files` sideband가 존재할 수 없고, §3.3의 inline content mode가 선택이
아니라 선결 조건이 된다.

### 6.2 가입, 로그인, token 발행

Cognito가 Tier A에서 Worker가 담당하던 세 요구사항을 그대로 담당한다. user pool이 셀프서비스
가입을 제공하고, Google을 federated identity provider로 추가하면 사용자가 Google 계정으로
로그인하며, pool이 발행한 JWT를 AgentCore의 inbound authorizer가 검증한다.

Cognito는 dynamic client registration을 구현하지 않으므로, Quick 관리자가 authorization
endpoint와 token endpoint, client credential을 직접 입력해 connector를 등록한다. Amazon Quick
Suite는 바로 이 방식을 지원한다. integration 콘솔은 dynamic registration을 제공하는 server를
받거나 endpoint와 credential 값을 명시적으로 받으며, three-legged OAuth를 사용하므로 Quick이
사용자를 대신해 tool을 호출하기 전에 각 사용자가 자신의 identity로 connector를 인가한다.
dynamic registration이 없어도 무방한 이유는 여기에서 등록이 관리 행위이기 때문이다. 반면 Tier
A의 셀프서비스 client에는 dynamic registration이 필요하다.

### 6.3 추가 작업

- 새 release target `aarch64-unknown-linux-gnu`와 arm64 container image.
- image repository, AgentCore runtime 설정, log 전달 구성.
- region을 확정하기 전에 확인할 사항. 의도한 region에서 AgentCore와 Quick MCP integration이 모두
  제공되는지, 그리고 runtime의 현재 유휴 및 최대 session 수명이 doc 20 §7의 값과 어떻게
  대비되는지 확인한다.

## 7. File 권한 모델

두 tier의 첫 구현은 doc 20 §3.2의 artifact model 대신 **session workspace**를 사용한다. Tier A는
`/files` route로, Tier B는 tool argument의 inline content로 workspace를 채운다. tool은 계속 path
argument를 받지만, 그 path는 하나의 private workspace 안의 상대 이름이다.

**doc 20 §10에 대한 amendment.** doc 20은 "local path schemas remain remotely writable" 상태의
릴리스를 실패로 규정한다. 이 기준이 겨냥하는 것은 client의 path argument가 다른 tenant의 데이터나
host filesystem에 도달할 수 있는 공유 server다. 그런데 여기의 두 tier에서 path는 다른 tenant의
데이터를 담지 않고 network egress도 없으며 session이 끝나면 파기되는 단일 session microVM 안에서
해석되고, process 내부의 기존 canonicalize 및 containment 검사도 그대로 동작한다. 따라서 이
tier들에 한해 기준을 다음과 같이 수정한다. 절대 경로와 traversal은 여전히 거부하며, workspace는
결코 공유하거나 재사용하지 않는다.

이 amendment가 미루는 것, 그래서 후속 phase가 반드시 제공해야 하는 것은 §3.2의 나머지다. object
store를 기반으로 하는 두 번째 `FileAuthority` 구현, tenant가 소유하는 불투명한 artifact 식별자,
보존 기간을 갖는 immutable output, 인증된 endpoint나 단기 signed URL을 통한 download가 여기에
해당한다. §3.2를 문면 그대로 충족하는 것은 그 phase뿐이다.

## 8. doc 20 §10 기준 보안 상태

첫 구현이 충족하는 항목과 각 항목을 강제하는 지점은 다음과 같다.

| 기준 | 강제 지점 |
|---|---|
| stdio가 기본으로 유지되고 tool이 정확히 20개로 노출된다 | 기존 stdio process test가 core 분리를 gate한다 |
| 인증이 tool 실행보다 먼저 수행된다 | Tier A는 OAuth provider와 token middleware가 handler 이전에 거부한다. Tier B는 runtime의 JWT authorizer가 container 이전에 거부한다 |
| session이 principal에 묶이고 다른 principal의 접근은 닫힌 채 실패한다 | Tier A는 object 이름이 principal에서 파생되므로 남의 session 식별자는 정보를 주지 않는 `404`가 된다. Tier B는 platform이 session을 인가된 호출자 범위로 한정한다 |
| request, upload, workspace, deadline limit이 결정적 정리와 함께 강제된다 | edge에서 parsing 이전에 body를 거부하고 `hwp serve`에서 한 번 더 거부한다. 파일과 workspace 상한은 adapter가, deadline과 종료는 edge가 담당한다 |
| tenant끼리 서로의 데이터를 읽거나 덮어쓸 수 없다 | session마다 microVM이 하나씩이고 공유 filesystem이 없으며, process 내부의 기존 root containment가 함께 동작한다 |
| 종료가 부분 산출물이나 재사용 가능한 권한을 남기지 않는다 | workspace가 microVM과 함께 파기되고 session 식별자는 dead로 표시된다 |
| log에 token, 문서 내용, path가 남지 않는다 | audit schema가 metadata만 저장하고 token은 hash로만 보관한다 |
| edge를 우회해 backend에 도달할 수 없다 | 두 runtime 모두 container를 공개하지 않으며, `hwp serve`는 root 없이는 기동조차 거부한다 |
| 문서 worker에 network egress가 없다 | Tier A는 container의 egress를 비활성화하고, Tier B는 runtime의 network 설정으로 처리한다 |
| proxy 뒤에 놓인 무제한 `hwp mcp`가 아니다 | shell-out이 없다. native adapter가 protocol core를 공유하며 단일 session microVM 안에서 제한된 상태로 동작한다 |

미루는 항목은 다음과 같으며, 첫 구현을 완성본으로 오해하지 않도록 여기에 명시한다. §3.2의 완전한
artifact model, SSE와 resumable stream, 요청 단위 cancellation(현재는 deadline이 session 자체를
끝낸다), session당 2개 동시 job(§7은 2개를 허용하지만 server는 순차 처리한다), `mcp:tools`보다
세분화된 scope, 비 browser client를 위해 origin 부재를 허용하는 대신 적용할 엄격한 origin
allowlist, 그리고 §9의 5번 항목이 일반 공개 전에 요구하는 독립 검토가 여기에 해당한다.

## 9. 운영

**구현 중 확정해야 할 배포 위험.** MCP client가 `GET /mcp`의 `405`를 수용하는지 확인해야 한다.
transport specification은 push stream을 제공하지 않는 server에 대해 이를 허용하지만, SSE 작업을
검토하기 전에 실제 대상 client로 확인해야 한다. `initialize` 시점의 container cold start는 slim
릴리스 tarball image로 줄인다. Google OAuth application은 검증을 받기 전까지 소수의 test 사용자로
제한되며, Cognito에도 자체 quota가 있다. Cloudflare containers의 설정 표면은 아직 변하고 있으므로
field 이름과 instance type은 이 문서를 믿지 말고 구현 시점에 확인한다. 배포 단계에서 Rust를
빌드하면 build limit을 넘을 수 있으며, 그럴 경우 CI에서 image를 빌드하고 registry에서 참조한다.

**비용 구조.** Tier A는 Workers Paid 구독이며 포함된 container 할당량이 경량 사용을 감당하고 그
이상은 memory와 vCPU로 과금된다. Tier B는 구독료가 없고 vCPU와 memory를 초 단위로 과금하며,
Cognito는 월간 활성 사용자 기준 이하에서는 무료다. 파일럿 규모에서는 어느 쪽도 비싸지 않지만,
공개적으로 알리기 전에 두 경우 모두 지출 경보를 설정해야 한다.

**릴리스 연동.** image가 릴리스 binary를 설치하는 방식으로 바뀐 뒤에는, service가 반영해야 하는
hwp 릴리스마다 version과 checksum을 올리고 재배포해야 한다. 이 과정의 CI 자동화는 배포 자체가
안정될 때까지 의도적으로 미룬다.

## 10. Issue #52의 activation requirement

Issue #52는 deferred 상태로 닫히면서, 구현을 시작하기 전에 명명하고 수용해야 할 항목 일곱 가지를
제시한다. 이 문서는 그 항목들에 다음과 같이 답한다.

| 요구 항목 | 답변 |
|---|---|
| 구체적 web consumer와 그 MCP protocol version | Tier A는 `2025-06-18` Streamable HTTP를 사용하는 임의의 MCP client이며, 현재 server가 이미 협상하는 version이다. Tier B는 doc 20 §1이 지목한 Amazon Quick Suite다 |
| 배포 및 보안 담당자 | 파일럿을 넘는 트래픽이 생기기 전까지는 repository owner가 담당한다. 착수 시점에 확정한다 |
| OAuth issuer, audience, scope, authorization 정책 | Tier A는 Worker가 issuer이고 그 MCP endpoint가 audience이며 scope는 `mcp:tools`, upstream identity provider는 Google이다(§5.1). Tier B는 Cognito user pool이 issuer이고 AgentCore runtime이 audience를 검증한다(§6.2) |
| Tenant와 session의 identity 및 영속성 모델 | principal은 검증된 identity claim에서 파생하며 key는 Google `sub` claim이다. session은 그 principal에 묶인 server 발행 식별자다. 사용자와 token은 관계형 저장소에 영속하고, session workspace는 전혀 영속하지 않는다(§5.2, §5.3) |
| Upload와 output limit, 보존, 삭제, 남용 통제 | §3.2와 doc 20 §7의 상한을 적용한다. workspace가 microVM과 함께 사라지므로 보존 기간은 구조적으로 0이다. rate limit은 §8에 미루는 항목으로 명시했으며, 공개 announce 전에 반드시 도입해야 한다 |
| Hosting 대상, rate limit, 예산, 모니터링 | hosting 대상과 비용 구조는 §2와 §9에 있다. 공개 announce 전에 지출 경보가 필요하다. 예산 상한 수치와 service level objective는 남은 미결 항목이며 doc 20 §9의 6번으로 미룬다 |
| dependency 최소화 및 no-SDK 불변식의 실용성 여부 | 유지한다. §4가 비교 과정을 기록하며, 작은 동기 HTTP dependency 하나만 추가해 불변식을 지킨다 |

따라서 작성 시점 기준으로 두 항목이 미결로 남는다. repository owner를 넘어서는 담당자 지정과,
예산 상한 수치 및 그 모니터링 목표가 그것이다. 두 항목 모두 설계 결정이 아니라 운영 결정이며,
§3의 공유 Rust 작업을 막지 않는다.
