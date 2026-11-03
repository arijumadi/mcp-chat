import { describe, expect, it } from "vitest";
import type { CoreEvent } from "@/lib/types";
import { NoticeLimiter, isOutOfSight, noticeFor, type NoticeContext } from "./notifications";

const titles: Record<string, string> = { c1: "Fix the build", c2: "Write release notes" };

function context(over: Partial<NoticeContext> = {}): NoticeContext {
  return { windowActive: false, activeId: "c1", titleOf: (id) => titles[id], ...over };
}

const idle = (conversationId = "c1", message: string | null = null): CoreEvent => ({
  type: "status",
  conversationId,
  status: "idle",
  message,
});

const approval = (conversationId = "c1", callId = "call1"): CoreEvent => ({
  type: "approval",
  conversationId,
  callId,
  server: "mcp-chat",
  tool: "bash",
  args: { command: "cat ~/.ssh/id_rsa" },
  allowAlways: true,
});

describe("isOutOfSight", () => {
  it("is out of sight when the window is not active", () => {
    expect(isOutOfSight("c1", { windowActive: false, activeId: "c1" })).toBe(true);
  });

  it("is out of sight when another conversation is open", () => {
    expect(isOutOfSight("c2", { windowActive: true, activeId: "c1" })).toBe(true);
    expect(isOutOfSight("c2", { windowActive: true, activeId: null })).toBe(true);
  });

  it("is in sight only for the open conversation in an active window", () => {
    expect(isOutOfSight("c1", { windowActive: true, activeId: "c1" })).toBe(false);
  });
});

describe("noticeFor", () => {
  it("tells the user a turn finished while the window is in the background", () => {
    expect(noticeFor(idle(), context())).toEqual({
      title: "Jarvis · Fix the build",
      body: "Finished.",
    });
  });

  it("tells the user about a background conversation even when the window is active", () => {
    const notice = noticeFor(idle("c2"), context({ windowActive: true }));
    expect(notice?.title).toBe("Jarvis · Write release notes");
  });

  it("stays quiet about the conversation the user is looking at", () => {
    expect(noticeFor(idle("c1"), context({ windowActive: true }))).toBeNull();
    expect(noticeFor(approval("c1"), context({ windowActive: true }))).toBeNull();
  });

  it("stays quiet when the user stopped the turn", () => {
    expect(noticeFor(idle("c1", "Stopped."), context())).toBeNull();
    expect(noticeFor(idle("c1", "anything else"), context())).toBeNull();
  });

  it("reports a failed turn", () => {
    const failed: CoreEvent = { type: "status", conversationId: "c1", status: "error", message: "boom" };
    expect(noticeFor(failed, context())).toEqual({
      title: "Jarvis · Fix the build",
      body: "Something went wrong.",
    });
  });

  it("ignores statuses that are not the end of a turn", () => {
    for (const status of ["thinking", "streaming", "compacting", "waiting_approval", "running_tool"] as const) {
      const event: CoreEvent = { type: "status", conversationId: "c1", status, message: null };
      expect(noticeFor(event, context()), status).toBeNull();
    }
  });

  it("asks for attention when a tool needs approval", () => {
    const notice = noticeFor(approval(), context());
    expect(notice?.body).toBe("Waiting for your approval: bash");
    expect(notice?.key).toBe("approval:call1");
  });

  it("asks for attention when the model asks a question", () => {
    const event: CoreEvent = {
      type: "input_request",
      conversationId: "c1",
      callId: "q1",
      request: { questions: [{ header: "Colour", question: "Which colour?", options: [{ label: "Blue" }] }] },
    };
    const notice = noticeFor(event, context());
    expect(notice?.body).toBe("Waiting for your answer.");
    expect(notice?.key).toBe("input:q1");
  });

  it("never puts message text, commands or paths in a notification", () => {
    const text = JSON.stringify([
      noticeFor(approval(), context()),
      noticeFor(idle(), context()),
    ]);
    expect(text).not.toContain("id_rsa");
    expect(text).not.toContain("cat ");
    expect(text).not.toContain(".ssh");
  });

  it("falls back to the app name for an unknown or untitled chat, and shortens long titles", () => {
    expect(noticeFor(idle("c1"), context({ titleOf: () => undefined }))?.title).toBe("Jarvis");
    expect(noticeFor(idle("c1"), context({ titleOf: () => "   " }))?.title).toBe("Jarvis");
    const long = noticeFor(idle("c1"), context({ titleOf: () => "x".repeat(200) }));
    expect(long?.title.length).toBeLessThan(80);
  });

  it("ignores events that are not about a conversation", () => {
    expect(noticeFor({ type: "resync", reason: "lagged" }, context())).toBeNull();
    expect(noticeFor({ type: "error", conversationId: null, message: "x" }, context())).toBeNull();
  });
});

describe("NoticeLimiter", () => {
  const done = { title: "Jarvis", body: "Finished." };
  const ask = (callId: string) => ({ title: "Jarvis", body: "Waiting", key: `approval:${callId}` });

  it("does not repeat the same approval", () => {
    const limiter = new NoticeLimiter();
    expect(limiter.allow(ask("a"), 0)).toBe(true);
    expect(limiter.allow(ask("a"), 100)).toBe(false);
    expect(limiter.allow(ask("b"), 200)).toBe(true);
  });

  it("forgets an approval after a while", () => {
    const limiter = new NoticeLimiter(1500, 1000);
    expect(limiter.allow(ask("a"), 0)).toBe(true);
    expect(limiter.allow(ask("a"), 5000)).toBe(true);
  });

  it("spaces out turn endings so a loop of short turns cannot flood the desktop", () => {
    const limiter = new NoticeLimiter(1500);
    expect(limiter.allow(done, 0)).toBe(true);
    expect(limiter.allow(done, 800)).toBe(false);
    expect(limiter.allow(done, 1600)).toBe(true);
  });

  it("tells the user about two finished turns in the same chat", () => {
    const limiter = new NoticeLimiter(1500);
    expect(limiter.allow(done, 0)).toBe(true);
    expect(limiter.allow(done, 60_000)).toBe(true);
  });

  it("never holds back an approval, which blocks the turn", () => {
    const limiter = new NoticeLimiter(1500);
    expect(limiter.allow(done, 0)).toBe(true);
    expect(limiter.allow(ask("a"), 10)).toBe(true);
    expect(limiter.allow(ask("b"), 20)).toBe(true);
  });
});
