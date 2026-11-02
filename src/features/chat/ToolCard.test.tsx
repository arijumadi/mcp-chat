import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ToolCard } from "./ToolCard";
import type { Part } from "@/lib/types";

function toolPart(id: string, payload: Record<string, unknown>): Part {
  return {
    id,
    messageId: "m1",
    idx: 0,
    kind: "tool_call",
    payloadJson: JSON.stringify(payload),
  };
}

const echo = toolPart("p1", {
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
});

const DIFF = [
  "--- a/src/app.ts",
  "+++ b/src/app.ts",
  "@@ -1,2 +1,2 @@",
  " keep",
  "-old",
  "+new",
  "",
].join("\n");

const edit = toolPart("p3", {
  callId: "call3",
  name: "edit",
  server: "mcp-chat",
  args: { filePath: "src/app.ts", oldString: "old", newString: "new" },
  state: "completed",
  startedAt: 1000,
  endedAt: 1120,
  executed: true,
  resultPreview: "Edited src/app.ts: replaced 1 occurrence(s).",
  diff: DIFF,
});

afterEach(cleanup);

describe("ToolCard", () => {
  it("is a collapsed one-line row with the tool, its main argument and the duration", () => {
    render(<ToolCard part={echo} />);
    const row = screen.getByRole("button", { name: /echo/ });
    expect(row).toHaveAttribute("aria-expanded", "false");
    expect(row).toHaveTextContent("Mock MCP/echo");
    expect(row).toHaveTextContent("text=hi");
    expect(row).toHaveTextContent("340ms");
    expect(row).toHaveAttribute("title", expect.stringContaining("Mock MCP/echo text=hi"));
    // Details stay hidden until the row is opened.
    expect(screen.queryByText("Arguments")).not.toBeInTheDocument();
    expect(screen.queryByText("Result")).not.toBeInTheDocument();
    expect(screen.queryByText("echo: hi")).not.toBeInTheDocument();
  });

  it("shows Arguments and Result after expanding the row", () => {
    render(<ToolCard part={echo} />);
    fireEvent.click(screen.getByRole("button", { name: /echo/ }));
    expect(screen.getByRole("button", { name: /echo/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Arguments")).toBeInTheDocument();
    expect(screen.getByText("Result")).toBeInTheDocument();
    expect(screen.getByText(/"text": "hi"/)).toBeInTheDocument();
    expect(screen.getByText("echo: hi")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Changes/ })).not.toBeInTheDocument();
  });

  it("previews an edit with its change counts without showing the diff", () => {
    render(<ToolCard part={edit} />);
    const row = screen.getByRole("button", { name: /Edit/ });
    expect(row).toHaveTextContent("Edit");
    expect(row).toHaveTextContent("src/app.ts");
    expect(row).toHaveTextContent("+1 \u22121");
    expect(screen.queryByText("-old")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Changes/ })).not.toBeInTheDocument();
  });

  it("keeps the diff collapsed inside the expanded tool, then opens it", () => {
    render(<ToolCard part={edit} />);
    fireEvent.click(screen.getByRole("button", { name: /Edit/ }));

    expect(screen.getByText("Arguments")).toBeInTheDocument();
    expect(screen.getByText("Result")).toBeInTheDocument();
    const changes = screen.getByRole("button", { name: /Changes/ });
    expect(changes).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("-old")).not.toBeInTheDocument();

    fireEvent.click(changes);
    expect(screen.getByText("-old")).toHaveClass("diff-line-removed");
    expect(screen.getByText("+new")).toHaveClass("diff-line-added");
  });

  it("does not show a diff while the tool is still running", () => {
    render(
      <ToolCard
        part={toolPart("p4", {
          callId: "call4",
          name: "edit",
          server: "mcp-chat",
          args: { filePath: "src/app.ts" },
          state: "running",
          executed: false,
          diff: DIFF,
        })}
      />,
    );
    const row = screen.getByRole("button", { name: /Edit/ });
    expect(row).toHaveAttribute("aria-expanded", "false");
    expect(row).toHaveTextContent("running");
    fireEvent.click(row);
    expect(screen.queryByRole("button", { name: /Changes/ })).not.toBeInTheDocument();
  });

  it("shows a non-zero shell exit as a warning, not a failed call", () => {
    render(
      <ToolCard
        part={toolPart("p2", {
          callId: "call2",
          name: "bash",
          server: "mcp-chat",
          args: { command: "npm test" },
          state: "completed",
          startedAt: 1000,
          endedAt: 13000,
          executed: true,
          exitCode: 1,
        })}
      />,
    );
    const row = screen.getByRole("button", { name: /Bash/ });
    expect(row).toHaveTextContent("npm test");
    expect(row).toHaveTextContent("exit 1");
    expect(row).toHaveTextContent("12s");
    expect(screen.getByRole("img", { name: "completed with non-zero exit" })).toBeInTheDocument();
  });

  it("surfaces an error in the row and in the expanded body", () => {
    render(
      <ToolCard
        part={toolPart("p5", {
          callId: "call5",
          name: "edit",
          server: "mcp-chat",
          args: { filePath: "a.ts" },
          state: "error",
          executed: true,
          error: "oldString not found",
          resultPreview: "oldString not found",
        })}
      />,
    );
    const row = screen.getByRole("button", { name: /Edit/ });
    expect(row).toHaveTextContent("oldString not found");
    fireEvent.click(row);
    // The row line plus one alert block. The identical result preview is not
    // repeated as a separate Result section.
    expect(screen.getAllByText("oldString not found")).toHaveLength(2);
    expect(screen.getByRole("alert")).toHaveTextContent("oldString not found");
    expect(screen.queryByText("Result")).not.toBeInTheDocument();
  });

  it("keeps a queued call collapsed with its status visible", () => {
    render(
      <ToolCard
        part={toolPart("p6", {
          callId: "call6",
          name: "echo",
          server: "Mock MCP",
          args: {},
          state: "queued",
          executed: false,
          approval: "pending",
        })}
      />,
    );
    const row = screen.getByRole("button", { name: /echo/ });
    expect(row).toHaveAttribute("aria-expanded", "false");
    expect(row).toHaveTextContent("queued");
    expect(row).toHaveTextContent("needs approval");
  });
});
