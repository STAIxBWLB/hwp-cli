/**
 * Operational limits, mirroring docs/design/20-remote-mcp.md §7 and doc 22 §3.2.
 *
 * The body cap is one byte below the 1 MiB the container's `hwp serve` enforces,
 * so a request this Worker accepts can never be the one the container rejects.
 */

export const MAX_BODY_BYTES = 1024 * 1024 - 1;

/** Default tool deadline (doc 20 §7). */
export const TOOL_DEADLINE_MS = 120_000;

/** Deadline for rendering, conversion and certification (doc 20 §7). */
export const SLOW_TOOL_DEADLINE_MS = 300_000;

/** Tools that get the longer deadline. */
export const SLOW_TOOLS = new Set(['hwp_render', 'hwp_convert', 'hwp_certify', 'hwp_diff']);

/**
 * Worker-side cap for one `/files` transfer.
 *
 * The container enforces 64 MiB, but the Worker has to hold the bytes in memory:
 * a request body cannot be streamed across the Durable Object RPC boundary
 * without the stream disconnecting mid-transfer, so both directions are
 * buffered. 32 MiB keeps that comfortably inside a Worker's 128 MB budget.
 */
export const MAX_FILE_TRANSFER_BYTES = 32 * 1024 * 1024;

/**
 * Idle lifetime before the container is stopped and the session invalidated.
 *
 * This is the lever that keeps live containers under `max_instances`, not the
 * rate limiter: the limiter caps how fast sessions are *created*, but each one
 * holds an instance for this long, so 5 new sessions a minute against a 10-minute
 * hold reaches ten times the ceiling. Three minutes keeps a working session alive
 * across normal think-time while returning capacity quickly, and it is the same
 * lever that keeps the monthly container-hours bill down.
 */
export const IDLE_SLEEP_AFTER = '3m';

/** Maximum session lifetime; after this the client must reinitialize (doc 20 §7). */
export const MAX_SESSION_MS = 8 * 60 * 60 * 1000;

/**
 * Workspace file names. The charset excludes `/` and a leading `.`, so a name
 * that passes cannot traverse out of the workspace. Percent-escapes are not
 * decoded anywhere in the chain; `hwp serve` applies the identical rule.
 */
export const FILE_NAME_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

/** Personal access token prefix. Recognised before the OAuth provider sees the request. */
export const PAT_PREFIX = 'hwp_pat_';

/** The single scope this service issues. */
export const MCP_SCOPE = 'mcp:tools';
