import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
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

describe("ToolCard", () => {
  it("renders server, tool and result state", () => {
    render(<ToolCard part={part} />);
    expect(screen.getByText("echo")).toBeInTheDocument();
    expect(screen.getByText("Mock MCP")).toBeInTheDocument();
    expect(screen.getByText("ok")).toBeInTheDocument();
    expect(screen.getAllByText(/echo: hi/).length).toBeGreaterThan(0);
  });
});
