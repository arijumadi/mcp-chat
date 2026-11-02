import { describe, expect, it } from "vitest";
import type { ToolCallPayload } from "@/lib/types";
import { shortPath, summarizeTool, thoughtDuration } from "./activity";

function tool(overrides: Partial<ToolCallPayload>): ToolCallPayload {
  return {
    callId: "c1",
    name: "echo",
    server: "mcp-chat",
    args: {},
    state: "completed",
    executed: true,
    ...overrides,
  };
}

const DIFF = "--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1 +1,2 @@\n-a\n+b\n+c\n";

describe("summarizeTool", () => {
  it("previews a read with its path, offset and limit", () => {
    const summary = summarizeTool(
      tool({ name: "read", args: { filePath: "ChatRoom.tsx", offset: 168, limit: 50 } }),
    );
    expect(summary.label).toBe("Read");
    expect(summary.target).toBe("ChatRoom.tsx offset=168 limit=50");
  });

  it("keeps the last three segments of a long path and the full text in the title", () => {
    expect(shortPath("/home/ari/work/app/src/components/ChatRoom.tsx")).toBe(
      "…/src/components/ChatRoom.tsx",
    );
    expect(shortPath("src/app.ts")).toBe("src/app.ts");
    const summary = summarizeTool(tool({ name: "read", args: { filePath: "/a/b/c/d/e.ts" } }));
    expect(summary.target).toBe("…/c/d/e.ts");
  });

  it("shows edit change counts from the diff", () => {
    const summary = summarizeTool(
      tool({ name: "edit", args: { filePath: "src/app.ts" }, diff: DIFF }),
    );
    expect(summary).toMatchObject({ label: "Edit", target: "src/app.ts", result: "+2 \u22121" });
    expect(summary.title).toBe("Edit src/app.ts +2 \u22121");
  });

  it("omits the stats while the edit has no diff yet", () => {
    const summary = summarizeTool(
      tool({ name: "edit", args: { filePath: "src/app.ts" }, state: "running" }),
    );
    expect(summary.result).toBe("");
    expect(summary.stateWord).toBe("running");
  });

  it("summarizes a patch by file count and change counts", () => {
    const patchText =
      "*** Begin Patch\n*** Add File: a.txt\n+x\n*** Update File: b.txt\n@@\n-a\n+b\n*** Delete File: c.txt\n*** End Patch";
    const summary = summarizeTool(tool({ name: "apply_patch", args: { patchText }, diff: DIFF }));
    expect(summary).toMatchObject({ label: "Patch", target: "3 files", result: "+2 \u22121" });
  });

  it("never puts the patch body or file content in the preview", () => {
    const summary = summarizeTool(
      tool({
        name: "write",
        args: { filePath: "a.txt", content: "SECRET BODY" },
      }),
    );
    expect(summary.title).not.toContain("SECRET BODY");
  });

  it("shows the first line of a shell command and its exit code", () => {
    const ok = summarizeTool(
      tool({ name: "bash", args: { command: "npm test\necho done" }, exitCode: 0 }),
    );
    expect(ok.target).toBe("npm test …");
    expect(ok.result).toBe("exit 0");
    expect(ok.tone).toBe("muted");
    const bad = summarizeTool(tool({ name: "bash", args: { command: "false" }, exitCode: 1 }));
    expect(bad.result).toBe("exit 1");
    expect(bad.tone).toBe("warn");
  });

  it("reads the match count of a grep from the first result line only", () => {
    const summary = summarizeTool(
      tool({
        name: "grep",
        args: { path: "/home/ari/proj/src/components", pattern: "scrollTo|scrollContainerRef" },
        resultPreview: "21 matching line(s):\nsrc/a.ts:1: scrollTo",
      }),
    );
    expect(summary.target).toBe("…/proj/src/components pattern=scrollTo|scrollContainerRef");
    expect(summary.result).toBe("21 matches");
    expect(
      summarizeTool(
        tool({ name: "grep", args: { pattern: "x" }, resultPreview: "No matches for /x/." }),
      ).result,
    ).toBe("no matches");
  });

  it("labels an MCP tool as server/tool with key=value arguments", () => {
    const summary = summarizeTool(
      tool({ name: "echo", server: "Mock MCP", args: { text: "hello", count: 2 } }),
    );
    expect(summary).toMatchObject({
      prefix: "Mock MCP/",
      label: "echo",
      target: "text=hello count=2",
    });
  });

  it("uses the inner server, tool and arguments of an mcp_call_tool", () => {
    const summary = summarizeTool(
      tool({
        name: "mcp_call_tool",
        args: { server: "docs", tool: "search", arguments: { query: "diff view" } },
      }),
    );
    expect(summary).toMatchObject({ prefix: "docs/", label: "search", target: "query=diff view" });
  });

  it("skips sensitive keys, nested values and empty strings in the fallback", () => {
    const summary = summarizeTool(
      tool({
        name: "mystery",
        server: "Other",
        args: {
          apiKey: "abc",
          authToken: "def",
          nested: { a: 1 },
          list: [1],
          empty: "",
          city: "Paris",
        },
      }),
    );
    expect(summary.target).toBe("city=Paris");
  });

  it("limits the fallback to three pairs and collapses whitespace", () => {
    const summary = summarizeTool(
      tool({ name: "x", server: "S", args: { a: "1\n2", b: 2, c: 3, d: 4 } }),
    );
    expect(summary.target).toBe("a=1 2 b=2 c=3");
  });

  it("shows the first error line in the danger tone", () => {
    const summary = summarizeTool(
      tool({ name: "edit", args: { filePath: "a.ts" }, state: "error", error: "no match\nmore" }),
    );
    expect(summary.result).toBe("no match");
    expect(summary.tone).toBe("danger");
  });

  it("reports stopped calls and pending approvals", () => {
    expect(summarizeTool(tool({ state: "aborted" }))).toMatchObject({
      result: "stopped",
      tone: "warn",
    });
    expect(summarizeTool(tool({ approval: "pending", state: "running" })).needsApproval).toBe(true);
  });

  it("formats the duration only when both timestamps exist", () => {
    expect(summarizeTool(tool({ startedAt: 1000, endedAt: 13000 })).duration).toBe("12s");
    expect(summarizeTool(tool({ startedAt: 1000 })).duration).toBe("");
  });

  it("lists the calls of a batch", () => {
    const summary = summarizeTool(
      tool({
        name: "mcp_execute",
        args: {
          calls: [
            { server: "a", tool: "x" },
            { server: "b", tool: "y" },
            { server: "c", tool: "z" },
            { server: "d", tool: "w" },
          ],
        },
      }),
    );
    expect(summary.label).toBe("MCP batch");
    expect(summary.target).toBe("a/x, b/y, c/z +1");
  });
});

describe("thoughtDuration", () => {
  it("is empty without timing and never invents one", () => {
    expect(thoughtDuration({ text: "hmm" })).toBe("");
  });

  it("uses durationMs or a start/end pair when present", () => {
    expect(thoughtDuration({ durationMs: 12_000 })).toBe("12s");
    expect(thoughtDuration({ startedAt: 1_000, endedAt: 3_500 })).toBe("2.5s");
    expect(thoughtDuration({ startedAt: 5_000, endedAt: 1_000 })).toBe("");
  });
});
