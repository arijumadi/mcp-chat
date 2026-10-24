import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
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

afterEach(() => {
  cleanup();
});

describe("AssistantTurn", () => {
  it("collapses tools and thoughts behind a summary, then shows the final answer", () => {
    const messages = [
      assistant("m1", [
        part("p1", "reasoning", { text: "think one" }),
        part("p1-note", "text", { text: "A pre-tool narration." }),
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
      screen.getByRole("button", { name: /Used 2 tools/ }),
    ).toBeInTheDocument();
    expect(screen.getByText("Final answer is here.")).toBeInTheDocument();
    expect(screen.queryByText("echo")).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: /Used 2 tools/ }),
    );

    expect(screen.getAllByText("echo").length).toBeGreaterThan(0);
    expect(screen.getAllByText("add").length).toBeGreaterThan(0);
    expect(screen.getByText("Thought 1")).toBeInTheDocument();
    expect(screen.getByText("Thought 2")).toBeInTheDocument();
    expect(screen.getByText("Final answer is here.")).toBeInTheDocument();
    expect(
      screen.queryByText("A pre-tool narration."),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Interim note")).not.toBeInTheDocument();
  });

  it("reports failed tool calls and total duration", () => {
    render(
      <AssistantTurn
        messages={[
          assistant("m1", [
            part("p1", "tool_call", {
              callId: "call1",
              name: "echo",
              server: "Mock",
              args: { text: "hi" },
              state: "error",
              startedAt: 1_000,
              endedAt: 2_500,
            }),
            part("p2", "tool_call", {
              callId: "call2",
              name: "add",
              server: "Mock",
              args: { a: 1, b: 2 },
              state: "completed",
              startedAt: 1_000,
              endedAt: 2_500,
            }),
          ]),
        ]}
        canRetry={false}
      />,
    );
    expect(
      screen.getByRole("button", { name: /Used 2 tools · 1 failed · 1.5s/ }),
    ).toBeInTheDocument();
  });

  it("names the tool that is currently running", () => {
    render(
      <AssistantTurn
        messages={[
          assistant(
            "m1",
            [
              part("p1", "tool_call", {
                callId: "call1",
                name: "echo",
                server: "Mock",
                args: { text: "hi" },
                state: "running",
              }),
            ],
            "streaming",
          ),
        ]}
        canRetry={false}
      />,
    );
    expect(screen.getByText("Running echo")).toBeInTheDocument();
  });

  it("shows thoughts when there are no tool calls", () => {
    render(
      <AssistantTurn
        messages={[
          assistant("m1", [
            part("p1", "reasoning", { text: "think one" }),
            part("p2", "reasoning", { text: "think two" }),
          ]),
        ]}
        canRetry={false}
      />,
    );
    expect(
      screen.getByRole("button", { name: /2 thoughts/ }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Used .* tools/)).not.toBeInTheDocument();
  });

  it("shows Thinking while reasoning is streaming", () => {
    render(
      <AssistantTurn
        messages={[
          assistant(
            "m1",
            [part("p1", "reasoning", { text: "working..." })],
            "streaming",
          ),
        ]}
        canRetry={false}
      />,
    );
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
  });
});
