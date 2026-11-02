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

  it("opens a finished edit straight onto its diff with one click", () => {
    render(<ToolCard part={edit} />);
    expect(screen.queryByRole("button", { name: /Changes/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Edit/ }));

    // The diff is already open: no second click on a Changes toggle.
    expect(screen.getByRole("button", { name: /Changes/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByText("-old")).toHaveClass("diff-line-removed");
    expect(screen.getByText("+new")).toHaveClass("diff-line-added");
    // And nothing else: no Arguments, no Result.
    expect(screen.queryByText("Arguments")).not.toBeInTheDocument();
    expect(screen.queryByText("Result")).not.toBeInTheDocument();
    expect(screen.queryByText(/oldString/)).not.toBeInTheDocument();
    expect(screen.queryByText(/replaced 1 occurrence/)).not.toBeInTheDocument();
  });

  it("opens a new-file write onto its diff", () => {
    render(
      <ToolCard
        part={toolPart("w1", {
          callId: "w1",
          name: "write",
          server: "mcp-chat",
          args: { filePath: "notes/new.txt", content: "hello" },
          state: "completed",
          executed: true,
          resultPreview: "Wrote notes/new.txt (5 bytes).",
          diff: "--- a/notes/new.txt\n+++ b/notes/new.txt\n@@ -0,0 +1 @@\n+hello\n",
        })}
      />,
    );
    const row = screen.getByRole("button", { name: /Write/ });
    expect(row).toHaveTextContent("notes/new.txt");
    expect(row).toHaveTextContent("+1 \u22120");
    fireEvent.click(row);
    expect(screen.getByText("+hello")).toHaveClass("diff-line-added");
    expect(screen.queryByText("Arguments")).not.toBeInTheDocument();
    expect(screen.queryByText("Result")).not.toBeInTheDocument();
  });

  it("shows every file of a multi-file patch in one open diff", () => {
    const patchDiff = [
      "--- a/a.txt",
      "+++ b/a.txt",
      "@@ -1 +1 @@",
      "-one",
      "+two",
      "--- a/b.txt",
      "+++ b/b.txt",
      "@@ -0,0 +1 @@",
      "+added",
      "",
    ].join("\n");
    render(
      <ToolCard
        part={toolPart("a1", {
          callId: "a1",
          name: "apply_patch",
          server: "mcp-chat",
          args: { patchText: "*** Begin Patch\n*** Update File: a.txt\n*** Add File: b.txt\n*** End Patch" },
          state: "completed",
          executed: true,
          resultPreview: "Applied patch (2 file(s)).",
          diff: patchDiff,
        })}
      />,
    );
    expect(screen.getByRole("button", { name: /Patch/ })).toHaveTextContent("2 files");
    fireEvent.click(screen.getByRole("button", { name: /Patch/ }));
    expect(screen.getAllByRole("button", { name: /Changes/ })).toHaveLength(1);
    expect(screen.getByRole("button", { name: /Changes/ })).toHaveTextContent("2 files");
    expect(screen.getByText("--- a/a.txt")).toBeInTheDocument();
    expect(screen.getByText("--- a/b.txt")).toBeInTheDocument();
    expect(screen.getByText("-one")).toBeInTheDocument();
    expect(screen.getByText("+added")).toBeInTheDocument();
    expect(screen.queryByText("Arguments")).not.toBeInTheDocument();
    expect(screen.queryByText("Result")).not.toBeInTheDocument();
  });

  it("collapses the row again and reopens it onto an open diff", () => {
    render(<ToolCard part={edit} />);
    const row = screen.getByRole("button", { name: /Edit/ });
    fireEvent.click(row);
    // The user may fold the diff itself...
    fireEvent.click(screen.getByRole("button", { name: /Changes/ }));
    expect(screen.queryByText("+new")).not.toBeInTheDocument();
    // ...but closing and reopening the row starts open again.
    fireEvent.click(row);
    expect(screen.queryByRole("button", { name: /Changes/ })).not.toBeInTheDocument();
    fireEvent.click(row);
    expect(screen.getByText("+new")).toBeInTheDocument();
  });

  it("keeps the arguments and the error when a file mutation fails", () => {
    render(
      <ToolCard
        part={toolPart("f1", {
          callId: "f1",
          name: "write",
          server: "mcp-chat",
          args: { filePath: "keep.txt", content: "new" },
          state: "error",
          executed: false,
          error: "File already exists. Use apply_patch to modify it, or pass overwrite: true to replace the whole file.",
          resultPreview: "File already exists. Use apply_patch to modify it, or pass overwrite: true to replace the whole file.",
        })}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Write/ }));
    expect(screen.getByRole("alert")).toHaveTextContent("File already exists.");
    expect(screen.getByText("Arguments")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Changes/ })).not.toBeInTheDocument();
  });

  it("falls back to arguments and result when a finished mutation has no diff", () => {
    render(
      <ToolCard
        part={toolPart("n1", {
          callId: "n1",
          name: "edit",
          server: "mcp-chat",
          args: { filePath: "src/app.ts", oldString: "same", newString: "same" },
          state: "completed",
          executed: true,
          resultPreview: "Edited src/app.ts: replaced 1 occurrence(s).",
        })}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Edit/ }));
    expect(screen.getByText("Arguments")).toBeInTheDocument();
    expect(screen.getByText("Result")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Changes/ })).not.toBeInTheDocument();
  });

  it("does not treat a same-named tool from another server as a file mutation", () => {
    render(
      <ToolCard
        part={toolPart("o1", {
          callId: "o1",
          name: "write",
          server: "Other MCP",
          args: { path: "x" },
          state: "completed",
          executed: true,
          resultPreview: "done",
          diff: DIFF,
        })}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Other MCP\/write/ }));
    expect(screen.getByText("Arguments")).toBeInTheDocument();
    expect(screen.getByText("Result")).toBeInTheDocument();
    // Its diff, if it has one, stays behind the usual collapsed toggle.
    expect(screen.getByRole("button", { name: /Changes/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  it("keeps the arguments and result view for bash", () => {
    render(
      <ToolCard
        part={toolPart("b1", {
          callId: "b1",
          name: "bash",
          server: "mcp-chat",
          args: { command: "npm test" },
          state: "completed",
          executed: true,
          exitCode: 0,
          resultPreview: "all tests passed",
        })}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Bash/ }));
    expect(screen.getByText("Arguments")).toBeInTheDocument();
    expect(screen.getByText("Result")).toBeInTheDocument();
    expect(screen.getByText(/"command": "npm test"/)).toBeInTheDocument();
    expect(screen.getByText("all tests passed")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Changes/ })).not.toBeInTheDocument();
  });

  it("keeps the arguments and result view for read", () => {
    render(
      <ToolCard
        part={toolPart("r1", {
          callId: "r1",
          name: "read",
          server: "mcp-chat",
          args: { filePath: "src/app.ts", offset: 1, limit: 20 },
          state: "completed",
          executed: true,
          resultPreview: "src/app.ts (lines 1-20 of 40)",
        })}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /Read/ }));
    expect(screen.getByText("Arguments")).toBeInTheDocument();
    expect(screen.getByText("Result")).toBeInTheDocument();
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
    expect(screen.getByText("Arguments")).toBeInTheDocument();
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
