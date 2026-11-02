import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { useApp } from "./store";
import { resetPrimedForTests, setBackendForTests, type NotifyBackend } from "@/lib/notify";
import type { CoreEvent } from "@/lib/types";

/**
 * These go through the same path as the running app: init() subscribes to the
 * bridge, and events arrive through it.
 */
let emit: (event: CoreEvent) => void = () => undefined;
const sent: Array<{ title: string; body: string }> = [];
let focused = false;

const backend: NotifyBackend = {
  permission: async () => "granted",
  request: async () => "granted",
  send: (notice) => {
    sent.push(notice);
  },
};

const conversation = (id: string, title: string) =>
  ({ id, title, createdAt: 0, updatedAt: 0, providerId: null, model: null, workspaceRoot: null,
     systemPrompt: null, archived: false, pinned: false, maxSteps: 0, toolMode: "lazy",
     serversConfigured: false }) as never;

const bridge = {
  invoke: async <T,>(command: string) => {
    if (command === "list_conversations") {
      return [conversation("c1", "Fix the build"), conversation("c2", "Release notes")] as T;
    }
    if (command === "list_settings") return {} as T;
    if (command === "app_info") {
      return { workspaceDir: "/w", dataDir: "/d", version: "x", platform: "linux" } as T;
    }
    return [] as T;
  },
  subscribe: (handler: (event: CoreEvent) => void) => {
    emit = handler;
    return () => undefined;
  },
};

// init() keeps module-level state (its subscription), so it runs once and the
// tests reset only the store.
beforeAll(async () => {
  // jsdom has no matchMedia, which applyTheme("system") needs during init().
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  })) as unknown as typeof window.matchMedia;
  window.__MCP_CHAT_MOCK__ = bridge;
  await useApp.getState().init();
});

beforeEach(() => {
  sent.length = 0;
  focused = false;
  vi.spyOn(document, "hasFocus").mockImplementation(() => focused);
  setBackendForTests(backend);
  resetPrimedForTests();
  window.__MCP_CHAT_MOCK__ = bridge;
  useApp.setState({ activeId: "c1", messages: [], approvals: [], inputRequests: [], settings: {} });
});

afterEach(() => {
  vi.restoreAllMocks();
  setBackendForTests(null);
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("notifications from the event stream", () => {
  it("announces a finished turn while the window is in the background", async () => {
    emit({ type: "status", conversationId: "c1", status: "idle", message: null });
    await settle();
    expect(sent).toEqual([{ title: "Jarvis · Fix the build", body: "Finished." }]);
  });

  it("announces a background conversation even while the app is in front", async () => {
    focused = true;
    emit({ type: "status", conversationId: "c2", status: "idle", message: null });
    await settle();
    expect(sent).toEqual([{ title: "Jarvis · Release notes", body: "Finished." }]);
  });

  it("stays quiet for the open chat while the app is in front", async () => {
    focused = true;
    emit({ type: "status", conversationId: "c1", status: "idle", message: null });
    await settle();
    expect(sent).toHaveLength(0);
  });

  it("announces an approval in a conversation that is not open", async () => {
    focused = true;
    emit({ type: "approval", conversationId: "c2", callId: "k1", server: "mcp-chat", tool: "bash", args: {}, allowAlways: true });
    await settle();
    expect(sent).toEqual([{ title: "Jarvis · Release notes", body: "Waiting for your approval: bash" }]);
  });

  it("announces an approval for the open chat when the window is in the background", async () => {
    emit({ type: "approval", conversationId: "c1", callId: "k2", server: "mcp-chat", tool: "edit", args: {}, allowAlways: false });
    await settle();
    expect(sent.map((n) => n.body)).toEqual(["Waiting for your approval: edit"]);
  });

  it("does not announce streaming text, and does not slow it down", async () => {
    emit({ type: "delta", conversationId: "c1", messageId: "m", partId: "p", deltaType: "text", text: "hi" });
    await settle();
    expect(sent).toHaveLength(0);
  });

  it("respects the setting that turns notifications off", async () => {
    useApp.setState({ settings: { "notifications.enabled": "false" } });
    emit({ type: "status", conversationId: "c1", status: "idle", message: null });
    await settle();
    expect(sent).toHaveLength(0);
  });

  it("does not announce a stop the user asked for", async () => {
    emit({ type: "status", conversationId: "c1", status: "idle", message: "Stopped." });
    await settle();
    expect(sent).toHaveLength(0);
  });
});
