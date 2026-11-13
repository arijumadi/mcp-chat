import { beforeEach, describe, expect, it, vi } from "vitest";
import { applyCoreEvent, parsePayload, useApp, type AppStore } from "./store";
import type { Conversation, CoreEvent } from "@/lib/types";

const set = (partial: Partial<AppStore> | ((state: AppStore) => Partial<AppStore>)) =>
  useApp.setState(partial as Partial<AppStore>);
const get = () => useApp.getState();

function apply(event: CoreEvent) {
  applyCoreEvent(event, set, get);
}

beforeEach(() => {
  window.__MCP_CHAT_MOCK__ = {
    invoke: async <T,>(_command: string) => [] as unknown as T,
    subscribe: () => () => undefined,
  };
  useApp.setState({
    activeId: "c1",
    conversations: [],
    subagents: {},
    messages: [],
    approvals: [],
    inputRequests: [],
    status: "idle",
    streamingMessageId: null,
    updateInfo: null,
    updateChecking: false,
    updateError: null,
    toasts: [],
  });
});

describe("transcript event reducer", () => {
  it("creates an assistant message and appends text deltas", () => {
    apply({
      type: "message_start",
      conversationId: "c1",
      messageId: "m1",
    });
    apply({
      type: "delta",
      conversationId: "c1",
      messageId: "m1",
      partId: "p1",
      deltaType: "text",
      text: "Hello",
    });
    apply({
      type: "delta",
      conversationId: "c1",
      messageId: "m1",
      partId: "p1",
      deltaType: "text",
      text: " world",
    });
    const message = get().messages[0]!;
    expect(message.id).toBe("m1");
    expect(message.parts).toHaveLength(1);
    expect(parsePayload(message.parts[0]!).text).toBe("Hello world");
  });

  it("upserts a tool part and updates its state", () => {
    apply({ type: "message_start", conversationId: "c1", messageId: "m1" });
    apply({
      type: "part",
      conversationId: "c1",
      messageId: "m1",
      partId: "tool1",
      kind: "tool_call",
      payload: {
        callId: "call1",
        name: "echo",
        server: "Mock",
        args: { text: "hi" },
        state: "running",
      },
    });
    apply({
      type: "tool",
      conversationId: "c1",
      callId: "call1",
      state: "completed",
      message: "echo: hi",
    });
    const payload = parsePayload(get().messages[0]!.parts[0]!);
    expect(payload.state).toBe("completed");
    expect(payload.resultPreview).toBe("echo: hi");
  });

  it("shows a shell exit code as soon as the call finishes", () => {
    apply({ type: "message_start", conversationId: "c1", messageId: "m1" });
    apply({
      type: "part",
      conversationId: "c1",
      messageId: "m1",
      partId: "bash1",
      kind: "tool_call",
      payload: { callId: "b1", name: "bash", server: "mcp-chat", args: {}, state: "running" },
    });
    apply({ type: "tool", conversationId: "c1", callId: "b1", state: "completed", message: "done", exitCode: 2 });
    const payload = parsePayload(get().messages[0]!.parts[0]!);
    expect(payload.exitCode).toBe(2);
    // An event without an exit code does not erase it.
    apply({ type: "tool", conversationId: "c1", callId: "b1", state: "completed" });
    expect(parsePayload(get().messages[0]!.parts[0]!).exitCode).toBe(2);
  });

  it("shows a read image as soon as the call finishes", () => {
    apply({ type: "message_start", conversationId: "c1", messageId: "m1" });
    apply({
      type: "part",
      conversationId: "c1",
      messageId: "m1",
      partId: "read1",
      kind: "tool_call",
      payload: { callId: "r1", name: "read", server: "mcp-chat", args: {}, state: "running" },
    });
    const image = {
      name: "pw_test.png",
      mime: "image/png",
      path: "/tmp/read-images/pw_test.png",
      size: 82444,
    };
    apply({
      type: "tool",
      conversationId: "c1",
      callId: "r1",
      state: "completed",
      message: "Image loaded: .mcp-chat/artifacts/pw_test.png (image/png, 82444 bytes)",
      image,
    });
    const payload = parsePayload(get().messages[0]!.parts[0]!);
    expect(payload.image).toEqual(image);

    // A later event without an image does not erase the preview.
    apply({ type: "tool", conversationId: "c1", callId: "r1", state: "completed" });
    expect(parsePayload(get().messages[0]!.parts[0]!).image).toEqual(image);
  });

  it("stops saying a tool needs approval once it has been answered", () => {
    const start = (callId: string) => {
      apply({
        type: "part",
        conversationId: "c1",
        messageId: "m1",
        partId: `part-${callId}`,
        kind: "tool_call",
        payload: { callId, name: "bash", server: "mcp-chat", args: {}, state: "running", approval: "pending" },
      });
    };
    apply({ type: "message_start", conversationId: "c1", messageId: "m1" });
    for (const [callId, state, expected] of [
      ["allowed-run", "running", "allowed"],
      ["allowed-done", "completed", "allowed"],
      ["rejected", "error", "rejected"],
      ["stopped", "aborted", "rejected"],
    ] as const) {
      start(callId);
      apply({ type: "tool", conversationId: "c1", callId, state, message: null });
      const part = get().messages[0]!.parts.find((item) => item.id === `part-${callId}`)!;
      expect(parsePayload(part).approval, callId).toBe(expected);
    }
    // A call still waiting for its answer stays pending.
    start("waiting");
    apply({ type: "tool", conversationId: "c1", callId: "waiting", state: "queued", message: null });
    const waiting = get().messages[0]!.parts.find((item) => item.id === "part-waiting")!;
    expect(parsePayload(waiting).approval).toBe("pending");
  });

  it("stores a finished tool's diff on the tool_call part", () => {
    apply({ type: "message_start", conversationId: "c1", messageId: "m1" });
    apply({
      type: "part",
      conversationId: "c1",
      messageId: "m1",
      partId: "tool1",
      kind: "tool_call",
      payload: {
        callId: "call1",
        name: "edit",
        server: "mcp-chat",
        args: { filePath: "a.txt" },
        state: "running",
      },
    });
    apply({
      type: "tool",
      conversationId: "c1",
      callId: "call1",
      state: "completed",
      message: "Edited a.txt",
      diff: "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n",
    });
    const payload = parsePayload(get().messages[0]!.parts[0]!);
    expect(payload.state).toBe("completed");
    expect(payload.diff).toBe("--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n");

    // A later event without a diff must not erase it.
    apply({ type: "tool", conversationId: "c1", callId: "call1", state: "completed" });
    expect(parsePayload(get().messages[0]!.parts[0]!).diff).toContain("+new");
  });

  it("shows a compaction summary as a system row without reloading", () => {
    apply({
      type: "message_start",
      conversationId: "c1",
      messageId: "summary1",
      role: "system",
    });
    apply({
      type: "part",
      conversationId: "c1",
      messageId: "summary1",
      partId: "part1",
      kind: "summary",
      payload: { text: "Earlier work", coversUntil: "m4" },
    });
    apply({
      type: "message_end",
      conversationId: "c1",
      messageId: "summary1",
      status: "completed",
      error: null,
    });
    const message = get().messages[0]!;
    expect(message.role).toBe("system");
    expect(message.status).toBe("completed");
    expect(message.parts[0]!.kind).toBe("summary");
    expect(parsePayload(message.parts[0]!).text).toBe("Earlier work");
    expect(get().streamingMessageId).toBeNull();
  });

  it("queues approvals and clears them when the turn ends", () => {
    apply({
      type: "approval",
      conversationId: "c1",
      callId: "call1",
      server: "Mock",
      tool: "echo",
      args: {},
      allowAlways: true,
    });
    expect(get().approvals).toHaveLength(1);
    expect(get().status).toBe("waiting_approval");
    apply({ type: "status", conversationId: "c1", status: "idle" });
    expect(get().approvals).toHaveLength(0);
    expect(get().status).toBe("idle");
  });

  it("keeps approvals for an inactive chat until that turn ends", () => {
    apply({
      type: "approval",
      conversationId: "c2",
      callId: "call2",
      server: "Mock",
      tool: "echo",
      args: {},
      allowAlways: true,
    });
    expect(get().approvals).toHaveLength(1);
    expect(get().status).toBe("idle");
    apply({ type: "status", conversationId: "c2", status: "idle" });
    expect(get().approvals).toHaveLength(0);
  });

  it("restores a pending approval when switching back to a generating chat", async () => {
    const approval = {
      type: "approval" as const,
      conversationId: "c2",
      callId: "call2",
      server: "Mock",
      tool: "echo",
      args: {},
      allowAlways: true,
    };
    useApp.setState({ approvals: [approval] });
    window.__MCP_CHAT_MOCK__ = {
      invoke: async <T,>(command: string) => {
        if (command === "get_messages_page") {
          return { messages: [], hasMore: false } as T;
        }
        if (command === "is_generating") return true as T;
        if (command === "get_auto_approve") return false as T;
        return [] as unknown as T;
      },
      subscribe: () => () => undefined,
    };
    await get().openConversation("c2");
    expect(get().activeId).toBe("c2");
    expect(get().status).toBe("waiting_approval");
    expect(get().approvals).toHaveLength(1);
  });

  it("fetches a hidden subagent conversation before opening it", async () => {
    const child: Conversation = {
      id: "child",
      title: "Inspect workspace",
      createdAt: 0,
      updatedAt: 0,
      providerId: null,
      model: null,
      reasoningEffort: "default",
      parentId: "parent",
      workspaceRoot: "/tmp",
      systemPrompt: null,
      archived: false,
      pinned: false,
      maxSteps: 30,
      toolMode: "lazy",
      serversConfigured: false,
    };
    window.__MCP_CHAT_MOCK__ = {
      invoke: async <T,>(command: string) => {
        if (command === "get_conversation") return child as T;
        if (command === "get_messages_page") {
          return { messages: [], hasMore: false } as T;
        }
        if (command === "is_generating") return false as T;
        if (command === "get_auto_approve") return false as T;
        return [] as unknown as T;
      },
      subscribe: () => () => undefined,
    };
    await get().openConversation("child");
    expect(get().activeId).toBe("child");
    expect(get().subagents.child?.parentId).toBe("parent");
  });

  it("drops stale approvals when the chat is no longer generating", async () => {
    useApp.setState({
      approvals: [
        {
          type: "approval",
          conversationId: "c2",
          callId: "call2",
          server: "Mock",
          tool: "echo",
          args: {},
          allowAlways: true,
        },
      ],
    });
    window.__MCP_CHAT_MOCK__ = {
      invoke: async <T,>(command: string) => {
        if (command === "get_messages_page") {
          return { messages: [], hasMore: false } as T;
        }
        if (command === "is_generating") return false as T;
        if (command === "get_auto_approve") return false as T;
        return [] as unknown as T;
      },
      subscribe: () => () => undefined,
    };
    await get().openConversation("c2");
    expect(get().status).toBe("idle");
    expect(get().approvals).toHaveLength(0);
  });

  it("ignores events from another conversation", () => {
    apply({
      type: "delta",
      conversationId: "other",
      messageId: "m9",
      partId: "p9",
      deltaType: "text",
      text: "nope",
    });
    expect(get().messages).toHaveLength(0);
  });

  it("stores live token usage for the active conversation", () => {
    apply({
      type: "usage",
      conversationId: "c1",
      promptTokens: 1200,
      completionTokens: 34,
      contextWindow: 128000,
    });
    expect(get().usage?.promptTokens).toBe(1200);
    expect(get().usage?.completionTokens).toBe(34);
  });

  it("keeps streamed text when the message ends", () => {
    useApp.setState({
      activeId: "c1",
      messages: [
        {
          id: "m1",
          conversationId: "c1",
          role: "assistant",
          createdAt: Date.now(),
          status: "streaming",
          error: null,
          parts: [
            {
              id: "p1",
              messageId: "m1",
              idx: 0,
              kind: "text",
              payloadJson: JSON.stringify({ text: "the answer" }),
            },
          ],
        },
      ],
    });
    apply({
      type: "message_end",
      conversationId: "c1",
      messageId: "m1",
      status: "completed",
      error: null,
    });
    expect(get().messages[0]!.parts[0]!.payloadJson).toContain("the answer");
    expect(get().messages[0]!.status).toBe("completed");
  });

  it("checks for updates and surfaces an available release", async () => {
    const invoke = vi.fn(async (command: string) => {
      if (command === "check_for_update") {
        return {
          currentVersion: "0.3.23",
          latestVersion: "0.3.24",
          available: true,
          name: "Jarvis 0.3.24",
          notes: "Update notes",
          url: "https://github.com/arijumadi/mcp-chat/releases/tag/v0.3.24",
          publishedAt: null,
        };
      }
      return [];
    });
    window.__MCP_CHAT_MOCK__ = {
      invoke: invoke as never,
      subscribe: () => () => undefined,
    };

    await get().checkForUpdate(true);

    expect(get().updateInfo?.latestVersion).toBe("0.3.24");
    expect(get().updateChecking).toBe(false);
    expect(get().toasts[0]?.text).toContain("Jarvis 0.3.24 is available");
    expect(get().toasts[0]?.actions?.[0]?.label).toBe("View release");
  });

  it("keeps the open conversation when a list refresh misses it", async () => {
    useApp.setState({
      activeId: "c1",
      messages: [
        {
          id: "m1",
          conversationId: "c1",
          role: "assistant",
          createdAt: Date.now(),
          status: "completed",
          error: null,
          parts: [],
        },
      ],
    });
    await get().refreshConversations();
    expect(get().activeId).toBe("c1");
    expect(get().messages).toHaveLength(1);
  });
});
