import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { ApprovalModal } from "./ApprovalModal";
import { useApp } from "@/app/store";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("ApprovalModal", () => {
  it("shows the proposed diff before the arguments", () => {
    const respondApproval = vi.fn().mockResolvedValue(undefined);
    useApp.setState({
      activeId: "c1",
      respondApproval,
      approvals: [
        {
          type: "approval",
          conversationId: "c1",
          callId: "call1",
          server: "mcp-chat",
          tool: "edit",
          args: { filePath: "src/app.ts", oldString: "old", newString: "new" },
          allowAlways: false,
          diff: "--- a/src/app.ts\n+++ b/src/app.ts\n@@\n-old\n+new\n",
        },
      ],
    });
    render(<ApprovalModal />);
    // Open by default so the user can review without an extra click.
    expect(screen.getByRole("button", { name: /Changes/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByText("Changes")).toBeInTheDocument();
    expect(screen.getByText(/--- a\/src\/app\.ts/)).toHaveClass("diff-file-header");
    expect(screen.getByText("-old")).toHaveClass("diff-line-removed");
    expect(screen.getByText("+new")).toHaveClass("diff-line-added");
    // The diff precedes the arguments.
    const diff = screen.getByRole("region", { name: "Changes" });
    const args = screen.getByText("Arguments");
    expect(diff.compareDocumentPosition(args) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeInTheDocument();
  });

  it("shows the exact pattern that Always allow will save", () => {
    useApp.setState({
      activeId: "c1",
      respondApproval: vi.fn().mockResolvedValue(undefined),
      approvals: [
        {
          type: "approval",
          conversationId: "c1",
          callId: "call-pattern",
          server: "mcp-chat",
          tool: "bash",
          args: { command: "cat /logs/log_1.log" },
          allowAlways: true,
          action: "builtin.bash",
          resource: "cat /logs/log_1.log",
          savePattern: "cat *",
          diff: null,
        },
      ],
    });
    render(<ApprovalModal />);
    const rule = screen.getByTestId("approval-rule");
    expect(rule).toHaveTextContent("Always allow will save");
    expect(rule).toHaveTextContent("builtin.bash");
    expect(screen.getByTestId("approval-pattern")).toHaveTextContent("cat *");
    expect(rule).toHaveTextContent("Pattern rule");
    expect(rule).toHaveTextContent("Saved rules apply across chats");
  });

  it("calls out a bash command that is allowed to write file contents", () => {
    useApp.setState({
      activeId: "c1",
      respondApproval: vi.fn().mockResolvedValue(undefined),
      approvals: [
        {
          type: "approval",
          conversationId: "c1",
          callId: "call2",
          server: "mcp-chat",
          tool: "bash",
          args: { command: "echo hi > out.txt", allowFileWrites: true },
          allowAlways: true,
          diff: null,
        },
      ],
    });
    render(<ApprovalModal />);
    expect(screen.getByRole("note")).toHaveTextContent("allowFileWrites is on.");
    // The flag is also in the raw arguments the user approves.
    expect(screen.getByText(/"allowFileWrites": true/)).toBeInTheDocument();
  });

  it("shows no warning for an ordinary bash command", () => {
    useApp.setState({
      activeId: "c1",
      respondApproval: vi.fn().mockResolvedValue(undefined),
      approvals: [
        {
          type: "approval",
          conversationId: "c1",
          callId: "call3",
          server: "mcp-chat",
          tool: "bash",
          args: { command: "npm test" },
          allowAlways: true,
          diff: null,
        },
      ],
    });
    render(<ApprovalModal />);
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("tells the user, and assistive technology, which keys answer the call", () => {
    useApp.setState({
      activeId: "c1",
      respondApproval: vi.fn().mockResolvedValue(undefined),
      approvals: [
        {
          type: "approval",
          conversationId: "c1",
          callId: "call9",
          server: "mcp-chat",
          tool: "bash",
          args: { command: "npm test" },
          allowAlways: true,
          diff: null,
        },
      ],
    });
    render(<ApprovalModal />);
    const once = screen.getByRole("button", { name: "Allow once" });
    expect(once).toHaveAttribute("title", expect.stringMatching(/Ctrl\+Enter|⌘↩/));
    expect(once).toHaveAttribute("aria-keyshortcuts", expect.stringMatching(/^(Control|Meta)\+Enter$/));
    expect(screen.getByRole("button", { name: "Always allow" })).toHaveAttribute(
      "aria-keyshortcuts",
      expect.stringMatching(/Shift\+Enter$/),
    );
    expect(screen.getByRole("button", { name: "Reject" })).toHaveAttribute(
      "aria-keyshortcuts",
      expect.stringMatching(/Backspace$/),
    );
  });
});
