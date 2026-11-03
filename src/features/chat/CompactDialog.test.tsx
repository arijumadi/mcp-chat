import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { CompactDialog } from "./CompactDialog";
import { useApp } from "@/app/store";

const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];

beforeEach(() => {
  calls.length = 0;
  window.__MCP_CHAT_MOCK__ = {
    invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
      calls.push({ command, args });
      if (command === "context_status") {
        return { tokens: 12450, window: 128000, threshold: 100000, keepTokens: 15000, toolCount: 3, toolMode: "lazy" } as T;
      }
      return null as T;
    },
    subscribe: () => () => undefined,
  };
  useApp.setState({
    activeId: "c1",
    view: "chat",
    status: "idle",
    draft: "",
    compactDialog: null,
    toasts: [],
  });
});

afterEach(() => {
  cleanup();
  delete window.__MCP_CHAT_MOCK__;
});

describe("CompactDialog", () => {
  it("renders nothing until a compaction is requested", () => {
    render(<CompactDialog />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("explains the consequences and shows the context size", async () => {
    useApp.getState().openCompact();
    render(<CompactDialog />);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText(/This cannot be undone/)).toBeInTheDocument();
    expect(await screen.findByText(/Context now: about 12k of 128k tokens\./)).toBeInTheDocument();
  });

  it("works when the context estimate is unavailable", async () => {
    window.__MCP_CHAT_MOCK__ = {
      invoke: async () => {
        throw new Error("no estimate");
      },
      subscribe: () => () => undefined,
    };
    useApp.getState().openCompact();
    render(<CompactDialog />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Compact" })).toBeEnabled());
    expect(screen.queryByText(/Context now/)).not.toBeInTheDocument();
  });

  it("sends the focus and closes when confirmed", async () => {
    useApp.getState().openCompact("keep the schema");
    render(<CompactDialog />);
    const focus = screen.getByLabelText("Focus (optional)") as HTMLTextAreaElement;
    expect(focus.value).toBe("keep the schema");
    fireEvent.change(focus, { target: { value: "  keep the migration notes  " } });
    fireEvent.click(screen.getByRole("button", { name: "Compact" }));

    await waitFor(() => expect(calls.some((c) => c.command === "compact_conversation")).toBe(true));
    const compact = calls.find((c) => c.command === "compact_conversation")!;
    expect(compact.args).toEqual({ conversationId: "c1", focus: "keep the migration notes" });
    expect(useApp.getState().compactDialog).toBeNull();
  });

  it("sends no focus at all when the field is blank", async () => {
    useApp.getState().openCompact();
    render(<CompactDialog />);
    fireEvent.click(screen.getByRole("button", { name: "Compact" }));
    await waitFor(() => expect(calls.some((c) => c.command === "compact_conversation")).toBe(true));
    expect(calls.find((c) => c.command === "compact_conversation")!.args).toEqual({
      conversationId: "c1",
      focus: null,
    });
  });

  it("clears a typed /compact draft on confirm, but keeps other drafts", async () => {
    useApp.setState({ draft: "/compact keep it" });
    useApp.getState().openCompact("keep it");
    const { unmount } = render(<CompactDialog />);
    fireEvent.click(screen.getByRole("button", { name: "Compact" }));
    await waitFor(() => expect(useApp.getState().draft).toBe(""));
    unmount();

    useApp.setState({ draft: "something I was writing", compactDialog: null });
    useApp.getState().openCompact();
    render(<CompactDialog />);
    fireEvent.click(screen.getByRole("button", { name: "Compact" }));
    await waitFor(() => expect(useApp.getState().compactDialog).toBeNull());
    expect(useApp.getState().draft).toBe("something I was writing");
  });

  it("Cancel closes without compacting and keeps the draft", () => {
    useApp.setState({ draft: "/compact keep it" });
    useApp.getState().openCompact("keep it");
    render(<CompactDialog />);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(useApp.getState().compactDialog).toBeNull();
    expect(useApp.getState().draft).toBe("/compact keep it");
    expect(calls.some((c) => c.command === "compact_conversation")).toBe(false);
  });

  it("confirms with Ctrl+Enter from the focus field", async () => {
    useApp.getState().openCompact();
    render(<CompactDialog />);
    fireEvent.keyDown(screen.getByLabelText("Focus (optional)"), { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(calls.some((c) => c.command === "compact_conversation")).toBe(true));
  });
});

describe("compaction requests", () => {
  it("refuses to open while a turn is running, and says why", () => {
    useApp.setState({ status: "streaming" });
    useApp.getState().openCompact();
    expect(useApp.getState().compactDialog).toBeNull();
    expect(useApp.getState().toasts.at(-1)?.text).toContain("Stop the current turn");
  });

  it("refuses without an open conversation or outside the chat view", () => {
    useApp.setState({ activeId: null });
    useApp.getState().openCompact();
    expect(useApp.getState().compactDialog).toBeNull();
    useApp.setState({ activeId: "c1", view: "settings" });
    useApp.getState().openCompact();
    expect(useApp.getState().compactDialog).toBeNull();
  });

  it("reports a backend refusal as an error toast", async () => {
    window.__MCP_CHAT_MOCK__ = {
      invoke: async (command: string) => {
        if (command === "compact_conversation") throw new Error("Not enough history to compact.");
        return null as never;
      },
      subscribe: () => () => undefined,
    };
    await useApp.getState().compactConversation("");
    const toast = useApp.getState().toasts.at(-1);
    expect(toast?.kind).toBe("error");
    expect(toast?.text).toContain("Not enough history");
  });

  it("confirms success with a toast", async () => {
    await useApp.getState().compactConversation("");
    expect(useApp.getState().toasts.at(-1)?.text).toBe("Conversation compacted.");
  });
});
