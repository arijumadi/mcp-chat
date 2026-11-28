import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentSessionPicker } from "./AgentSessionPicker";
import { useApp } from "@/app/store";
import type { Conversation } from "@/lib/types";

afterEach(cleanup);

const conversation = {
  id: "c1",
  title: "Chat",
} as unknown as Conversation;

const agents = [
  {
    id: "a1",
    name: "OpenCode",
    slug: "opencode",
    command: "opencode",
    args: ["acp"],
    envKeys: [],
    permissionPolicy: "ask",
    enabled: true,
  },
];

/** Install a fake `call` that answers the three commands this component uses. */
function withCalls(page: unknown) {
  const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
  const invoke = vi.fn(async (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "list_acp_agents_for_chat") return agents;
    if (command === "list_agent_sessions") return page;
    if (command === "attach_agent_session") return null;
    if (command === "get_agent_config") return [];
    if (command === "get_agent_commands") return [];
    if (command === "get_messages") return [];
    return null;
  });
  // The store is the only path the component uses to reach the backend.
  useApp.setState({
    toast: vi.fn(),
    refreshConversations: vi.fn(async () => {}),
    // Matches the store: agent config returns options, commands return void.
    refreshAgentConfig: vi.fn(async () => []),
    refreshAgentCommands: vi.fn(async () => {}),
    openConversation: vi.fn(async () => {}),
  });
  window.__MCP_CHAT_MOCK__ = { invoke } as never;
  return { calls, invoke };
}

describe("AgentSessionPicker", () => {
  it("says so when the agent cannot list sessions", async () => {
    withCalls({ sessions: [], supported: false });
    render(<AgentSessionPicker conversation={conversation} open onClose={vi.fn()} />);

    await waitFor(() =>
      expect(
        screen.getByText(/does not implement session listing/i),
      ).toBeInTheDocument(),
    );
  });

  it("distinguishes an empty list from an unsupported agent", async () => {
    withCalls({ sessions: [], supported: true });
    render(<AgentSessionPicker conversation={conversation} open onClose={vi.fn()} />);

    await waitFor(() =>
      expect(screen.getByText(/no sessions to show/i)).toBeInTheDocument(),
    );
  });

  it("attaches with the session's own cwd, never the chat's workspace", async () => {
    const { calls } = withCalls({
      supported: true,
      sessions: [
        {
          sessionId: "ses_1",
          cwd: "/home/ari/some-project",
          title: "Old work",
          updatedAt: null,
        },
      ],
    });
    render(<AgentSessionPicker conversation={conversation} open onClose={vi.fn()} />);

    fireEvent.click(await screen.findByText("Old work"));

    await waitFor(() => {
      const attach = calls.find((call) => call.command === "attach_agent_session");
      expect(attach).toBeDefined();
    });
    const attach = calls.find((call) => call.command === "attach_agent_session");
    expect(attach?.args).toMatchObject({
      conversationId: "c1",
      sessionId: "ses_1",
      // The whole point: the session's directory, not the chat's.
      cwd: "/home/ari/some-project",
    });
  });

  it("filters by title and by directory", async () => {
    withCalls({
      supported: true,
      sessions: [
        { sessionId: "a", cwd: "/home/ari/alpha", title: "Alpha", updatedAt: null },
        { sessionId: "b", cwd: "/home/ari/beta", title: "Beta", updatedAt: null },
      ],
    });
    render(<AgentSessionPicker conversation={conversation} open onClose={vi.fn()} />);

    await screen.findByText("Alpha");
    fireEvent.change(screen.getByPlaceholderText(/Filter by title/i), {
      target: { value: "beta" },
    });

    expect(screen.queryByText("Alpha")).not.toBeInTheDocument();
    expect(screen.getByText("Beta")).toBeInTheDocument();
  });
});
