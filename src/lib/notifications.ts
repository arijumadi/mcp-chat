import type { CoreEvent } from "@/lib/types";

/**
 * Desktop notifications for a turn that finished, failed, or is waiting for the
 * user while they are looking elsewhere.
 *
 * The decision is pure: given an event and what the user is currently looking
 * at, it returns what to send, or nothing. Sending is a separate step, so the
 * rules can be tested without a window.
 *
 * Notifications can show on a lock screen, so they carry only the chat title
 * and what happened. Never message text, commands, paths or tool arguments.
 */

export interface Notice {
  title: string;
  body: string;
  /**
   * Set when the same event can arrive twice (an approval is re-sent to a
   * conversation that was reopened). Notices that end a turn have no key: two
   * turns in one chat are two different things to be told about.
   */
  key?: string;
}

/** What the user is looking at when the event arrives. */
export interface Attention {
  /** The app window has focus and is visible. */
  windowActive: boolean;
  /** The open conversation. */
  activeId: string | null;
}

export interface NoticeContext extends Attention {
  /** Title of a conversation, by id. */
  titleOf: (conversationId: string) => string | undefined;
}

const APP_NAME = "Jarvis";

/** True when the user would not notice the event on screen. */
export function isOutOfSight(conversationId: string, attention: Attention): boolean {
  return !attention.windowActive || attention.activeId !== conversationId;
}

function heading(conversationId: string, context: NoticeContext): string {
  const title = context.titleOf(conversationId)?.trim();
  return title ? `${APP_NAME} · ${title.slice(0, 60)}` : APP_NAME;
}

/**
 * What to tell the user about an event, or null for nothing.
 *
 * - A turn ends with status `idle`. `idle` carrying a message is a stop the
 *   user asked for ("Stopped."), which needs no alert. A manual compaction also
 *   ends with a plain `idle`, so it reports "Finished." like any other turn.
 * - `error` ends a turn with a failure.
 * - An approval or an input request leaves the turn waiting for the user.
 */
export function noticeFor(event: CoreEvent, context: NoticeContext): Notice | null {
  if (!("conversationId" in event) || !event.conversationId) return null;
  const conversationId = event.conversationId;
  if (!isOutOfSight(conversationId, context)) return null;
  const title = heading(conversationId, context);

  switch (event.type) {
    case "status":
      if (event.status === "idle" && !event.message) {
        return { title, body: "Finished." };
      }
      if (event.status === "error") {
        return { title, body: "Something went wrong." };
      }
      return null;
    case "approval":
      return {
        title,
        body: `Waiting for your approval: ${event.tool}`,
        key: `approval:${event.callId}`,
      };
    case "input_request":
      return { title, body: "Waiting for your answer.", key: `input:${event.callId}` };
    default:
      return null;
  }
}

/** Remembers what was already shown, and spaces notifications out. */
export class NoticeLimiter {
  private readonly seen = new Map<string, number>();
  /** When the last notification went out; null before the first. */
  private lastSentAt: number | null = null;

  private readonly minGapMs: number;
  private readonly rememberMs: number;

  constructor(
    /** Minimum gap between two turn-ending notifications, in ms. */
    minGapMs = 1500,
    /** How long a key stays remembered, in ms. */
    rememberMs = 10 * 60 * 1000,
  ) {
    this.minGapMs = minGapMs;
    this.rememberMs = rememberMs;
  }

  /** Whether to show this notice now. Records it when it does. */
  allow(notice: Notice, now: number = Date.now()): boolean {
    for (const [key, at] of this.seen) {
      if (now - at > this.rememberMs) this.seen.delete(key);
    }
    if (notice.key !== undefined && this.seen.has(notice.key)) return false;
    // A notice with a key is one the turn is blocked on (an approval, a
    // question). It is never held back by the gap. Turn endings are, so a loop
    // of short turns cannot flood the desktop.
    if (
      notice.key === undefined &&
      this.lastSentAt !== null &&
      now - this.lastSentAt < this.minGapMs
    ) {
      return false;
    }
    if (notice.key !== undefined) this.seen.set(notice.key, now);
    this.lastSentAt = now;
    return true;
  }
}
