# hwp MCP on Cloudflare

The Cloudflare tier of [docs/design/22-remote-mcp-deployment.md](../../docs/design/22-remote-mcp-deployment.md):
a public MCP service where anyone signs in with a Google account and drives the
20 hwp tools from any MCP client.

A Worker is the public edge — it is this service's OAuth **authorization server**,
not merely a resource server, so clients register themselves and no administrator
provisions anything. Each session gets its own container instance running
`hwp serve --root /work`, and the workspace dies with it.

```
MCP client ──HTTPS──► Worker ──► Durable Object (one per session) ──► container: hwp serve
                      OAuth + Google, PAT auth, limits, audit          --root /work, no egress
```

## Owner prerequisites

These four cannot be done by an agent. Everything else in this directory is ready
to deploy once they exist.

1. **Workers Paid** ($5/month) on the Cloudflare account. Containers require it.
2. **A Cloudflare API token** for the deploy host, with: Workers Scripts:Edit,
   Workers KV Storage:Edit, D1:Edit, Containers:Edit, Account Settings:Read.
3. **A Google OAuth client** (type: Web application) with the redirect URI
   `https://hwp-mcp.young-joon-lee.workers.dev/callback`. Keep the client id and secret.
4. **A $10/month billing notification** (Dashboard → Notifications). The budget cap
   for this service is $10/month; see *Cost* below.

## Where to deploy from

`wrangler deploy` builds the container image locally, so the deploy host needs a
running Docker engine and an amd64 target. Use the Ubuntu build host rather than a
Mac without Docker:

```bash
ssh yjlee@172.16.229.33
# one-time: install Node 22+ (fnm or the distribution package), clone the repo
cd ~/hwp-cli/deploy/cloudflare
```

Store credentials once, readable only by you:

```bash
install -m 600 /dev/null ~/.config/hwp-mcp-deploy.env
cat > ~/.config/hwp-mcp-deploy.env <<'EOF'
CLOUDFLARE_API_TOKEN=...
CLOUDFLARE_ACCOUNT_ID=...
EOF
```

## Already provisioned

Account `entelecheia` (`b378caab1c7aea09cb77db791fe5f3f8`), workers.dev subdomain
`young-joon-lee`, so the service URL is
`https://hwp-mcp.young-joon-lee.workers.dev`.

These exist and their ids are already in `wrangler.jsonc` — do not recreate them:

| Resource | Id |
|---|---|
| KV namespace `OAUTH_KV` | `6e717629e44a48b4afc8e2b52684cfe9` |
| D1 database `hwp-mcp` | `b8a7e5b9-7c14-4354-91a6-52e5c36d45c8` |

`schema.sql` is applied to the remote database; `users`, `pats` and `audit` are live.

## First deploy

```bash
set -a && . ~/.config/hwp-mcp-deploy.env && set +a
npm ci

# Secrets (never in wrangler.jsonc).
npx wrangler secret put GOOGLE_CLIENT_ID
npx wrangler secret put GOOGLE_CLIENT_SECRET
openssl rand -hex 32 | npx wrangler secret put COOKIE_ENCRYPTION_KEY

npx wrangler deploy
```

The first deploy takes several minutes: it compiles the Rust workspace inside the
image. The Worker URL may answer before container routes do.

## Verifying

```bash
# 1. Protocol, from a machine with a browser.
npx @modelcontextprotocol/inspector
#    Transport: Streamable HTTP → https://hwp-mcp.young-joon-lee.workers.dev/mcp
#    Expect: dynamic registration → Google sign-in → initialize returns
#    Mcp-Session-Id → tools/list shows exactly 20 tools.

# 2. A real client.
claude mcp add --transport http hwp https://hwp-mcp.young-joon-lee.workers.dev/mcp
```

Then check the negatives, which are the parts worth distrusting:

| Check | Expect |
|---|---|
| A second account's token plus the first account's `Mcp-Session-Id` | `404`, revealing nothing |
| A body over 1 MiB on `POST /mcp` | `413` before any parsing |
| `GET /mcp` | `405` with `Allow` — both clients above must tolerate it |
| `DELETE /mcp`, then any call on that session | `204`, then `404` |
| A tool argument pointing outside the workspace | refused, same message the stdio server gives |

After testing, confirm the container count returns to zero in the dashboard.

## What is already verified

The container half was built and exercised on an amd64 host before any Cloudflare
account existed, so the only untested part of this directory is the Worker running
against real Cloudflare services.

| Checked | Result |
|---|---|
| `docker build` from the repo root | 134 MB image; the Rust release build takes well under a minute on a large host |
| `hwp serve` startup | binds and prints `hwp serve: listening on http://0.0.0.0:8080` |
| `GET /healthz`, `POST /mcp` initialize, `tools/list` | 200, correct handshake, exactly 20 tools |
| `GET /mcp` | 405 |
| `/files` upload, download, and a rejected `.hidden` name | 200, byte-identical, 400 |
| `hwp_new` into `/work`, then `GET /files/made.hwpx` | created and downloadable |
| A tool writing outside `/work` | refused, same message the stdio server gives |
| Process identity and fonts | runs as `hwp` (uid 10001); fonts-nanum present |

`wrangler deploy --dry-run` bundles the Worker (about 52 KiB gzipped) and resolves
all four bindings plus the container.

## Cost

Workers Paid is $5/month and includes 25 GiB-hours of container memory, 375 vCPU
minutes and 200 GB-hours of disk. The `basic` instance type is 1 GiB, so roughly
25 hours of *active* container time per month are included; beyond that it is
about $0.009 per hour. Containers scale to zero, so idle sessions cost nothing.

Three guards keep this inside the $10 cap: `sleepAfter` is 10 minutes,
`max_instances` is 5, and the billing notification above fires at $10.

## Layout

| Path | Purpose |
|---|---|
| `src/index.ts` | Entry point: PAT branch, then the OAuth provider; exports the session Durable Object |
| `src/mcp-api.ts` | The authenticated `/mcp` and `/files/{name}` surface; mints and routes sessions |
| `src/session.ts` | `HwpSession` — one container per session, deadlines, idle and lifetime expiry |
| `src/google-handler.ts` | Consent screen, Google round trip, and the callback where first login becomes signup |
| `src/pat.ts` | Personal access tokens for header-configured clients |
| `src/users.ts` | The user upsert keyed on the Google `sub` claim |
| `src/audit.ts` | Metadata-only audit writes |
| `src/limits.ts` | Every limit in one place, mirroring doc 20 §7 |
| `container/Dockerfile` | Builds `hwp` and runs `hwp serve` |
| `schema.sql` | D1 tables |

## Known scope

This tier uses the **session workspace** model, not the artifact model of
[doc 20 §3.2](../../docs/design/20-remote-mcp.md). Files live in one container's
private workspace and are destroyed with it, so a client that loses its session
re-uploads. The amendment is recorded in doc 22 §7; the artifact model stays
required and unimplemented.

Also deliberately absent for now: SSE and resumable streams, per-request
cancellation (a blown deadline ends the session), more than one concurrent job per
session, scopes finer than `mcp:tools`, a strict origin allowlist, and the
independent security review doc 20 §9 item 5 requires before general availability.
