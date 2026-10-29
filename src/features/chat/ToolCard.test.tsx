import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { ToolCard } from "./ToolCard";
import type { Part } from "@/lib/types";

const part: Part = {
  id: "p1",
  messageId: "m1",
  idx: 0,
  kind: "tool_call",
  payloadJson: JSON.stringify({
    callId: "call1",
    name: "echo",
    server: "Mock MCP",
    args: { text: "hi" },
    state: "completed",
    startedAt: 1000,
    endedAt: 1340,
    executed: true,
    resultPreview: "echo: hi",
    truncated: false,
  }),
};

afterEach(cleanup);

describe("ToolCard", () => {
  it("renders server, tool and result state", () => {
    render(<ToolCard part={part} />);
    expect(screen.getByText("echo")).toBeInTheDocument();
    expect(screen.getByText("Mock MCP")).toBeInTheDocument();
    expect(screen.getByText("ok")).toBeInTheDocument();
    expect(screen.getAllByText(/echo: hi/).length).toBeGreaterThan(0);
  });

  it("shows a non-zero shell exit as a warning, not a failed call", () => {
    const shellPart: Part = {
      ...part,
      id: "p2",
      payloadJson: JSON.stringify({
        ...JSON.parse(part.payloadJson),
        name: "bash",
        server: "mcp-chat",
        exitCode: 1,
        state: "completed",
      }),
    };
    render(<ToolCard part={shellPart} />);
    expect(screen.getByText("exit 1")).toBeInTheDocument();
    expect(screen.queryByText("ok")).not.toBeInTheDocument();
  });
});
