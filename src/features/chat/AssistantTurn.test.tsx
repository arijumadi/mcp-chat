import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { installFakeScroll, removeFakeScroll } from "@/test/fakeScroll";
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

beforeEach(installFakeScroll);

afterEach(() => {
  cleanup();
  removeFakeScroll();
});

describe("AssistantTurn", () => {
  it("shows a compact row for every thought and tool by default, plus the final answer", () => {
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

    // The summary is only a thin header. The rows are visible without clicking.
    expect(screen.getByRole("button", { name: /Used 2 tools/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByRole("button", { name: /echo/ })).toHaveTextContent("text=hi");
    expect(screen.getByRole("button", { name: /add/ })).toHaveTextContent("a=1 b=2");
    expect(screen.getAllByRole("button", { name: /Thought/ })).toHaveLength(2);
    expect(screen.getByText("Final answer is here.")).toBeInTheDocument();

    // Every row starts collapsed: no thought text, arguments or results.
    expect(screen.queryByText("think one")).not.toBeInTheDocument();
    expect(screen.queryByText("think two")).not.toBeInTheDocument();
    expect(screen.queryByText("Arguments")).not.toBeInTheDocument();
    expect(screen.queryByText("A pre-tool narration.")).not.toBeInTheDocument();
  });

  it("expands a thought and a tool independently", () => {
    render(
      <AssistantTurn
        messages={[
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
        ]}
        canRetry={false}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Thought/ }));
    expect(screen.getByText("think one")).toBeInTheDocument();
    expect(screen.queryByText("Arguments")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /echo/ }));
    expect(screen.getByText("Arguments")).toBeInTheDocument();
    expect(screen.getByText("think one")).toBeInTheDocument();
  });

  it("lets the header hide and show the whole list of rows", () => {
    render(
      <AssistantTurn
        messages={[
          assistant("m1", [
            part("p1", "tool_call", {
              callId: "call1",
              name: "echo",
              server: "Mock",
              args: { text: "hi" },
              state: "completed",
            }),
            part("p2", "text", { text: "Done." }),
          ]),
        ]}
        canRetry={false}
      />,
    );
    const header = screen.getByRole("button", { name: /Used 1 tool/ });
    fireEvent.click(header);
    expect(header).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: /echo/ })).not.toBeInTheDocument();
    expect(screen.getByText("Done.")).toBeInTheDocument();
    fireEvent.click(header);
    expect(screen.getByRole("button", { name: /echo/ })).toBeInTheDocument();
  });

  it("shows an edit row with change counts and opens its diff in one click", () => {
    render(
      <AssistantTurn
        messages={[
          assistant("m1", [
            part("p1", "tool_call", {
              callId: "call1",
              name: "edit",
              server: "mcp-chat",
              args: { filePath: "src/app.ts" },
              state: "completed",
              diff: "--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1 +1 @@\n-old\n+new\n",
            }),
          ]),
        ]}
        canRetry={false}
      />,
    );
    const row = screen.getByRole("button", { name: /Edit/ });
    expect(row).toHaveTextContent("src/app.ts");
    expect(row).toHaveTextContent("+1 \u22121");
    expect(screen.queryByText("-old")).not.toBeInTheDocument();
    // One click on the row shows the colored diff; there is no second toggle.
    fireEvent.click(row);
    expect(screen.getByText("-old")).toHaveClass("diff-line-removed");
    expect(screen.getByText("+new")).toHaveClass("diff-line-added");
    expect(screen.queryByText("Arguments")).not.toBeInTheDocument();
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
    expect(screen.getAllByRole("button", { name: /^Thought/ })).toHaveLength(2);
    expect(screen.queryByText("think one")).not.toBeInTheDocument();
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

  it("makes a thought follow its text only while its own message is streaming", () => {
    const text = "x".repeat(400);
    const turn = (status: string) => (
      <AssistantTurn
        messages={[assistant("m1", [part("p1", "reasoning", { text })], status)]}
        canRetry={false}
      />
    );
    const { unmount } = render(turn("streaming"));
    fireEvent.click(screen.getByRole("button", { name: /^Thought/ }));
    expect(screen.getByRole("region", { name: "Thought text" }).scrollTop).toBe(300);
    unmount();

    // A finished message opens at the top.
    render(turn("completed"));
    fireEvent.click(screen.getByRole("button", { name: /^Thought/ }));
    expect(screen.getByRole("region", { name: "Thought text" }).scrollTop).toBe(0);
  });
});
