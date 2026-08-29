import { writeAudit } from './audit';
import { sign, verify } from './cookies';
import { FILE_NAME_RE, MAX_BODY_BYTES, MAX_FILE_TRANSFER_BYTES } from './limits';
import type { Env, Principal } from './types';

/**
 * The authenticated MCP surface.
 *
 * Everything before this point established *who* is calling. This module decides
 * *which session* they may talk to and forwards the raw JSON-RPC body to the
 * container running `hwp serve`. It never interprets the protocol itself: the
 * Rust core owns initialize, tools/list and tools/call, so the two transports
 * cannot drift apart.
 */

function principalOf(ctx: ExecutionContext): Principal | null {
  const props = (ctx as ExecutionContext & { props?: Principal }).props;
  return props && typeof props.userId === 'string' ? props : null;
}

/**
 * A session id is opaque and server-minted: a random uuid carrying an HMAC of
 * itself, so the edge can tell a real id from an invented one without asking
 * storage.
 *
 * The check is not cosmetic. Reaching the Durable Object at all is what starts a
 * container, and it starts even on the path that answers 404 - so validating the
 * id only inside the object let anyone signed in burn a container per made-up id
 * and pin the whole instance ceiling. Verifying the MAC here means a forged id
 * never touches the object.
 */
const SESSION_ID_RE = /^[0-9a-f-]{36}\.[A-Za-z0-9_-]{43}$/;

async function mintSessionId(env: Env): Promise<string> {
  return sign(env.COOKIE_ENCRYPTION_KEY, crypto.randomUUID());
}

/** Returns the id when this service minted it, and null otherwise. */
async function ourSessionId(env: Env, sessionId: string | null): Promise<string | null> {
  if (!sessionId || !SESSION_ID_RE.test(sessionId)) return null;
  return (await verify(env.COOKIE_ENCRYPTION_KEY, sessionId)) === null ? null : sessionId;
}

function sessionStub(env: Env, principal: Principal, sessionId: string) {
  // The principal is part of the name, so another user presenting this session id
  // addresses a different object that was never initialized — and gets a plain
  // 404 that reveals nothing about who owns the id.
  const id = env.HWP_SESSION.idFromName(`${principal.userId}:${sessionId}`);
  return env.HWP_SESSION.get(id);
}

/** Reads the body once, refusing anything over the cap before parsing it. */
async function readBody(request: Request): Promise<{ bytes: ArrayBuffer } | { tooLarge: true }> {
  const declared = request.headers.get('content-length');
  if (declared !== null && Number(declared) > MAX_BODY_BYTES) return { tooLarge: true };
  const bytes = await request.arrayBuffer();
  if (bytes.byteLength > MAX_BODY_BYTES) return { tooLarge: true };
  return { bytes };
}

/** Extracts the JSON-RPC method and tool name without validating the document. */
function describe(body: ArrayBuffer): { method: string | null; tool: string | null } {
  try {
    const parsed = JSON.parse(new TextDecoder().decode(body)) as {
      method?: unknown;
      params?: { name?: unknown };
    };
    const method = typeof parsed.method === 'string' ? parsed.method : null;
    const tool = typeof parsed.params?.name === 'string' ? parsed.params.name : null;
    return { method, tool };
  } catch {
    return { method: null, tool: null };
  }
}

/**
 * DNS-rebinding defense (docs/design/20-remote-mcp.md §2.2).
 *
 * Absent Origin is allowed, and that is the documented policy rather than an
 * oversight: MCP clients are not browsers and send no Origin. A *present* Origin
 * must be this deployment's own, because nothing here is meant to be called from
 * another site's page.
 */
function originAllowed(request: Request, url: URL): boolean {
  const origin = request.headers.get('origin');
  return origin === null || origin === url.origin;
}

/** `fetch` is required (not optional) so this satisfies the provider's handler type. */
export const mcpApiHandler = {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const principal = principalOf(ctx);
    if (!principal) {
      // Reaching here without props means the composition in index.ts is wrong,
      // not that the caller did anything. Fail closed rather than guess.
      return new Response('unauthenticated', { status: 401 });
    }

    const url = new URL(request.url);
    if (!originAllowed(request, url)) {
      return new Response('origin not allowed', { status: 403 });
    }
    if (url.pathname === '/mcp') return handleMcp(request, env, ctx, principal);
    if (url.pathname.startsWith('/files/')) return handleFiles(request, env, principal, url);
    return new Response('not found', { status: 404 });
  },
};

