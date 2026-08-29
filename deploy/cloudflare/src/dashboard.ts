import { sha256Hex } from './audit';
import { readCookie, setCookie, sign, verify } from './cookies';
import { PAT_PREFIX } from './limits';
import type { Env } from './types';

/**
 * The dashboard: where a person signs in with Google and mints a personal access
 * token for MCP clients that carry a static `Authorization` header instead of
 * running an OAuth flow.
 *
 * Sign-in reuses the same Google round trip the OAuth path uses. The difference
 * is only where it lands: `/callback` sees a `dash` state and sets a session
 * cookie here rather than completing an authorization it was never given.
 */

export const SESSION_COOKIE = 'hwp_dash';
const SESSION_TTL_SECONDS = 60 * 60 * 12;
const CSRF_COOKIE = 'hwp_csrf';

/** Full token shown once; only its SHA-256 is stored. */
function mintToken(): string {
  const raw = crypto.getRandomValues(new Uint8Array(32));
  const b64 = btoa(String.fromCharCode(...raw))
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
  return `${PAT_PREFIX}${b64}`;
}

export async function currentUser(request: Request, env: Env): Promise<string | null> {
  return verify(env.COOKIE_ENCRYPTION_KEY, readCookie(request, SESSION_COOKIE));
}

export async function startSession(env: Env, userId: string): Promise<string> {
  return setCookie(
    SESSION_COOKIE,
    await sign(env.COOKIE_ENCRYPTION_KEY, userId),
    SESSION_TTL_SECONDS,
  );
}

function escape(text: string): string {
  return text.replace(
    /[&<>"']/g,
    (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!,
  );
}

function page(body: string, extraHeaders: Record<string, string> = {}): Response {
  return new Response(
    `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">` +
      `<link rel="stylesheet" href="/style.css"><title>hwp MCP tokens</title><main>${body}</main>`,
    { headers: { 'content-type': 'text/html; charset=utf-8', ...extraHeaders } },
  );
}

interface PatRow {
  id: string;
  label: string | null;
  created_at: number;
  last_used_at: number | null;
}

function when(ms: number | null): string {
  return ms ? new Date(ms).toISOString().slice(0, 16).replace('T', ' ') : 'never';
}

async function render(env: Env, userId: string, csrf: string, fresh?: string): Promise<string> {
  const { results } = await env.DB.prepare(
    `SELECT id, label, created_at, last_used_at FROM pats
      WHERE user_id = ? AND revoked_at IS NULL ORDER BY created_at DESC`,
  )
    .bind(userId)
    .all<PatRow>();

  const rows = (results ?? [])
    .map(
      (r) => `<tr>
        <td>${escape(r.label ?? '(no label)')}</td>
        <td>${when(r.created_at)}</td>
        <td>${when(r.last_used_at)}</td>
        <td><form method="post" action="/dashboard/revoke">
          <input type="hidden" name="csrf" value="${csrf}">
          <input type="hidden" name="id" value="${escape(r.id)}">
          <button type="submit">Revoke</button>
        </form></td>
      </tr>`,
    )
    .join('');

  // Shown once, on the response that created it. It is never stored in plaintext,
  // so a reload cannot bring it back.
  const banner = fresh
    ? `<div class="token">
         <p><strong>Copy this now.</strong> It is not shown again.</p>
         <code>${escape(fresh)}</code>
         <p>Point a client at it with:</p>
         <code>claude mcp add --transport http hwp https://hwp-mcp.staix.workers.dev/mcp --header "Authorization: Bearer ${escape(fresh)}"</code>
       </div>`
    : '';

  return `<h1>Access tokens</h1>
    <p>Use a token when an MCP client needs a fixed <code>Authorization</code> header
       instead of signing in through a browser. Everything else should just use the
       OAuth flow at <code>/mcp</code>.</p>
    ${banner}
    <form method="post" action="/dashboard/create">
      <input type="hidden" name="csrf" value="${csrf}">
      <input name="label" placeholder="What is this token for?" maxlength="60">
      <button type="submit">Create token</button>
    </form>
    ${
      rows
        ? `<table><thead><tr><th>Label</th><th>Created</th><th>Last used</th><th></th></tr></thead><tbody>${rows}</tbody></table>`
        : '<p class="empty">No tokens yet.</p>'
    }`;
}

/** Double-submit CSRF: the cookie must match the field on every state change. */
async function csrfToken(request: Request, env: Env): Promise<{ value: string; header?: string }> {
  const existing = await verify(env.COOKIE_ENCRYPTION_KEY, readCookie(request, CSRF_COOKIE));
  if (existing) return { value: existing };
  const value = crypto.randomUUID();
  return {
    value,
    header: setCookie(CSRF_COOKIE, await sign(env.COOKIE_ENCRYPTION_KEY, value), SESSION_TTL_SECONDS),
  };
}

async function checkCsrf(request: Request, env: Env, form: FormData): Promise<boolean> {
  const cookie = await verify(env.COOKIE_ENCRYPTION_KEY, readCookie(request, CSRF_COOKIE));
  const field = form.get('csrf');
  return typeof field === 'string' && cookie !== null && field === cookie;
}

export async function handleDashboard(request: Request, env: Env, url: URL): Promise<Response> {
  const userId = await currentUser(request, env);
  if (!userId) {
    // Not signed in: bounce through the same Google round trip the OAuth path uses.
    return Response.redirect(`${url.origin}/dashboard/login`, 302);
  }

  const csrf = await csrfToken(request, env);
  const headers: Record<string, string> = {};
  if (csrf.header) headers['set-cookie'] = csrf.header;

  if (request.method === 'GET' && url.pathname === '/dashboard') {
    return page(await render(env, userId, csrf.value), headers);
  }

  if (request.method === 'POST') {
    const form = await request.formData();
    if (!(await checkCsrf(request, env, form))) {
      return new Response('bad csrf token', { status: 403 });
    }

    if (url.pathname === '/dashboard/create') {
      const token = mintToken();
      const label = (form.get('label') as string | null)?.slice(0, 60) || null;
      await env.DB.prepare(
        'INSERT INTO pats (id, user_id, token_hash, label, created_at) VALUES (?, ?, ?, ?, ?)',
      )
        .bind(crypto.randomUUID(), userId, await sha256Hex(token), label, Date.now())
        .run();
      return page(await render(env, userId, csrf.value, token), headers);
    }

    if (url.pathname === '/dashboard/revoke') {
      const id = form.get('id');
      if (typeof id === 'string') {
        // Scoped to the owner, so a guessed id cannot revoke someone else's token.
        await env.DB.prepare(
          'UPDATE pats SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL',
        )
          .bind(Date.now(), id, userId)
          .run();
      }
      return page(await render(env, userId, csrf.value), headers);
    }
  }

  return new Response('not found', { status: 404 });
}
