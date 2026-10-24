import { describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { AssistantTurn } from "./AssistantTurn";
import type { MessageWithParts, Part } from "@/lib/types";

function part(id: string, kind: string, payload: unknown): Part {
  return {
    id,
    messageId: "unused",
    idx: 0,
    kind,
    payloadJson: JSON.stringify(payload),
  };
}

function assistant(
  id: string,
  parts: Part[],
  status = "completed",
): MessageWithParts {
  return {
    id,
    conversationId: "c1",
    role: "assistant",
    createdAt: Date.now(),
    status,
    error: null,
    parts,
  };
}

describe("AssistantTurn", () => {
  it("collapses tools and thoughts behind a summary, then shows the final answer", () => {
    const messages = [
      assistant("m1", [
        part("p1", "reasoning", { text: "think one" }),
        part("p2", "tool_call", {
          callId: "call1",
          name: "echo",
          server: "Mock",
          args: { text: "hi" },
          state: "completed",
        }),
      ]),
      assistant("m2", [
        part("p3", "reasoning", { text: "think two" }),
        part("p4", "tool_call", {
          callId: "call2",
          name: "add",
          server: "Mock",
          args: { a: 1, b: 2 },
          state: "completed",
        }),
      ]),
      assistant("m3", [part("p5", "text", { text: "Final answer is here." })]),
    ];

    render(<AssistantTurn messages={messages} canRetry={false} />);

    expect(
      screen.getByRole("button", { name: /Used 2 tools · 2 thoughts/ }),
    ).toBeInTheDocument();
    expect(screen.getByText("Final answer is here.")).toBeInTheDocument();
    expect(screen.queryByText("echo")).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: /Used 2 tools · 2 thoughts/ }),
    );

    expect(screen.getAllByText("echo").length).toBeGreaterThan(0);
    expect(screen.getAllByText("add").length).toBeGreaterThan(0);
    expect(screen.getByText("Thought 1")).toBeInTheDocument();
    expect(screen.getByText("Thought 2")).toBeInTheDocument();
    expect(screen.getByText("Final answer is here.")).toBeInTheDocument();
  });
});
