[한국어](20-remote-mcp.ko.md) · [English](20-remote-mcp.md)

# Remote MCP transport design

> **Status:** design only. The current release implements only `hwp mcp` over stdio. It does not
> contain an HTTP listener, OAuth resource server, hosted workspace, or artifact service.

This document defines a future Remote MCP service for web clients that cannot launch a local
process. Amazon Quick Web is the first concrete consumer. Implementation remains tracked by
[issue #52](https://github.com/STAIxBWLB/hwp-cli/issues/52).

## 1. Scope and non-goals

### 1.1 Scope

- Preserve `hwp mcp` stdio as the default local transport and keep its 17-tool behavior compatible.
- Add an explicit, separately configured Streamable HTTP mode for hosted deployments.
- Reuse one transport-independent JSON-RPC and tool-dispatch core from both adapters.
- Authenticate every remote request and isolate workspaces by tenant, principal, and session.
- Replace client-local path authority with uploaded content and server-owned artifact references.
- Define security, limits, observability, cleanup, and release gates before runtime work begins.

### 1.2 Non-goals

- This change does not implement HTTP, OAuth/OIDC, hosting, or Quick Web registration.
- The local stdio command is not exposed directly through a reverse proxy.
- A remote client never receives arbitrary server filesystem access or supplies an absolute server
  path.
- The service is not a general file store, long-term document archive, or collaborative editor.
- This design does not select an async runtime, web framework, MCP SDK, cloud, or identity provider.

## 2. Compatibility baseline

The first implementation targets MCP `2025-06-18`, which the current stdio server already
negotiates. Streamable HTTP uses one MCP endpoint and HTTP `POST`, `GET`, and `DELETE`. Protocol
requirements are based on the official [2025-06-18 transport
specification](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports) and
[authorization specification](https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization).

The protocol has continued to evolve. Before implementation, verify the protocol version accepted
by Quick Web. If a later version removes protocol-level sessions, implement a versioned adapter;
do not mix later session semantics into the `2025-06-18` contract.

Content from the linked specifications is paraphrased for licensing compliance.

### 2.1 Endpoint contract

The MCP endpoint is a single configurable path, defaulting to `POST|GET|DELETE /mcp`.

| Method | Purpose | Successful response |
|---|---|---|
| `POST` | Send one JSON-RPC message | `application/json` for one response, or `text/event-stream` when streaming is needed |
| `GET` | Open the optional server-to-client SSE stream for an established session | `text/event-stream` |
| `DELETE` | Explicitly terminate a session and schedule its workspace for cleanup | Empty success response |

Rules:

- A client sends `Accept: application/json, text/event-stream` on `POST`; JSON request bodies use
  `Content-Type: application/json`.
- The server validates `MCP-Protocol-Version` after initialization. Missing, unsupported, or
  contradictory versions fail before tool dispatch.
- For the `2025-06-18` stateful profile, the server creates an opaque, cryptographically random
  `Mcp-Session-Id` during initialization. The client returns it on later `POST`, `GET`, and `DELETE`
  requests. A session ID is never accepted as authorization by itself.
- Session IDs are bound to the authenticated tenant and principal. Unknown, expired, malformed, or
  cross-principal IDs fail closed without revealing whether another tenant owns the ID.
- Notifications receive no JSON-RPC response. HTTP status codes report transport failures;
  JSON-RPC errors report protocol failures; `tools/call` execution failures retain the current MCP
  `isError: true` result contract.
- Batch JSON-RPC is unsupported until the protocol core explicitly implements and tests it.

### 2.2 Origin and DNS rebinding defense

- Validate `Origin` against an exact deployment allowlist before reading or dispatching a request.
  Reject an unexpected origin. A deployment that intentionally supports non-browser clients must
  define and test its absent-Origin policy rather than silently allowing every request.
- Validate the external host and scheme reconstructed by the trusted reverse proxy. Reject unknown
  `Host`, forwarded host, or SNI combinations to prevent DNS rebinding and host-header routing.
- Trust `Forwarded` and `X-Forwarded-*` only from configured proxy addresses. The application port
  is private and cannot be reached directly from the Internet.
- CORS is not authentication. Preflight responses expose only the configured origin, methods, and
  headers; credentials are never combined with `*`.

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

### 3.1 Core extraction

Refactor `crates/hwp-cli/src/commands/mcp.rs` before adding HTTP:

1. Extract `handle_json(Value, RequestContext) -> Option<Value>`, protocol negotiation, tool
   registry, schemas, and dispatch into a library-visible module.
2. Keep framing, body limits, transport headers, and connection lifecycle in adapters.
3. Represent file authority through a context trait rather than `PathBuf` arguments embedded in the
   protocol core.
4. Keep the existing stdio adapter and process integration test as a no-regression gate.
5. Do not make synchronous document work run on an async I/O executor. Use a bounded blocking worker
   pool or isolated worker processes with deadlines and cancellation.

The existing string `handle_request`, `call_tool`, and `tool_defs` functions are the starting seams,
not the final public API. The current handler, tool registry, local path guards, and stdio loop are
still combined in one binary-private module.

### 3.2 Local and remote authority

`LocalFsContext` preserves the current canonical `--root` behavior for trusted desktop use.
`RemoteArtifactContext` uses a different contract:

- Inputs are inline content below the request limit or opaque `artifact_id` values owned by the
  authenticated tenant.
- An upload is scanned and stored outside the worker. The worker receives a read-only materialized
  copy in a new job directory.
- Outputs are written only in that job directory, finalized atomically, and registered as new
  immutable artifacts.
- Tool results return metadata plus an `artifact_id`; downloads use an authenticated artifact
  endpoint or a short-lived, single-purpose signed URL.
- Existing local-path tool schemas are not published unchanged to remote clients. A remote schema
  maps every input, output, image, font, spec, policy, reference, and parts path to content or an
  artifact reference.
- Nested assets retain the existing opened-handle containment checks. They are defense in depth,
  not a substitute for tenant ownership checks.

One MCP endpoint does not require artifact bytes to travel inside JSON-RPC. Upload and download may
use a separate authenticated artifact service or signed object-store URLs, but they must share the
same tenant authorization and audit context.

## 4. Authentication and authorization

Quick Web integration treats the service as an OAuth/OIDC protected resource, not as an OAuth
client secret holder.

The resource server must:

- publish or reference protected-resource metadata and identify the accepted authorization server;
- validate token signature, issuer, audience/resource, expiry, not-before time, and required scopes;
- reject tokens intended for another resource and return standards-compliant `WWW-Authenticate`
  challenges without leaking token details;
- derive tenant and principal only from validated claims and an explicit tenancy mapping;
- authorize every MCP request, session lookup, artifact read/write, and administrative action;
- use least-privilege scopes such as `mcp:tools`, `artifacts:read`, and `artifacts:write`;
- never accept access tokens in URLs, persist raw tokens, or include them in logs;
- never forward Quick access tokens to document tools or unrelated downstream services;
- define key rotation, clock skew, token revocation/expiry behavior, and emergency issuer disablement.

OAuth proves who may call the service. `Mcp-Session-Id` correlates protocol state. `artifact_id`
identifies tenant-owned content. None of these replaces the other two checks.

## 5. TLS and reverse-proxy boundary

- Public traffic uses TLS 1.2 or newer. Plain HTTP is allowed only on an isolated loopback or private
  network hop from a trusted TLS-terminating proxy.
- The proxy enforces request-header and body limits, slow-client timeouts, canonical host routing,
  and denial-of-service controls before forwarding.
- The application repeats content-type, protocol-version, origin, authorization, and body-size
  validation. Proxy checks are not the sole security boundary.
- Forwarded client identity is ignored unless cryptographically conveyed by the validated access
  token. Internal service authentication may use mTLS or workload identity.
- Error responses and audit events use request IDs but do not expose server paths, stack traces,
  tokens, document content, or cross-tenant identifiers.

## 6. Tenant, session, and workspace isolation

The containment hierarchy is `tenant / principal / session / job`.

- Each level uses a server-minted opaque ID. User text never becomes a directory name.
- Every job starts in a new private directory with an explicit byte quota and no inherited current
  working directory. The worker cannot enumerate another session's directory.
- Inputs are read-only. Outputs use separate paths and atomic publication. Symlinks, hardlinks,
  device files, sockets, and parent traversal are rejected.
- Font sets, policies, and optional certification oracles are deployment-controlled resources,
  referenced by approved IDs rather than client paths.
- Network egress is denied for document workers by default. External oracles run only when an
  administrator enables a named policy.
- Session deletion prevents new calls immediately, cancels queued work, and schedules workspace and
  artifact-reference cleanup. Cleanup is idempotent.
- A process crash must not merge authority: recovered jobs revalidate tenant, session, artifact
  ownership, and expiry before resuming.

## 7. Initial operational limits

These are conservative launch defaults and must be configurable below deployment-wide maxima.

| Limit | Initial default | Failure behavior |
|---|---:|---|
| JSON request body | 1 MiB | `413` before JSON parsing |
| Single uploaded artifact | 64 MiB | Reject upload and delete partial data |
| Materialized session workspace | 256 MiB | Stop the job, preserve no partial output |
| JSON response body | 1 MiB; larger document output becomes an artifact | Return artifact metadata |
| Concurrent running jobs | 2 per session, 8 per tenant | `429` with bounded retry guidance |
| Default tool deadline | 120 s | Cancel or kill worker, return timeout |
| Render/certify deadline | 300 s | Cancel or kill worker, return timeout |
| Idle session lifetime | 30 min | Expire and clean workspace |
| Maximum session lifetime | 8 h | Require reinitialization |
| Completed output artifact TTL | 1 h by default | Authenticated download fails after expiry |
| Failed or cancelled workspace retention | 15 min maximum | Automatic deletion |
| Audit metadata retention | 30 days by default | Metadata only; no document body or token |

Also cap artifacts per session, pages per render, pixel dimensions, decompressed archive size, nested
archive depth, grep matches, and tool queue length. ZIP/CFB parsers and renderers must enforce limits
inside the worker, not only at upload time.

Audit events include timestamp, request ID, hashed session reference, tenant/principal reference,
tool name, input/output artifact IDs, byte counts, duration, outcome, and policy version. They exclude
raw arguments containing document text, local paths, access tokens, and artifact bytes.

## 8. Dependency decision gate

The current server intentionally uses no tokio and no MCP SDK. Remote transport work must not add a
runtime or framework as an incidental choice.

Before implementation, write an ADR comparing at least:

1. a mature MCP SDK and async HTTP stack;
2. an HTTP stack with the existing hand-written protocol core; and
3. a synchronous or process-isolated service that keeps the no-tokio stance.

The decision must measure protocol conformance, SSE and disconnect behavior, OAuth integration,
maintenance and security update burden, binary size, compile time, blocking-work isolation, and
compatibility with the workspace MSRV. Prefer maintained protocol and HTTP primitives over custom
parsing. Keeping no tokio/no SDK is acceptable only if conformance, cancellation, streaming, and
security tests pass without recreating an unsafe framework.

## 9. Phased follow-up backlog

1. **Core extraction:** introduce typed JSON-RPC values and authority contexts; keep stdio output and
   all 17 tools byte/behavior compatible.
2. **Artifact model:** implement tenant-owned uploads, immutable outputs, remote-safe schemas, quotas,
   and cleanup without opening a network listener.
3. **Transport and auth:** add the selected HTTP adapter, protocol headers, SSE/session lifecycle,
   OAuth resource-server validation, proxy boundary, and audit events behind an explicit build or
   deployment mode.
4. **Quick Web pilot:** register a non-production connector, verify initialization and remote-safe
   parity for all 17 tools, then test upload, edit, render, convert, download, expiry, and reconnect.
5. **Security gate:** complete cross-tenant, rebinding, proxy-bypass, archive-bomb, race, timeout,
   cancellation, and load tests; obtain an independent review before general availability.
6. **Operations:** define SLOs, capacity, key rotation, incident response, deletion verification,
   backups if any, and cost controls before production enablement.

## 10. Acceptance and failure criteria

A Remote MCP implementation is acceptable only when all of the following are demonstrated:

- stdio remains the default and its existing process test still exposes exactly 17 tools;
- HTTP conformance covers `POST`, `GET`, `DELETE`, both response media types, version negotiation,
  notifications, session creation/termination, reconnect, and disconnect behavior;
- invalid origin, host, content type, protocol version, token, scope, session, or artifact ownership
  fails before tool execution;
- two tenants cannot infer, read, overwrite, link, or retain each other's sessions or artifacts,
  including through traversal, symlink/hardlink, race, nested asset, or error-message attacks;
- request, upload, decompression, workspace, concurrency, deadline, response, and retention limits
  are enforced and produce deterministic cleanup;
- worker termination leaves no published partial artifact and no reusable authority;
- logs and errors contain no tokens, document content, client-local paths, or server filesystem paths;
- the application backend cannot be reached by bypassing TLS, the trusted proxy, host/origin checks,
  or OAuth validation;
- Quick Web completes initialize, lists the intended 17 remote-safe tools, and performs an
  upload-to-download edit workflow in a tenant-isolated test environment.

The implementation fails the release gate if any criterion is unverified, if HTTP merely shells out
to unrestricted `hwp mcp`, if local path schemas remain remotely writable, or if authentication is
optional in a public deployment.
