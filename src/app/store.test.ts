import { beforeEach, describe, expect, it } from "vitest";
import { applyCoreEvent, parsePayload, useApp, type AppStore } from "./store";
import type { CoreEvent } from "@/lib/types";

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
    messages: [],
    approvals: [],
    inputRequests: [],
    status: "idle",
    streamingMessageId: null,
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
