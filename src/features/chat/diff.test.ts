import { describe, expect, it } from "vitest";
import { DIFF_TRUNCATED_MARKER, diffStats, parseDiff, statsForDiff } from "./diff";

describe("parseDiff", () => {
  it("classifies file headers, hunks, additions, removals and context", () => {
    const lines = parseDiff(
      "--- a/src/app.ts\n+++ b/src/app.ts\n@@ -1,3 +1,3 @@\n keep\n-old\n+new\n",
    );
    expect(lines.map((line) => line.kind)).toEqual([
      "file",
      "file",
      "hunk",
      "context",
      "removed",
      "added",
    ]);
    expect(lines[0]!.text).toBe("--- a/src/app.ts");
    expect(lines[4]!.text).toBe("-old");
  });

  it("returns nothing for an empty diff and drops only the trailing newline", () => {
    expect(parseDiff("")).toEqual([]);
    expect(parseDiff("\n")).toEqual([]);
    expect(parseDiff("+a\n\n")).toHaveLength(2);
  });

  it("does not mistake a removed line that starts with dashes for a file header", () => {
    // Removing "---" (a Markdown rule) renders as "----".
    const lines = parseDiff("--- a/doc.md\n+++ b/doc.md\n@@ -1 +1 @@\n----\n+***\n");
    expect(lines.map((line) => line.kind)).toEqual(["file", "file", "hunk", "removed", "added"]);
  });

  it("treats a lone --- line without a following +++ as a removal", () => {
    expect(parseDiff("--- not a header\n").map((line) => line.kind)).toEqual(["removed"]);
  });
});

describe("diffStats", () => {
  it("counts additions, removals and files across a multi-file diff", () => {
    const stats = statsForDiff(
      [
        "--- a/a.txt",
        "+++ b/a.txt",
        "@@ -1 +1 @@",
        "-one",
        "+two",
        "+three",
        "--- a/b.txt",
        "+++ b/b.txt",
        "@@ -1 +0,0 @@",
        "-gone",
        "",
      ].join("\n"),
    );
    expect(stats).toEqual({ added: 2, removed: 2, files: 2, truncated: false });
  });

  it("flags a truncated diff", () => {
    const stats = diffStats(parseDiff(`+a\n${DIFF_TRUNCATED_MARKER}\n`));
    expect(stats.truncated).toBe(true);
    expect(stats.added).toBe(1);
  });
});
