import { writeAudit } from './audit';
import { FILE_NAME_RE, MAX_BODY_BYTES } from './limits';
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

/** A session id is opaque and server-minted; reject anything that is not ours. */
const SESSION_ID_RE = /^[0-9a-f-]{36}$/;

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

/** `fetch` is required (not optional) so this satisfies the provider's handler type. */
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
    if (!headerSessionId || !SESSION_ID_RE.test(headerSessionId)) {
      return new Response('session not found', { status: 404 });
    }
    await sessionStub(env, principal, headerSessionId).terminate();
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
    sessionId = crypto.randomUUID();
    minted = true;
  } else {
    if (!headerSessionId || !SESSION_ID_RE.test(headerSessionId)) {
      return new Response('session not found', { status: 404 });
    }
    sessionId = headerSessionId;
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
  const sessionId = request.headers.get('mcp-session-id');
  if (!sessionId || !SESSION_ID_RE.test(sessionId)) {
    return new Response('session not found', { status: 404 });
  }
  const name = url.pathname.slice('/files/'.length);
  if (!FILE_NAME_RE.test(name)) {
    return new Response('invalid file name', { status: 400 });
  }
  if (request.method !== 'GET' && request.method !== 'POST') {
    return new Response(null, { status: 405, headers: { Allow: 'GET, POST' } });
  }

  const stub = sessionStub(env, principal, sessionId);
  if (!(await stub.isUsable())) {
    return new Response('session not found', { status: 404 });
  }

  const forwarded = new Request(`http://container/files/${name}`, {
    method: request.method,
    body: request.method === 'POST' ? request.body : undefined,
  });
  return stub.proxy(forwarded, null);
}
