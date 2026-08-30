import { readCookie, setCookie, sign, verify } from './cookies';
import { handleDashboard, startSession } from './dashboard';
import { MCP_SCOPE } from './limits';
import { upsertUser, type GoogleIdentity } from './users';
import type { Env, Principal } from './types';
import type { AuthRequest } from '@cloudflare/workers-oauth-provider';

/**
 * The browser-facing half of the service: the consent screen, the round trip to
 * Google, and the callback that turns a Google identity into one of our users.
 *
 * Google is only the upstream identity provider. The token an MCP client ends up
 * holding is one this Worker issued, which is what the MCP 2025-06-18
 * third-party authorization flow requires — a Google access token must never
 * reach the client or the document tools.
 */

const APPROVED_COOKIE = 'hwp_approved';
const APPROVAL_TTL_SECONDS = 60 * 60 * 24 * 90;
const STATE_TTL_SECONDS = 600;

const GOOGLE_AUTHORIZE = 'https://accounts.google.com/o/oauth2/v2/auth';
const GOOGLE_TOKEN = 'https://oauth2.googleapis.com/token';

function html(body: string, status = 200): Response {
  return new Response(
    `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">` +
      `<link rel="stylesheet" href="/style.css"><title>hwp MCP</title><main>${body}</main>`,
    { status, headers: { 'content-type': 'text/html; charset=utf-8' } },
  );
}

function escape(text: string): string {
  return text.replace(
    /[&<>"']/g,
    (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!,
  );
}

/** Decodes a JWT payload. Signature verification is Google's job: the id_token
 *  arrives over TLS on our own back-channel token exchange, not from the client. */
function decodeIdToken(idToken: string): GoogleIdentity | null {
  const parts = idToken.split('.');
  if (parts.length !== 3) return null;
  try {
    const json = atob(parts[1]!.replace(/-/g, '+').replace(/_/g, '/'));
    const claims = JSON.parse(json) as { sub?: string; email?: string; name?: string };
    if (!claims.sub || !claims.email) return null;
    return { sub: claims.sub, email: claims.email, name: claims.name };
  } catch {
    return null;
  }
}

function redirectToGoogle(env: Env, url: URL, state: string): Response {
  const target = new URL(GOOGLE_AUTHORIZE);
  target.searchParams.set('client_id', env.GOOGLE_CLIENT_ID);
  target.searchParams.set('redirect_uri', `${url.origin}/callback`);
  target.searchParams.set('response_type', 'code');
  target.searchParams.set('scope', 'openid email profile');
  target.searchParams.set('state', state);
  target.searchParams.set('access_type', 'online');
  if (env.HOSTED_DOMAIN) target.searchParams.set('hd', env.HOSTED_DOMAIN);
  return Response.redirect(target.toString(), 302);
}

/** Stores the in-flight authorization request; the state parameter is its key. */
async function stashAuthRequest(env: Env, authRequest: AuthRequest): Promise<string> {
  const state = crypto.randomUUID();
  await env.OAUTH_KV.put(`state:${state}`, JSON.stringify(authRequest), {
    expirationTtl: STATE_TTL_SECONDS,
  });
  return state;
}

async function takeAuthRequest(env: Env, state: string | null): Promise<AuthRequest | null> {
  if (!state) return null;
  const stored = await env.OAUTH_KV.get(`state:${state}`);
  if (!stored) return null;
  await env.OAUTH_KV.delete(`state:${state}`);
  return JSON.parse(stored) as AuthRequest;
}

export const googleHandler: ExportedHandler<Env> = {
  async fetch(request, env, _ctx): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === '/authorize') return authorize(request, env, url);
    if (url.pathname === '/callback') return callback(request, env, url);
    if (url.pathname === '/dashboard/login') return dashboardLogin(env, url);
    if (url.pathname.startsWith('/dashboard')) return handleDashboard(request, env, url);
    if (url.pathname === '/') return home();
    return env.ASSETS.fetch(request);
  },
};

/**
 * Sign-in for the dashboard.
 *
 * Reuses the OAuth path's Google round trip rather than adding a second one; the
 * stored state carries a `dashboard` marker so `callback` knows to set a session
 * cookie instead of completing an authorization nobody requested.
 */
async function dashboardLogin(env: Env, url: URL): Promise<Response> {
  const state = crypto.randomUUID();
  await env.OAUTH_KV.put(`state:${state}`, JSON.stringify({ dashboard: true }), {
    expirationTtl: STATE_TTL_SECONDS,
  });
  return redirectToGoogle(env, url, state);
}

async function authorize(request: Request, env: Env, url: URL): Promise<Response> {
  const authRequest = await env.OAUTH_PROVIDER.parseAuthRequest(request);
  const client = await env.OAUTH_PROVIDER.lookupClient(authRequest.clientId);
  const state = await stashAuthRequest(env, authRequest);

  // A client the user already approved skips straight to Google.
  const approved = await verify(env.COOKIE_ENCRYPTION_KEY, readCookie(request, APPROVED_COOKIE));
  if (approved === authRequest.clientId || request.method === 'POST') {
    const response = redirectToGoogle(env, url, state);
    if (request.method === 'POST') {
      const headers = new Headers(response.headers);
      headers.append(
        'Set-Cookie',
        setCookie(
          APPROVED_COOKIE,
          await sign(env.COOKIE_ENCRYPTION_KEY, authRequest.clientId),
          APPROVAL_TTL_SECONDS,
        ),
      );
      return new Response(null, { status: 302, headers });
    }
    return response;
  }

  const name = escape(client?.clientName ?? authRequest.clientId);
  return html(
    `<h1>Authorize ${name}</h1>
     <p><strong>${name}</strong> wants to use your hwp documents through this service.
        It will be able to read, convert, render and edit files inside its own
        temporary workspace.</p>
     <p>Signing in with Google creates your account if you do not have one yet.</p>
     <form method="post"><button type="submit">Continue with Google</button></form>`,
  );
}

