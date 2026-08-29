/**
 * Audit records carry metadata only — no tool arguments, no paths, no document
 * content, no tokens (docs/design/20-remote-mcp.md §7).
 */

export interface AuditEvent {
  requestId: string | null;
  userId: string | null;
  /** Raw session id; hashed here so the id itself never reaches storage. */
  sessionId: string | null;
  tool: string | null;
  outcome: 'ok' | 'tool_error' | 'rejected' | 'timeout';
  durationMs: number | null;
  bytesIn: number | null;
  bytesOut: number | null;
}

export async function sha256Hex(input: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(input));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

export async function writeAudit(db: D1Database, event: AuditEvent): Promise<void> {
  const sessionRef = event.sessionId ? await sha256Hex(event.sessionId) : null;
  try {
    await db
      .prepare(
        `INSERT INTO audit (ts, request_id, user_id, session_ref, tool, outcome, duration_ms, bytes_in, bytes_out)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .bind(
        Date.now(),
        event.requestId,
        event.userId,
        sessionRef,
        event.tool,
        event.outcome,
        event.durationMs,
        event.bytesIn,
        event.bytesOut,
      )
      .run();
  } catch (error) {
    // An audit write must never take down a tool call that already succeeded.
    console.error('audit write failed', error instanceof Error ? error.message : String(error));
  }
}
