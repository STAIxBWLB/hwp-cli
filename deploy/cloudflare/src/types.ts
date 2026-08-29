import type { OAuthHelpers } from '@cloudflare/workers-oauth-provider';

import type { HwpSession } from './session';

export interface Env {
  HWP_SESSION: DurableObjectNamespace<HwpSession>;
  OAUTH_KV: KVNamespace;
  DB: D1Database;
  ASSETS: Fetcher;

  /** Caps how often one principal may start a container (see wrangler.jsonc). */
  SESSION_LIMITER: RateLimit;
  /** Caps ordinary traffic once a session exists. */
  CALL_LIMITER: RateLimit;

  GOOGLE_CLIENT_ID: string;
  GOOGLE_CLIENT_SECRET: string;
  COOKIE_ENCRYPTION_KEY: string;
  /** Optional: restrict sign-in to one Google Workspace domain. */
  HOSTED_DOMAIN?: string;

  /** Injected by the OAuth provider on the API handler's env. */
  OAUTH_PROVIDER: OAuthHelpers;
}

/**
 * The authenticated caller. The OAuth flow puts this in `ctx.props`; the
 * personal-access-token path synthesizes the identical shape, so everything
 * downstream has exactly one notion of "who is calling".
 */
export interface Principal {
  userId: string;
  email: string;
  via: 'oauth' | 'pat';
}
