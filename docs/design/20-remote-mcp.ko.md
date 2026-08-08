[한국어](20-remote-mcp.ko.md) · [English](20-remote-mcp.md)

# Remote MCP transport 설계

> **상태:** 설계 전용. 현재 릴리스는 stdio 기반 `hwp mcp`만 구현한다. HTTP listener,
> OAuth resource server, hosted workspace, artifact service는 포함하지 않는다.

이 문서는 로컬 프로세스를 실행할 수 없는 Web client를 위한 향후 Remote MCP service를 정의한다.
첫 concrete consumer는 Amazon Quick Web이다. 구현 추적은
[issue #52](https://github.com/STAIxBWLB/hwp-cli/issues/52)에서 계속한다.

## 1. 범위와 비범위

### 1.1 범위

- `hwp mcp` stdio를 기본 local transport로 유지하고 16개 tool 동작의 호환성을 보존.
- hosted deployment용 Streamable HTTP mode를 명시적 별도 설정으로 추가.
- 두 adapter가 하나의 transport-independent JSON-RPC/tool dispatch core를 공유.
- 모든 remote request 인증 및 tenant, principal, session별 workspace 격리.
- client-local path 권한을 upload content와 server-owned artifact reference로 대체.
- runtime 구현 전에 보안, limit, observability, cleanup, release gate 정의.

### 1.2 비범위

- 이번 변경에서는 HTTP, OAuth/OIDC, hosting, Quick Web 등록을 구현하지 않음.
- local stdio 명령을 reverse proxy로 직접 공개하지 않음.
- remote client에 임의 server filesystem 접근 권한이나 absolute server path를 허용하지 않음.
- 범용 file store, 장기 문서 보관소, collaborative editor를 만들지 않음.
- async runtime, web framework, MCP SDK, cloud, identity provider를 이 문서에서 선택하지 않음.

## 2. 호환성 기준선

첫 구현의 기준은 현재 stdio server가 이미 협상하는 MCP `2025-06-18`이다. Streamable HTTP는
하나의 MCP endpoint와 HTTP `POST`, `GET`, `DELETE`를 사용한다. Protocol 요구사항은 공식
[2025-06-18 transport specification](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)과
[authorization specification](https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization)을
기준으로 한다.

Protocol은 계속 변하고 있다. 구현 전 Quick Web이 허용하는 protocol version을 다시 확인한다.
후속 version에서 protocol-level session이 제거되었다면 versioned adapter로 구현하며,
`2025-06-18` 계약에 후속 session semantics를 섞지 않는다.

링크된 specification의 내용은 licensing compliance를 위해 재서술했다.

### 2.1 Endpoint 계약

MCP endpoint는 하나의 configurable path이며 기본값은 `POST|GET|DELETE /mcp`다.

| Method | 용도 | 성공 response |
|---|---|---|
| `POST` | JSON-RPC message 1개 전송 | 단일 response는 `application/json`, streaming 필요 시 `text/event-stream` |
| `GET` | established session의 optional server-to-client SSE stream 개설 | `text/event-stream` |
| `DELETE` | session 명시적 종료 및 workspace cleanup 예약 | body 없는 성공 response |

규칙:

- Client는 `POST`에 `Accept: application/json, text/event-stream`을 보내고 JSON request body는
  `Content-Type: application/json`을 사용.
- 초기화 뒤 `MCP-Protocol-Version` 검증. 누락, 미지원, 상충 version은 tool dispatch 전에 실패.
- `2025-06-18` stateful profile에서는 초기화 시 opaque하고 cryptographically random한
  `Mcp-Session-Id` 생성. 이후 `POST`, `GET`, `DELETE`에 같은 값을 전송. Session ID 자체는
  authorization으로 인정하지 않음.
- Session ID를 인증된 tenant와 principal에 binding. unknown, expired, malformed,
  cross-principal ID는 다른 tenant의 소유 여부를 드러내지 않고 fail closed.
- Notification에는 JSON-RPC response를 보내지 않음. HTTP status는 transport failure,
  JSON-RPC error는 protocol failure, `tools/call` 실행 실패는 현재 MCP의 `isError: true` 계약 유지.
- Protocol core가 명시적으로 구현하고 test하기 전에는 batch JSON-RPC 미지원.

### 2.2 Origin 및 DNS rebinding 방어

- Request를 읽거나 dispatch하기 전에 `Origin`을 exact deployment allowlist와 대조. 예상하지 않은
  origin은 거부. non-browser client를 의도적으로 지원하는 deployment는 absent-Origin policy를
  명시하고 test해야 하며, 모든 요청을 암묵적으로 허용하지 않음.
- Trusted reverse proxy가 복원한 external host와 scheme 검증. unknown `Host`, forwarded host,
  SNI 조합을 거부해 DNS rebinding과 host-header routing 차단.
- `Forwarded`와 `X-Forwarded-*`는 configured proxy address에서 온 경우에만 신뢰. Application
  port는 private이며 Internet에서 직접 접근 불가.
- CORS는 인증이 아님. Preflight는 configured origin, method, header만 노출하며 credential과 `*`를
  함께 사용하지 않음.

## 3. Architecture

```text
stdio adapter                         Streamable HTTP adapter
(newline framing, LocalFsContext)     (HTTP, auth, session, RemoteArtifactContext)
             \                         /
              transport-independent JSON-RPC core
              initialize | ping | tools/list | tools/call
                              |
                    tool registry and schemas
                              |
                  existing execute/library APIs
                              |
             parse | edit | convert | render | certify
```

### 3.1 Core 분리

HTTP 추가 전에 `crates/hwp-cli/src/commands/mcp.rs`를 다음과 같이 refactor한다.

1. `handle_json(Value, RequestContext) -> Option<Value>`, protocol negotiation, tool registry,
   schema, dispatch를 library-visible module로 분리.
2. Framing, body limit, transport header, connection lifecycle은 adapter에 유지.
3. Protocol core에 박힌 `PathBuf` 대신 context trait로 file authority 표현.
4. 기존 stdio adapter와 process integration test를 no-regression gate로 유지.
5. Synchronous document work를 async I/O executor에서 직접 실행하지 않음. Deadline과
   cancellation을 갖춘 bounded blocking worker pool 또는 isolated worker process 사용.

기존 `handle_request`, `call_tool`, `tool_defs`가 출발 seam이지만 최종 public API는 아니다. 현재는
handler, tool registry, local path guard, stdio loop가 binary-private 단일 module에 결합되어 있다.

### 3.2 Local/remote authority

`LocalFsContext`는 trusted desktop 사용을 위해 현재 canonical `--root` 동작을 유지한다.
`RemoteArtifactContext`는 다른 계약을 사용한다.

- Input은 request limit 이하 inline content 또는 인증 tenant 소유 opaque `artifact_id`.
- Upload는 worker 밖에서 scan/storage 처리. Worker는 새 job directory의 read-only materialized
  copy만 수신.
- Output은 job directory 안에서만 작성하고 atomic finalize 뒤 새 immutable artifact로 등록.
- Tool result는 metadata와 `artifact_id` 반환. Download는 authenticated artifact endpoint 또는
  short-lived single-purpose signed URL 사용.
- 기존 local-path tool schema를 remote client에 그대로 publish하지 않음. Remote schema는 모든
  input/output/image/font/spec/policy/reference/parts path를 content 또는 artifact reference로 변환.
- Nested asset은 기존 opened-handle containment check를 계속 사용. 이는 defense in depth이며
  tenant ownership check를 대체하지 않음.

MCP endpoint가 하나라는 것은 artifact byte까지 JSON-RPC에 넣어야 한다는 뜻이 아니다. Upload와
Download는 별도 authenticated artifact service 또는 signed object-store URL을 사용할 수 있지만,
동일 tenant authorization과 audit context를 공유해야 한다.

## 4. 인증 및 인가

Quick Web 연동에서 service는 OAuth client secret holder가 아니라 OAuth/OIDC protected resource다.

Resource server 책임:

- protected-resource metadata를 publish/reference하고 허용 authorization server 식별;
- token signature, issuer, audience/resource, expiry, not-before, required scope 검증;
- 다른 resource용 token 거부 및 token detail을 누출하지 않는 standards-compliant
  `WWW-Authenticate` challenge 반환;
- validated claim과 explicit tenancy mapping만으로 tenant/principal 도출;
- 모든 MCP request, session lookup, artifact read/write, administrative action 인가;
- `mcp:tools`, `artifacts:read`, `artifacts:write` 등 least-privilege scope 사용;
- access token을 URL, persistent storage, log에 남기지 않음;
- Quick access token을 document tool이나 무관한 downstream service로 전달하지 않음;
- key rotation, clock skew, token revocation/expiry, emergency issuer disablement 정의.

OAuth는 service 호출 권한을 증명한다. `Mcp-Session-Id`는 protocol state를 연결한다. `artifact_id`는
tenant 소유 content를 식별한다. 어느 하나도 나머지 두 검사를 대체하지 않는다.

## 5. TLS/reverse-proxy 경계

- Public traffic은 TLS 1.2 이상 사용. Plain HTTP는 trusted TLS-terminating proxy의 isolated
  loopback/private network hop에만 허용.
- Proxy가 forwarding 전에 request header/body limit, slow-client timeout, canonical host routing,
  denial-of-service control 적용.
- Application도 content type, protocol version, origin, authorization, body size를 재검증.
  Proxy 검사를 단독 security boundary로 사용하지 않음.
- Forwarded client identity는 validated access token으로 cryptographically 전달되지 않으면 무시.
  Internal service authentication에는 mTLS 또는 workload identity 사용 가능.
- Error와 audit event는 request ID를 사용하되 server path, stack trace, token, document content,
  cross-tenant identifier를 노출하지 않음.

## 6. Tenant/session/workspace 격리

Containment hierarchy는 `tenant / principal / session / job`이다.

- 각 level은 server-minted opaque ID 사용. User text를 directory name으로 사용하지 않음.
- 각 job은 byte quota가 있는 새 private directory에서 시작하며 inherited current working
  directory가 없음. Worker는 다른 session directory를 enumerate할 수 없음.
- Input은 read-only. Output은 별도 path와 atomic publication 사용. Symlink, hardlink, device file,
  socket, parent traversal 거부.
- Font set, policy, optional certification oracle은 deployment-controlled resource이며 client path가
  아니라 approved ID로 참조.
- Document worker의 network egress는 기본 차단. External oracle은 administrator가 named policy로
  활성화한 경우에만 실행.
- Session deletion은 즉시 신규 call을 차단하고 queued work를 cancel하며 workspace/artifact
  reference cleanup 예약. Cleanup은 idempotent.
- Process crash가 authority를 합치면 안 됨. Recovery 시 tenant, session, artifact ownership,
  expiry를 다시 검증한 뒤 resume.

## 7. 초기 operational limit

아래 값은 conservative launch default이며 deployment-wide maximum 이하에서 설정 가능해야 한다.

| Limit | 초기 기본값 | Failure behavior |
|---|---:|---|
| JSON request body | 1 MiB | JSON parsing 전 `413` |
| 단일 uploaded artifact | 64 MiB | Upload 거부 및 partial data 삭제 |
| materialized session workspace | 256 MiB | Job 중단, partial output 미보존 |
| JSON response body | 1 MiB, 큰 document output은 artifact 전환 | Artifact metadata 반환 |
| concurrent running job | session당 2, tenant당 8 | bounded retry 안내와 `429` |
| 기본 tool deadline | 120 s | Worker cancel/kill 후 timeout 반환 |
| render/certify deadline | 300 s | Worker cancel/kill 후 timeout 반환 |
| idle session lifetime | 30 min | Expire 및 workspace cleanup |
| maximum session lifetime | 8 h | Reinitialize 요구 |
| completed output artifact TTL | 기본 1 h | 만료 뒤 authenticated download 실패 |
| failed/cancelled workspace retention | 최대 15 min | 자동 삭제 |
| audit metadata retention | 기본 30 days | metadata만 보관, document body/token 제외 |

Session당 artifact 수, render page 수, pixel dimension, decompressed archive size, nested archive
depth, grep match, tool queue length도 제한한다. ZIP/CFB parser와 renderer가 upload 단계뿐 아니라
worker 내부에서도 limit을 강제해야 한다.

Audit event에는 timestamp, request ID, hashed session reference, tenant/principal reference,
tool name, input/output artifact ID, byte count, duration, outcome, policy version을 포함한다. Document
text가 든 raw argument, local path, access token, artifact byte는 제외한다.

## 8. Dependency decision gate

현재 server는 의도적으로 tokio와 MCP SDK를 사용하지 않는다. Remote transport 작업에서 runtime이나
framework를 부수적으로 선택하면 안 된다.

구현 전 ADR에서 최소 다음을 비교한다.

1. mature MCP SDK + async HTTP stack;
2. HTTP stack + 기존 hand-written protocol core;
3. no-tokio 기조를 유지하는 synchronous/process-isolated service.

Protocol conformance, SSE/disconnect 동작, OAuth integration, maintenance/security update burden,
binary size, compile time, blocking-work isolation, workspace MSRV 호환성을 측정한다. Custom parsing보다
maintained protocol/HTTP primitive를 우선한다. no tokio/no SDK 유지는 unsafe framework를 재구현하지
않고 conformance, cancellation, streaming, security test를 통과할 때만 허용한다.

## 9. 단계별 follow-up backlog

1. **Core extraction:** typed JSON-RPC value와 authority context 도입. stdio output과 16개 tool의
   byte/behavior compatibility 유지.
2. **Artifact model:** network listener 없이 tenant-owned upload, immutable output, remote-safe
   schema, quota, cleanup 구현.
3. **Transport/auth:** 선택한 HTTP adapter, protocol header, SSE/session lifecycle, OAuth resource
   server validation, proxy boundary, audit event를 명시적 build/deployment mode 뒤에 추가.
4. **Quick Web pilot:** non-production connector 등록. 16개 tool의 remote-safe parity, upload,
   edit, render, convert, download, expiry, reconnect 검증.
5. **Security gate:** cross-tenant, rebinding, proxy bypass, archive bomb, race, timeout,
   cancellation, load test 완료. General availability 전 independent review.
6. **Operations:** production enablement 전에 SLO, capacity, key rotation, incident response,
   deletion verification, backup 정책, cost control 정의.

## 10. Acceptance/failure 기준

Remote MCP 구현은 다음을 모두 입증해야 통과한다.

- stdio가 기본값으로 남고 기존 process test가 정확히 16개 tool을 계속 노출;
- HTTP conformance가 `POST`, `GET`, `DELETE`, 두 response media type, version negotiation,
  notification, session 생성/종료, reconnect, disconnect를 검증;
- invalid origin, host, content type, protocol version, token, scope, session, artifact ownership이
  tool 실행 전에 실패;
- 두 tenant가 traversal, symlink/hardlink, race, nested asset, error-message attack을 포함해 서로의
  session/artifact를 추론, 읽기, 덮어쓰기, link, 보존할 수 없음;
- request, upload, decompression, workspace, concurrency, deadline, response, retention limit을
  강제하고 deterministic cleanup 수행;
- worker termination 뒤 published partial artifact나 reusable authority가 남지 않음;
- log/error에 token, document content, client-local path, server filesystem path가 없음;
- TLS/trusted proxy/host-origin check/OAuth validation을 우회해 application backend에 접근 불가;
- tenant-isolated test environment에서 Quick Web이 initialize, 의도한 16개 remote-safe tool list,
  upload-to-download edit workflow를 완료.

어느 기준이든 미검증이면 release gate 실패다. HTTP가 unrestricted `hwp mcp`를 단순 shell-out하거나,
local path schema를 remote에서 writable하게 유지하거나, public deployment에서 인증이 optional이면
출시하지 않는다.