async function handleMcp(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
  principal: Principal,
): Promise<Response> {
  const headerSessionId = request.headers.get('mcp-session-id');
  const requestId = request.headers.get('cf-ray');

  if (request.method === 'DELETE') {
    const known = await ourSessionId(env, headerSessionId);
    if (!known) {
      return new Response('session not found', { status: 404 });
    }
    await sessionStub(env, principal, known).terminate();
    return new Response(null, { status: 204 });
  }

  if (request.method !== 'POST') {
    // No server-to-client stream is offered, which the transport spec permits.
    return new Response(null, { status: 405, headers: { Allow: 'POST, DELETE' } });
  }

  const body = await readBody(request);
  if ('tooLarge' in body) {
    ctx.waitUntil(
      writeAudit(env.DB, {
        requestId,
        userId: principal.userId,
        sessionId: headerSessionId,
        tool: null,
        outcome: 'rejected',
        durationMs: null,
        bytesIn: null,
        bytesOut: null,
      }),
    );
    return new Response('request body too large', { status: 413 });
  }

  const { method, tool } = describe(body.bytes);

  // `initialize` mints the session; every other call must present one.
  let sessionId: string;
  let minted = false;
  if (method === 'initialize') {
    // The budget guard sits here rather than on the edge as a whole: this is the
    // one call that starts a container, and container time is what the monthly
    // cap actually buys. Ordinary traffic inside a session is far cheaper and
    // gets the looser limit below.
    const { success } = await env.SESSION_LIMITER.limit({ key: principal.userId });
    if (!success) {
      return new Response('too many sessions started; wait a minute', {
        status: 429,
        headers: { 'retry-after': '60' },
      });
    }
    sessionId = await mintSessionId(env);
    minted = true;
  } else {
    const { success } = await env.CALL_LIMITER.limit({ key: principal.userId });
    if (!success) {
      return new Response('too many requests', {
        status: 429,
        headers: { 'retry-after': '60' },
      });
    }
    const known = await ourSessionId(env, headerSessionId);
    if (!known) {
      return new Response('session not found', { status: 404 });
    }
    sessionId = known;
  }

  const stub = sessionStub(env, principal, sessionId);
  if (minted) {
    await stub.begin();
  } else if (!(await stub.isUsable())) {
    return new Response('session not found', { status: 404 });
  }

  const started = Date.now();
  const forwarded = new Request('http://container/mcp', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: body.bytes,
  });
  const response = await stub.proxy(forwarded, tool);

  const out = await response.arrayBuffer();
  ctx.waitUntil(
    writeAudit(env.DB, {
      requestId,
      userId: principal.userId,
      sessionId,
      tool: tool ?? method,
      outcome: response.ok ? 'ok' : response.status === 504 ? 'timeout' : 'tool_error',
      durationMs: Date.now() - started,
      bytesIn: body.bytes.byteLength,
      bytesOut: out.byteLength,
    }),
  );

  const headers = new Headers(response.headers);
  if (minted) headers.set('Mcp-Session-Id', sessionId);
  return new Response(out.byteLength === 0 ? null : out, {
    status: response.status,
    headers,
  });
}

/**
 * Workspace file transfer. This is the session-workspace model of doc 22 §7, not
 * the artifact model of doc 20 §3.2: the bytes live in the container's private
 * workspace and die with it.
 */
async function handleFiles(
  request: Request,
  env: Env,
  principal: Principal,
  url: URL,
): Promise<Response> {
  const sessionId = await ourSessionId(env, request.headers.get('mcp-session-id'));
  if (!sessionId) {
    return new Response('session not found', { status: 404 });
  }
  const name = url.pathname.slice('/files/'.length);
  if (!FILE_NAME_RE.test(name)) {
    return new Response('invalid file name', { status: 400 });
  }
  if (request.method !== 'GET' && request.method !== 'POST') {
    return new Response(null, { status: 405, headers: { Allow: 'GET, POST' } });
  }

  const { success } = await env.CALL_LIMITER.limit({ key: principal.userId });
  if (!success) {
    return new Response('too many requests', { status: 429, headers: { 'retry-after': '60' } });
  }

  const stub = sessionStub(env, principal, sessionId);
  if (!(await stub.isUsable())) {
    return new Response('session not found', { status: 404 });
  }

  // Both directions are buffered rather than streamed. A ReadableStream sent
  // across the Durable Object RPC boundary disconnects partway through, which
  // surfaced as an intermittently failing upload, so the bytes are materialized
  // on this side and handed over whole.
  let body: ArrayBuffer | undefined;
  if (request.method === 'POST') {
    const declared = request.headers.get('content-length');
    if (declared !== null && Number(declared) > MAX_FILE_TRANSFER_BYTES) {
      return new Response('file too large', { status: 413 });
    }
    body = await request.arrayBuffer();
    if (body.byteLength > MAX_FILE_TRANSFER_BYTES) {
      return new Response('file too large', { status: 413 });
    }
  }

  const forwarded = new Request(`http://container/files/${name}`, {
    method: request.method,
    body,
  });
  const response = await stub.proxy(forwarded, null);
  const out = await response.arrayBuffer();
  return new Response(out.byteLength === 0 ? null : out, {
    status: response.status,
    headers: response.headers,
  });
}
