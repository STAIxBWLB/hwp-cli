import OAuthProvider from '@cloudflare/workers-oauth-provider';

import { googleHandler } from './google-handler';
import { mcpApiHandler } from './mcp-api';
import { handlePat, isPat } from './pat';
import type { Env } from './types';

export { HwpSession } from './session';

/**
 * hwp MCP — the Cloudflare tier of docs/design/22-remote-mcp-deployment.md.
 *
 * The Worker is this service's OAuth authorization server, not just a resource
 * server: it registers clients dynamically, runs the consent screen, and issues
 * its own tokens, with Google as the upstream identity provider. That is what
 * lets any MCP client connect without an administrator provisioning anything.
 */
const provider = new OAuthProvider<Env>({
  apiRoute: ['/mcp', '/files/'],
  apiHandler: mcpApiHandler,
  defaultHandler: googleHandler,
  authorizeEndpoint: '/authorize',
  tokenEndpoint: '/token',
  clientRegistrationEndpoint: '/register',
  scopesSupported: ['mcp:tools'],
});

export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> | Response {
    // Personal access tokens bypass the OAuth machinery and reach the same MCP
    // handler with the same principal shape (see pat.ts).
    if (isPat(request.headers.get('authorization'))) {
      return handlePat(request, env, ctx);
    }
    return provider.fetch(request, env, ctx);
  },
} satisfies ExportedHandler<Env>;
