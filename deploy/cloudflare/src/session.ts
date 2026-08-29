import { Container } from '@cloudflare/containers';

import {
  IDLE_SLEEP_AFTER,
  MAX_SESSION_MS,
  SLOW_TOOLS,
  SLOW_TOOL_DEADLINE_MS,
  TOOL_DEADLINE_MS,
} from './limits';
import type { Env } from './types';

/**
 * One MCP session, backed by one container instance holding one workspace.
 *
 * Isolation comes from the platform, not from code here: the object name is
 * derived from the authenticated principal plus a server-minted session id, so
 * presenting someone else's session id resolves to a different object that was
 * never initialized. `hwp serve --root /work` confines file access inside the
 * container on top of that.
 *
 * A session is single-use in the strong sense: once the container stops for any
 * reason — idle, deadline, crash, eviction, explicit delete — the workspace is
 * gone, so the session is marked dead and every later request gets a 404. The
 * client reinitializes and re-uploads. This is a deliberate simplification over
 * doc 20 §3.2's artifact model, recorded in doc 22 §7.
 */
export class HwpSession extends Container<Env> {
  defaultPort = 8080;
  sleepAfter = IDLE_SLEEP_AFTER;
  /** Document workers get no network egress (doc 20 §6). */
  enableInternet = false;
  envVars = { HWP_LANG: 'en' };

  /**
   * Serializes forwarded requests. `hwp serve` handles one request at a time, so
   * overlapping fetches would only queue inside the container; keeping the queue
   * here makes the deadline apply per request rather than per pile-up.
   */
  private chain: Promise<unknown> = Promise.resolve();

  /** Marks this session unusable. Called on every path that destroys the workspace. */
  private async markDead(): Promise<void> {
    await this.ctx.storage.put('dead', true);
  }

  async isDead(): Promise<boolean> {
    return (await this.ctx.storage.get<boolean>('dead')) === true;
  }

  /**
   * True only for a session that `initialize` actually created and that is still
   * alive.
   *
   * Asking "is it dead?" is not enough: a never-initialized object has no dead
   * flag either, so a caller who invents a well-formed session id would sail
   * through and start a fresh container. The id must correspond to a session
   * this service minted.
   */
  async isUsable(): Promise<boolean> {
    if (await this.isDead()) return false;
    return (await this.ctx.storage.get<number>('started')) !== undefined;
  }

  /** Records the creation time and arms the maximum-lifetime alarm. */
  async begin(): Promise<void> {
    const started = await this.ctx.storage.get<number>('started');
    if (started === undefined) {
      await this.ctx.storage.put('started', Date.now());
      await this.ctx.storage.setAlarm(Date.now() + MAX_SESSION_MS);
    }
  }

  /** Proxies one request to the container under the deadline for `tool`. */
  async proxy(request: Request, tool: string | null): Promise<Response> {
    if (await this.isDead()) {
      return new Response('session not found', { status: 404 });
    }
    const deadline = tool && SLOW_TOOLS.has(tool) ? SLOW_TOOL_DEADLINE_MS : TOOL_DEADLINE_MS;

    const run = this.chain.then(() => this.forward(request, deadline));
    // Keep the chain alive even when this call rejects, or one failure would
    // wedge every later request on the same session.
    this.chain = run.catch(() => undefined);
    return run;
  }

  private async forward(request: Request, deadlineMs: number): Promise<Response> {
    // `containerFetch` takes no options object, so the deadline rides on the
    // request itself.
    const deadlined = new Request(request, { signal: AbortSignal.timeout(deadlineMs) });
    try {
      return await this.containerFetch(deadlined);
    } catch (error) {
      // A blown deadline ends the session: the container may still be mid-write,
      // and the only guarantee worth keeping is that nothing partial survives.
      // ponytail: deadline kills the session; per-request cancellation when a
      // real user hits this.
      await this.terminate();
      const timedOut = error instanceof Error && error.name === 'TimeoutError';
      return new Response(timedOut ? 'tool deadline exceeded' : 'container unavailable', {
        status: timedOut ? 504 : 502,
      });
    }
  }

  /** Idempotent: stops the container, kills the alarm, and marks the session dead. */
  async terminate(): Promise<void> {
    await this.markDead();
    await this.ctx.storage.deleteAlarm();
    try {
      await this.destroy();
    } catch {
      // Already gone; the dead flag is what callers read.
    }
  }

  override async alarm(): Promise<void> {
    await this.terminate();
  }

  override onStop(): void | Promise<void> {
    // Idle sleep, crash and eviction all land here. The workspace died with the
    // instance, so the session cannot be resumed.
    return this.markDead();
  }

  override onError(error: unknown): unknown {
    void this.markDead();
    return error;
  }
}
