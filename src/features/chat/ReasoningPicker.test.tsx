import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ReasoningPicker } from "./ReasoningPicker";
import { useApp } from "@/app/store";
import type { Conversation } from "@/lib/types";

const conversation: Conversation = {
  id: "c1",
  title: "Test",
  createdAt: 0,
  updatedAt: 0,
  providerId: "p1",
  model: "mock",
  reasoningEffort: "default",
  workspaceRoot: "/home/you/project",
  systemPrompt: null,
  archived: false,
  pinned: false,
  maxSteps: 25,
  toolMode: "lazy",
  serversConfigured: false,
};

const calls: Array<{ conversationId: string; effort: string }> = [];

function Harness() {
  const current = useApp((state) =>
    state.conversations.find((item) => item.id === conversation.id),
  );
  return current ? <ReasoningPicker conversation={current} /> : null;
}

beforeEach(() => {
  calls.length = 0;
  window.__MCP_CHAT_MOCK__ = {
    invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
      if (command === "set_conversation_reasoning_effort") {
        calls.push({
          conversationId: String(args?.conversationId),
          effort: String(args?.effort),
        });
      }
      return null as T;
    },
    subscribe: () => () => undefined,
  };
  useApp.setState({ conversations: [conversation], toasts: [] });
});

afterEach(() => {
  cleanup();
  delete window.__MCP_CHAT_MOCK__;
});

describe("ReasoningPicker", () => {
  it("shows Default and saves another effort", async () => {
    render(<Harness />);
    const trigger = screen.getByRole("button", { name: "Reasoning effort: Default" });
    fireEvent.click(trigger);

    expect(screen.getByRole("listbox", { name: "Reasoning effort" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /^Max/ }));

    await waitFor(() =>
      expect(calls).toEqual([{ conversationId: "c1", effort: "max" }]),
    );
    expect(
      screen.getByRole("button", { name: "Reasoning effort: Max" }),
    ).toBeInTheDocument();
  });
});
