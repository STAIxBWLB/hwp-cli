[한국어](22-remote-mcp-deployment.ko.md) · [English](22-remote-mcp-deployment.md)

# Remote MCP deployment design

> **Status:** design only. No HTTP adapter, container image, Worker, or cloud runtime exists yet.
> The shipped product is still `hwp mcp` over stdio.

[Doc 20](20-remote-mcp.md) defines *what* a Remote MCP service must guarantee: endpoint contract,
authentication, isolation, limits, and release gates. This document defines *how* it gets built
and *where* it runs. It selects the runtime dependency (doc 20 §8 gate), specifies one
platform-neutral Rust HTTP adapter, and specifies two deployment tiers that share it.

[Issue #52](https://github.com/STAIxBWLB/hwp-cli/issues/52) was closed as deferred because the
remote service had no named consumer, owner, OAuth policy, or budget, and it asks for a fresh
scoped delivery issue once those exist. This document supplies them, and the delivery issue that
accompanies it is the activation record.

## 1. Decision summary

| Question | Decision |
|---|---|
| Where does document work run? | A native `hwp serve` HTTP mode inside a container, not a bridge that shells out to `hwp mcp`, and not wasm |
| HTTP dependency (doc 20 §8) | Synchronous `tiny_http`, keeping the workspace's no-tokio, no-SDK stance |
| Where does the service run? | Two tiers sharing one binary: **Tier A** Cloudflare Workers + Containers, **Tier B** AWS Bedrock AgentCore behind an Amazon Quick Suite connector |
| Who issues tokens? | Tier A: the Worker itself, via `@cloudflare/workers-oauth-provider`, with Google as upstream IdP. Tier B: Amazon Cognito, with Google as a federated IdP |
| File transfer in the first implementation | Session workspace, not the doc 20 §3.2 artifact model. Recorded as an amendment in §7 |

Tier order is not fixed by this document. Each tier is independently deployable from the same
binary once the shared Rust work lands.

## 2. Platform evaluation

Three hosting models were compared against the requirements that the service must authenticate
users, issue tokens, support Google sign-in, and execute a native Rust binary that needs a
filesystem and font files.

| Criterion | Cloudflare Workers + Containers | AWS AgentCore + Quick | Vercel Functions |
|---|---|---|---|
| Runs a native binary | Containers (microVM) invoked from a Worker | Container is the unit of deployment | Binary bundled into a function, or the beta Rust runtime |
| OAuth authorization server for MCP clients | First-party: `workers-oauth-provider` implements `/authorize`, `/token`, and RFC 7591 dynamic client registration | Cognito issues JWTs; no dynamic client registration, so clients are registered by an administrator | None first-party; requires an external IdP or a hand-written server |
| Session affinity | Built by hand from a Durable Object plus the Container class | Managed: the runtime injects `Mcp-Session-Id` and routes a session to its own microVM | None; instances are not addressable |
| Extra HTTP routes beside `/mcp` | Available, so a file-upload sideband is possible | Not available; `0.0.0.0:8000/mcp` is the entire contract | Available |
| CPU architecture | amd64 | **arm64 required** | amd64 |
| Fit with doc 20 | Advances backlog items 1 and 3 | Additionally reaches backlog item 4, the Quick pilot named in doc 20 §1 | Same as Cloudflare, minus the auth server |
| Cost shape | Workers Paid subscription plus container usage | Per-second vCPU and memory metering, no subscription | Pro seat subscription plus usage |

**Vercel is rejected.** It offers no first-party authorization server, so signup and token
issuance would depend on a third-party identity service, and it offers no instance affinity, so
the full artifact model of doc 20 §3.2 would be a precondition rather than a later phase. Neither
cost is justified when another platform removes both problems.

**Both remaining platforms are specified**, because they serve different consumers. Tier A serves
an open service where any person signs up with a Google account and connects any MCP client.
Tier B serves an organization whose administrator registers one connector inside Amazon Quick
Suite, which is the consumer doc 20 §1 named from the start. The Rust work is identical for both;
only the adapter's configuration and the surrounding cloud resources differ.

## 3. Shared Rust workstream

### 3.1 Protocol core extraction

`crates/hwp-cli/src/commands/mcp.rs` is split into a module directory before any HTTP work
begins, as required by doc 20 §3.1:

| File | Contents |
|---|---|
| `commands/mcp/mod.rs` | Protocol core: `handle_request`, protocol negotiation, tool registry, schemas, dispatch |
| `commands/mcp/authority.rs` | `FileAuthority` trait plus `LocalFsContext`, the existing canonical-root and font-directory checks |
| `commands/mcp/stdio.rs` | The existing newline-framed loop, including the zeroizing read buffer and password scrubbing |
| `commands/mcp/http.rs` | The `hwp serve` adapter |

**Amendment to doc 20 §3.1.** Doc 20 asks for a "library-visible module". The protocol core
reaches into 16 binary-private command modules across roughly 70 call sites, so making it
library-visible would move most of `commands/` into the `hwp_cli` library for no current
out-of-binary consumer. The first implementation therefore keeps the core binary-internal and
shares it between the two adapters compiled into the same `hwp` binary. Doc 20 already describes
the current functions as "starting seams, not the final public API". Promotion to the library
happens when a consumer outside the binary actually exists.

The stdio adapter keeps its present behavior byte for byte. The existing stdio process test,
which asserts exactly 20 tools, is the regression gate for the whole split.

### 3.2 `hwp serve` contract

A new subcommand runs the protocol core over HTTP. It is a private hop: in both tiers a trusted
edge terminates TLS, authenticates the caller, validates origin, and caps the request body before
anything reaches this server.

| Route | Behavior |
|---|---|
| `POST /mcp` | Body capped at 1 MiB, mirroring the stdio `MAX_REQUEST_LINE_BYTES`. A request returns `200 application/json`; a notification produces no protocol output and returns `202` with an empty body |
| `GET /mcp` | `405`. The server never pushes, so no SSE stream is offered |
| `GET /healthz` | `200` once the listener is bound. Container platforms probe this for readiness |
| `POST /files/{name}` | Enabled only by `--files`. Streams an upload into the workspace root |
| `GET /files/{name}` | Enabled only by `--files`. Streams a workspace file back |

Flags: `--addr` (default `0.0.0.0:8080`; Tier B runs `0.0.0.0:8000`), `--root` (**mandatory**, unlike
stdio where an absent root only warns), `--font-dir`, and `--files`.

Rules the adapter enforces:

- Refuse to start without `--root`. A remote deployment never runs with unrestricted filesystem
  authority.
- Accept and ignore any inbound `Mcp-Session-Id`. Tier B's platform injects one, and session
  affinity is the platform's responsibility, not the document server's.
- On the `/files` routes, accept a name matching `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`, resolve it
  under the root, cap one file at 64 MiB and the workspace at 256 MiB, and unlink a partial upload
  when a cap is breached.
- Handle one request at a time. One container serves one MCP session, so there is no concurrency
  to exploit and no interleaving to corrupt.

### 3.3 Remote-safe inline content

Tier B exposes no route other than `/mcp`, so documents must travel inside JSON-RPC. Tool schemas
gain an inline mode in which a document input or output is base64 content with a size cap rather
than a path. The pattern already exists in the current server: `hwp_compose` accepts a `spec`
inline or as `spec_path`, and `hwp_template` accepts inline template and data through
`inline_contract_input`.

This is the first slice of the doc 20 §3.2 remote schema requirement. The complete artifact model,
meaning tenant-owned uploads, immutable outputs, retention, and signed download URLs backed by an
object store, is a later phase in both tiers.

## 4. Dependency decision record

This section satisfies the decision gate in doc 20 §8, which forbids adding an async runtime or an
MCP SDK as an incidental choice.

### 4.1 Options

**Option 1: the Rust MCP SDK with tokio and axum.** Complete Streamable HTTP conformance, SSE, and
session handling arrive already implemented and tested. Against it: in this architecture the edge
already owns authentication, sessions, origin validation, and body limits, and no SSE stream is
offered, so most of what the SDK provides is either duplicated or unused. It introduces tokio into
a workspace that deliberately has none, requires re-registering 20 tools against the SDK's API,
and adds compile time, binary size, and MSRV exposure. Doc 20 §3.1 also warns that synchronous
document work must not run on an async I/O executor, which would require `spawn_blocking`
discipline throughout.

**Option 2: an async HTTP stack without the SDK.** Pays the tokio cost of option 1 while keeping
the hand-written protocol core, so it buys none of option 1's conformance benefit.

**Option 3: a synchronous server on `tiny_http`.** One small maintained dependency, a thread-based
model, no async runtime. The existing protocol core is reused unchanged, and blocking document work
runs naturally on the request thread.

### 4.2 Decision

**Option 3.** Doc 20 §8 permits keeping the no-tokio stance only if conformance, cancellation,
streaming, and security tests pass without recreating an unsafe framework. That condition is met
by scope reduction rather than by effort: the in-container HTTP surface is a private hop with at
most five routes, unreachable except through the trusted edge, so hostile-input parsing, TLS,
slow-client defense, and origin validation are the edge's responsibility. Streaming is explicitly
not offered. Cancellation is the edge's deadline followed by container termination.

`tiny_http` is chosen over hand-rolled `TcpListener` parsing because doc 20 §8 prefers maintained
HTTP primitives over custom parsing.

**Revisit trigger.** If a client requires SSE or resumable streams, or if one process must ever
host multiple sessions concurrently, this decision flips to option 1 and a new record supersedes
this section.

## 5. Tier A: Cloudflare public service

```text
MCP client
   |  HTTPS
   v
Worker (TypeScript)
   |  workers-oauth-provider: /authorize, /token, /register (dynamic client registration)
   |  Google upstream IdP; first login creates the user record
   |  personal access token middleware; body limit; origin check; audit
   v
Durable Object HwpSession, named tenant-principal plus session
   |  Container class: idle sleep, maximum-lifetime alarm, deadlines, no egress
   v
microVM: hwp serve --addr 0.0.0.0:8080 --root /work --files --font-dir <fonts>
```

### 5.1 Signup, login, and token issuance

The Worker is the OAuth authorization server that MCP clients talk to, and Google is only the
upstream identity provider. This is the third-party authorization flow of the MCP `2025-06-18`
specification: the client never receives a Google token, only a token this service minted.

1. An unauthenticated `POST /mcp` returns `401` with a `WWW-Authenticate` challenge.
2. The client reads protected-resource and authorization-server metadata, then registers itself at
   `/register`. Dynamic client registration means no administrator has to pre-provision anything.
3. The client opens `/authorize` in a browser. The Worker shows a consent screen, then redirects to
   Google with the `openid email profile` scopes.
4. Google returns to `/callback`. The Worker exchanges the code, reads the identity claims, and
   **creates the user record if the `sub` claim is new. First Google login is signup.** It then
   completes authorization with the `mcp:tools` scope.
5. The client exchanges its code at `/token` using PKCE and receives this service's access token.
   Grants and tokens are stored hashed in a KV namespace, which the provider library requires to be
   bound as `OAUTH_KV`.

For clients configured with a static header rather than an OAuth flow, a dashboard issues personal
access tokens. A token is `hwp_pat_` followed by 32 random bytes in base64url, shown once at
creation and stored only as a SHA-256 hash. A middleware ahead of the OAuth provider recognizes the
prefix, looks the hash up, and invokes the same MCP handler with an equivalent identity, so there is
exactly one authorization path downstream.

### 5.2 Data model

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

The identity key is the Google `sub` claim, not the email address, because an email address can be
reassigned. `session_ref` is a hash of the session identifier, never the identifier itself, and the
audit table holds no tool arguments, paths, document content, or tokens, as doc 20 §7 requires.

### 5.3 Session lifecycle

| Event | Mechanism |
|---|---|
| `initialize` | The Worker mints a random session identifier and derives the Durable Object name from the principal and that identifier. The container starts, the readiness probe passes, and the response carries `Mcp-Session-Id` |
| Later calls | The same header reaches the same object, therefore the same microVM, workspace, and process |
| Idle 30 minutes | The container class stops the instance. The object marks the session dead, so the next call returns `404` and the client reinitializes |
| `DELETE /mcp` | Terminates the container, marks the session dead, cancels the alarm, and returns `204`. Idempotent |
| Maximum lifetime 8 hours | An alarm terminates the session, which forces reinitialization as doc 20 §7 requires |
| Deadline exceeded | 120 seconds by default and 300 seconds for rendering, conversion, and certification. The Worker aborts, terminates the container, and returns a timeout |
| Crash or eviction | The process exit stops the container, the object marks the session dead, and later calls return `404` |
| Workspace cleanup | Guaranteed by teardown. The workspace is container-local disk that is never persisted, so nothing survives to be collected |

**Cross-principal access fails closed by construction.** The object name derives from the
authenticated principal, so presenting another person's session identifier resolves to a different
object that was never initialized, and the response is an ordinary `404` that reveals nothing about
who owns the identifier.

### 5.4 Resources

A `deploy/cloudflare/` project holds the Worker source, the wrangler configuration, the D1 schema,
and the container definition. The configuration binds the container class with its Durable Object
migration, the `OAUTH_KV` namespace, the D1 database, and the dashboard's static assets. Secrets are
the Google client identifier and secret, plus a cookie signing key.

The container image is built from repository source while `serve` is unreleased. Once a release
carries `serve`, the image switches to downloading the checksum-verified release tarball onto a slim
base, which removes the Rust toolchain from the deploy path and shortens cold starts. Fonts come
from the distribution's Nanum package, matching the font baseline that CI already uses.

## 6. Tier B: AgentCore behind an Amazon Quick connector

```text
Amazon Quick Suite
   |  an administrator registers one MCP integration; each person authorizes it individually
   v
Amazon Cognito user pool: self-service signup, Google federation, JWT issuance
   v
AgentCore Runtime: validates the JWT, injects and routes Mcp-Session-Id,
   gives each session its own microVM
   v
arm64 container: hwp serve --addr 0.0.0.0:8000 --root /work --font-dir <fonts>
```

### 6.1 Platform contract

AgentCore Runtime requires the container to serve Streamable HTTP at `0.0.0.0:8000/mcp` from an
**arm64** image, and it manages `Mcp-Session-Id` itself, adding the header when a request lacks one
and routing a session to its dedicated microVM. The server must therefore tolerate a
platform-supplied session identifier rather than reject it, which §3.2 already requires.

Two consequences follow. First, the isolation layer that Tier A assembles from a Durable Object and
a container class is provided by the platform, so Tier B needs no edge code of its own. Second, the
runtime exposes only the MCP endpoint, so the `/files` sideband cannot exist and the inline content
mode of §3.3 is a hard prerequisite rather than an option.

### 6.2 Signup, login, and token issuance

Cognito covers the same three requirements that the Worker covers in Tier A: a user pool provides
self-service signup, Google is added as a federated identity provider so people sign in with a
Google account, and the pool issues the JWT that AgentCore's inbound authorizer validates.

Cognito does not implement dynamic client registration, so a Quick administrator registers the
connector manually with the authorization and token endpoints and the client credentials. Amazon
Quick Suite supports exactly this: its integration console accepts either a server that offers
dynamic registration or explicit endpoint and credential values, and it uses three-legged OAuth so
each person authorizes the connector under their own identity before Quick calls tools on their
behalf. The absence of dynamic registration is acceptable here precisely because registration is an
administrative act, whereas Tier A's self-service clients need it.

### 6.3 Additional work

- A new release target, `aarch64-unknown-linux-gnu`, and an arm64 container image.
- An image repository, the AgentCore runtime configuration, and log delivery.
- Verification before committing to a region: whether AgentCore and Quick MCP integration are both
  available in the intended region, and the runtime's current idle and maximum session lifetimes
  against the values in doc 20 §7.

## 7. File authority

The first implementation of both tiers uses a **session workspace** rather than the artifact model
of doc 20 §3.2. Tier A populates it through the `/files` routes; Tier B populates it through inline
content in tool arguments. Tools continue to take path arguments, but a path is a relative name
inside one private workspace.

**Amendment to doc 20 §10.** Doc 20 fails a release whose "local path schemas remain remotely
writable". That criterion targets a shared server on which a client's path argument could reach
another tenant's data or the host filesystem. In both tiers here, the path resolves inside a
single-session microVM that holds no other tenant's data, has no network egress, and is destroyed
when the session ends, and the existing canonicalize-and-contain check still runs inside the
process. The criterion is therefore amended for these tiers: absolute paths and traversal remain
rejected, and a workspace is never shared or reused.

What the amendment defers, and what a later phase must deliver, is the rest of §3.2: a second
`FileAuthority` implementation backed by an object store, opaque artifact identifiers owned by a
tenant, immutable outputs with retention, and download through an authenticated endpoint or a
short-lived signed URL. Only that phase satisfies §3.2 as written.

## 8. Security posture against doc 20 §10

Satisfied by the first implementation, with the point that enforces each:

| Criterion | Enforcement |
|---|---|
| stdio stays the default and still exposes exactly 20 tools | The existing stdio process test gates the core split |
| Authentication precedes tool execution | Tier A: the OAuth provider and the token middleware both reject before the handler. Tier B: the runtime's JWT authorizer rejects before the container |
| Sessions bind to a principal and cross-principal access fails closed | Tier A: the object name derives from the principal, so a foreign session identifier yields an uninformative `404`. Tier B: the platform scopes sessions to the authorized caller |
| Request, upload, workspace, and deadline limits with deterministic cleanup | Body rejection before parsing at the edge and again in `hwp serve`; file and workspace caps in the adapter; deadlines and termination at the edge |
| Tenants cannot read or overwrite each other's data | One microVM per session, no shared filesystem, plus the existing root containment inside the process |
| Termination leaves no published partial output and no reusable authority | The workspace dies with the microVM and the session identifier is marked dead |
| No tokens, document content, or paths in logs | The audit schema stores metadata only; tokens are stored hashed |
| The backend cannot be reached by bypassing the edge | Neither runtime exposes the container publicly, and `hwp serve` additionally refuses to run without a root |
| Document workers have no network egress | Egress disabled on the container in Tier A; the runtime's network configuration in Tier B |
| The service is not an unrestricted `hwp mcp` behind a proxy | There is no shell-out. A native adapter shares the protocol core and runs confined in a single-session microVM |

Deferred, and listed here so no reader mistakes the first implementation for a complete one: the
full artifact model of §3.2; SSE and resumable streams; per-request cancellation, since a deadline
currently ends the session; more than one concurrent job per session, since the server is
sequential while §7 permits two; scopes finer than `mcp:tools`; a strict origin allowlist rather
than allowing an absent origin for non-browser clients; and the independent review that §9 item 5
requires before general availability.

## 9. Operations

**Deployment risks to settle during implementation.** Whether MCP clients tolerate `405` on
`GET /mcp`, which the transport specification permits when a server offers no push stream, and
which must be confirmed against the clients that matter before any SSE work is considered.
Container cold start on `initialize`, which the slim release-tarball image reduces. Google OAuth
applications remain limited to a small number of test users until verification, and Cognito has
its own quotas. The Cloudflare containers configuration surface is still moving, so field names and
instance types are verified at implementation time rather than trusted from this document. Building
Rust inside the deploy step may exceed build limits, in which case the image is built in CI and
referenced from a registry.

**Cost shape.** Tier A is a Workers Paid subscription whose included container allowance covers
light use, with metered memory and vCPU beyond it. Tier B has no subscription and meters vCPU and
memory per second, with Cognito free below its monthly active user threshold. Neither is expensive
at pilot scale; both need a spending alarm before any public announcement.

**Release coupling.** Once the image installs a release binary, every hwp release that the service
should pick up requires a version and checksum bump followed by a redeploy. Automating that in CI
is deliberately deferred until the deployment itself is stable.

## 10. Activation requirements from issue #52

Issue #52 was closed as deferred and lists seven items that must be named and accepted before
implementation starts. This document answers them as follows.

| Requirement | Answer |
|---|---|
| A concrete web consumer and its MCP protocol version | Tier A: any MCP client that speaks `2025-06-18` Streamable HTTP, which the existing server already negotiates. Tier B: Amazon Quick Suite, the consumer doc 20 §1 named |
| Deployment and security owners | The repository owner, until the service has more than pilot traffic. Confirm at kickoff |
| OAuth issuer, audience, scopes, and authorization policy | Tier A: the Worker is the issuer, its MCP endpoint is the audience, the scope is `mcp:tools`, and Google is the upstream identity provider (§5.1). Tier B: a Cognito user pool is the issuer and the AgentCore runtime validates the audience (§6.2) |
| Tenant and session identity and persistence model | Principal derives from validated identity claims, keyed on the Google `sub` claim. A session is a server-minted identifier bound to that principal. Users and tokens persist in a relational store; session workspaces do not persist at all (§5.2, §5.3) |
| Upload and output limits, retention, deletion, abuse controls | The caps in §3.2 and doc 20 §7. Retention is zero by construction, because a workspace dies with its microVM. Rate limiting is named as deferred in §8 and must land before public announcement |
| Hosting target, rate limits, budget, monitoring | Hosting targets and cost shape are in §2 and §9. A spending alarm is required before any public announcement. A numeric budget cap and service-level objectives are the remaining open items, deferred to doc 20 §9 item 6 |
| Whether the dependency-minimal, no-SDK invariant remains practical | Yes. §4 records the comparison and keeps the invariant with one small synchronous HTTP dependency |

Two items therefore remain open at the time of writing: the named owners beyond the repository
owner, and a numeric budget cap with its monitoring targets. Both are operational decisions rather
than design decisions, and neither blocks the shared Rust work in §3.