async function callback(request: Request, env: Env, url: URL): Promise<Response> {
  const code = url.searchParams.get('code');
  const stored = await takeAuthRequest(env, url.searchParams.get('state'));
  const toDashboard = (stored as { dashboard?: boolean } | null)?.dashboard === true;
  const authRequest = toDashboard ? null : stored;
  if (!code || (!authRequest && !toDashboard)) {
    return html('<h1>Sign-in failed</h1><p>The request expired. Start again from your client.</p>', 400);
  }

  const exchange = await fetch(GOOGLE_TOKEN, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      code,
      client_id: env.GOOGLE_CLIENT_ID,
      client_secret: env.GOOGLE_CLIENT_SECRET,
      redirect_uri: `${url.origin}/callback`,
      grant_type: 'authorization_code',
    }),
  });
  if (!exchange.ok) {
    // Log Google's own error code. Without it the only signal is "rejected", which is
    // indistinguishable between a wrong secret, a redirect_uri mismatch and a code
    // already spent -- three very different fixes. The body carries no user data and
    // no token: on failure Google returns {error, error_description} only.
    const detail = await exchange.text().catch(() => '');
    console.error('google token exchange failed', exchange.status, detail.slice(0, 300));
    // Google's `error` field is a short fixed enum -- invalid_client, invalid_grant,
    // redirect_uri_mismatch and so on -- and carries no secret. Showing it turns four
    // indistinguishable failures into one that names its own cause, which matters
    // because the alternative is reading Worker logs that need a token scope the
    // deploy credentials do not have.
    let reason = '';
    try {
      const parsed = JSON.parse(detail) as { error?: unknown };
      const description =
        typeof (parsed as { error_description?: unknown }).error_description === 'string'
          ? (parsed as { error_description: string }).error_description
          : '';
      if (typeof parsed.error === 'string' && /^[a-z_]{1,40}$/.test(parsed.error)) {
        // The description names the offending parameter, which is the whole point:
        // invalid_request alone does not say *which* field Google refused.
        reason = description
          ? ` (${parsed.error}: ${description.slice(0, 200)})`
          : ` (${parsed.error})`;
      }
    } catch {
      // A non-JSON body means Google is not answering the way it documents; the
      // status alone is then the only honest signal.
      reason = ` (HTTP ${exchange.status})`;
    }
    return html(`<h1>Sign-in failed</h1><p>Google rejected the exchange${escape(reason)}.</p>`, 502);
  }

  const tokens = (await exchange.json()) as { id_token?: string };
  const identity = tokens.id_token ? decodeIdToken(tokens.id_token) : null;
  if (!identity) {
    return html('<h1>Sign-in failed</h1><p>Google returned no usable identity.</p>', 502);
  }
  if (env.HOSTED_DOMAIN && !identity.email.endsWith(`@${env.HOSTED_DOMAIN}`)) {
    return html('<h1>Sign-in refused</h1><p>This service is restricted to one domain.</p>', 403);
  }

  // First login is signup, whichever door it came through.
  const userId = await upsertUser(env, identity);

  if (toDashboard) {
    return new Response(null, {
      status: 302,
      headers: { location: `${url.origin}/dashboard`, 'set-cookie': await startSession(env, userId) },
    });
  }

  const props: Principal = { userId, email: identity.email, via: 'oauth' };
  const { redirectTo } = await env.OAUTH_PROVIDER.completeAuthorization({
    request: authRequest!,
    userId,
    metadata: { email: identity.email },
    scope: [MCP_SCOPE],
    props,
  });
  return Response.redirect(redirectTo, 302);
}

function home(): Response {
  // Google's verification review reads this page: it wants a homepage on the verified
  // domain that says what the app does and links the privacy policy. It is also the
  // first thing a person sees, so it says what the service is for, not just that it
  // exists.
  return html(
    `<h1>hwp MCP</h1>
     <p>An MCP server for HWP and HWPX documents. It reads, renders, converts, edits
        and creates Korean word-processor files, so an AI assistant can work with them
        directly instead of asking you to open Hangul.</p>
     <p>Point an MCP client at <code>/mcp</code> and sign in with Google when it asks.
        For a client that needs a fixed <code>Authorization</code> header instead, sign
        in at <a href="/dashboard">the dashboard</a> and create a token there.</p>
     <p>Each session gets its own container with no network access, and it is destroyed
        when the session ends. What is stored, and what deliberately is not, is set out
        in the <a href="/privacy">privacy policy</a>.</p>
     <p>Operated by Young Joon Lee. Contact
        <a href="mailto:yj.lee@chu.ac.kr">yj.lee@chu.ac.kr</a>.</p>`,
  );
}
