import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CoreEvent } from "@/lib/types";
import {
  maybeNotify,
  notificationsEnabled,
  primeNotifications,
  resetPrimedForTests,
  sendTestNotification,
  setBackendForTests,
  type NotifyBackend,
  type Permission,
} from "./notify";

function fakeBackend(initial: Permission, answer: Permission = "granted") {
  const sent: Array<{ title: string; body: string }> = [];
  const state = { permission: initial, requested: 0 };
  const backend: NotifyBackend = {
    permission: async () => state.permission,
    request: async () => {
      state.requested += 1;
      state.permission = answer;
      return answer;
    },
    send: (notice) => {
      sent.push(notice);
    },
  };
  return { backend, sent, state };
}

const done: CoreEvent = { type: "status", conversationId: "c1", status: "idle", message: null };
const context = { enabled: true, activeId: "c1", titleOf: () => "Fix the build" };

let focused = false;
let visibility: DocumentVisibilityState = "visible";

beforeEach(() => {
  focused = false;
  visibility = "visible";
  vi.spyOn(document, "hasFocus").mockImplementation(() => focused);
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibility);
  resetPrimedForTests();
});

afterEach(() => {
  vi.restoreAllMocks();
  setBackendForTests(null);
});

describe("maybeNotify", () => {
  it("sends a notification when the window is in the background", async () => {
    const { backend, sent } = fakeBackend("granted");
    setBackendForTests(backend);
    await maybeNotify(done, context);
    expect(sent).toEqual([{ title: "Jarvis · Fix the build", body: "Finished." }]);
  });

  it("sends nothing while the user is looking at that chat", async () => {
    focused = true;
    const { backend, sent } = fakeBackend("granted");
    setBackendForTests(backend);
    await maybeNotify(done, context);
    expect(sent).toHaveLength(0);
  });

  it("treats a hidden window as out of sight even if it still has focus", async () => {
    focused = true;
    visibility = "hidden";
    const { backend, sent } = fakeBackend("granted");
    setBackendForTests(backend);
    await maybeNotify(done, context);
    expect(sent).toHaveLength(1);
  });

  it("tells the user about another chat even when the window is active", async () => {
    focused = true;
    const { backend, sent } = fakeBackend("granted");
    setBackendForTests(backend);
    await maybeNotify({ ...done, conversationId: "c2" }, { ...context, titleOf: () => "Other" });
    expect(sent).toEqual([{ title: "Jarvis · Other", body: "Finished." }]);
  });

  it("does nothing when notifications are turned off", async () => {
    const { backend, sent } = fakeBackend("granted");
    setBackendForTests(backend);
    await maybeNotify(done, { ...context, enabled: false });
    expect(sent).toHaveLength(0);
  });

  it("never asks for permission from a background event", async () => {
    const { backend, sent, state } = fakeBackend("default");
    setBackendForTests(backend);
    await maybeNotify(done, context);
    expect(state.requested).toBe(0);
    expect(sent).toHaveLength(0);
  });

  it("sends nothing when the user denied permission", async () => {
    const { backend, sent } = fakeBackend("denied");
    setBackendForTests(backend);
    await maybeNotify(done, context);
    expect(sent).toHaveLength(0);
  });

  it("repeats an approval notice only once", async () => {
    const { backend, sent } = fakeBackend("granted");
    setBackendForTests(backend);
    const approval: CoreEvent = {
      type: "approval", conversationId: "c1", callId: "call1", server: "mcp-chat",
      tool: "bash", args: {}, allowAlways: true,
    };
    await maybeNotify(approval, context);
    await maybeNotify(approval, context);
    expect(sent).toHaveLength(1);
  });

  it("swallows a failing backend instead of breaking the chat", async () => {
    setBackendForTests({
      permission: async () => "granted",
      request: async () => "granted",
      send: () => {
        throw new Error("the OS refused");
      },
    });
    await expect(maybeNotify(done, context)).resolves.toBeUndefined();
  });
});

describe("primeNotifications", () => {
  it("asks once, while the app is in front", async () => {
    const { backend, state } = fakeBackend("default");
    setBackendForTests(backend);
    await primeNotifications(true);
    await primeNotifications(true);
    expect(state.requested).toBe(1);
  });

  it("does not ask if permission is already decided or notifications are off", async () => {
    const granted = fakeBackend("granted");
    setBackendForTests(granted.backend);
    await primeNotifications(true);
    expect(granted.state.requested).toBe(0);

    resetPrimedForTests();
    const off = fakeBackend("default");
    setBackendForTests(off.backend);
    await primeNotifications(false);
    expect(off.state.requested).toBe(0);
  });
});

describe("sendTestNotification", () => {
  it("asks for permission, because the user pressed the button", async () => {
    const { backend, sent, state } = fakeBackend("default", "granted");
    setBackendForTests(backend);
    expect(await sendTestNotification()).toBe("sent");
    expect(state.requested).toBe(1);
    expect(sent).toEqual([{ title: "Jarvis", body: "Notifications are on." }]);
  });

  it("says so when the user refuses", async () => {
    const { backend, sent } = fakeBackend("default", "denied");
    setBackendForTests(backend);
    expect(await sendTestNotification()).toBe("blocked");
    expect(sent).toHaveLength(0);
  });

  it("reports an unavailable backend", async () => {
    setBackendForTests({
      permission: async () => {
        throw new Error("no such thing");
      },
      request: async () => "denied",
      send: () => undefined,
    });
    expect(await sendTestNotification()).toBe("unavailable");
  });
});

describe("notificationsEnabled", () => {
  it("is on unless explicitly turned off", () => {
    expect(notificationsEnabled({})).toBe(true);
    expect(notificationsEnabled({ "notifications.enabled": "true" })).toBe(true);
    expect(notificationsEnabled({ "notifications.enabled": "false" })).toBe(false);
  });
});
