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
    expect(screen.getByText("Changes")).toBeInTheDocument();
    expect(screen.getByText(/--- a\/src\/app\.ts/)).toBeInTheDocument();
    expect(screen.getByText(/-old/)).toBeInTheDocument();
    expect(screen.getByText(/\+new/)).toBeInTheDocument();
  });
});
