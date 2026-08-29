import type { Env } from './types';

export interface GoogleIdentity {
  sub: string;
  email: string;
  name?: string;
}

/**
 * Upserts the user keyed on the Google `sub` claim and returns our own user id.
 *
 * This is the whole of "signup": the first successful Google login creates the
 * record, so there is no separate registration step to build or to explain.
 */
export async function upsertUser(env: Env, identity: GoogleIdentity): Promise<string> {
  const now = Date.now();
  const existing = await env.DB.prepare('SELECT id FROM users WHERE google_sub = ?')
    .bind(identity.sub)
    .first<{ id: string }>();

  if (existing) {
    await env.DB.prepare('UPDATE users SET email = ?, name = ?, last_login_at = ? WHERE id = ?')
      .bind(identity.email, identity.name ?? null, now, existing.id)
      .run();
    return existing.id;
  }

  const id = crypto.randomUUID();
  await env.DB.prepare(
    `INSERT INTO users (id, google_sub, email, name, created_at, last_login_at)
     VALUES (?, ?, ?, ?, ?, ?)`,
  )
    .bind(id, identity.sub, identity.email, identity.name ?? null, now, now)
    .run();
  return id;
}
