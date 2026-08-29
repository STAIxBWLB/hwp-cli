import { sha256Hex } from './audit';
import { mcpApiHandler } from './mcp-api';
import { PAT_PREFIX } from './limits';
import type { Env, Principal } from './types';

/**
 * Personal access tokens, for MCP clients configured with a static
 * `Authorization` header rather than an OAuth flow.
 *
 * This runs ahead of the OAuth provider and, on a hit, calls the same MCP handler
 * with an equivalent `Principal`. There is therefore exactly one authorization
 * path downstream: nothing in mcp-api.ts knows or cares which door was used.
 */
export function isPat(authorization: string | null): boolean {
  return authorization?.startsWith(`Bearer ${PAT_PREFIX}`) === true;
}

export async function handlePat(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
): Promise<Response> {
  const token = request.headers.get('authorization')!.slice('Bearer '.length);
  const row = await env.DB.prepare(
    `SELECT pats.id AS pat_id, users.id AS user_id, users.email AS email
       FROM pats JOIN users ON users.id = pats.user_id
      WHERE pats.token_hash = ? AND pats.revoked_at IS NULL`,
  )
    .bind(await sha256Hex(token))
    .first<{ pat_id: string; user_id: string; email: string }>();

  if (!row) {
    return new Response('invalid token', {
      status: 401,
      headers: { 'WWW-Authenticate': 'Bearer error="invalid_token"' },
    });
  }

  ctx.waitUntil(
    env.DB.prepare('UPDATE pats SET last_used_at = ? WHERE id = ?')
      .bind(Date.now(), row.pat_id)
      .run()
      .then(() => undefined),
  );

  // Attach the principal the same way the OAuth provider does: by assigning to
  // `ctx.props` on the live context. Cloning the context instead would leave
  // `waitUntil` — which lives on the prototype — behind, and the audit writes in
  // mcp-api.ts depend on it.
  const principal: Principal = { userId: row.user_id, email: row.email, via: 'pat' };
  (ctx as ExecutionContext & { props?: Principal }).props = principal;

  return mcpApiHandler.fetch(request, env, ctx);
}
