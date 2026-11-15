import { describe, expect, it } from "vitest";
import { highlightLines, languageForPath, splitHighlightedLines } from "./highlight";

describe("workspace syntax highlighting", () => {
  it("maps common file extensions to highlighter languages", () => {
    expect(languageForPath("src/main.ts")).toBe("typescript");
    expect(languageForPath("crates/core/src/lib.rs")).toBe("rust");
    expect(languageForPath("README.md")).toBe("markdown");
    expect(languageForPath("Dockerfile")).toBe("dockerfile");
    expect(languageForPath("notes.txt")).toBeNull();
  });

  it("keeps a highlighted line self-contained when a span crosses a newline", () => {
    const lines = splitHighlightedLines(
      '<span class="hljs-comment">first\nsecond</span>\nplain',
    );
    expect(lines).toHaveLength(3);
    expect(lines[0]).toBe('<span class="hljs-comment">first</span>');
    expect(lines[1]).toBe('<span class="hljs-comment">second</span>');
    expect(lines[2]).toBe("plain");
  });

  it("escapes unknown file types instead of injecting markup", () => {
    const lines = highlightLines('<script>alert("x")</script>\nnext', "notes.txt");
    expect(lines).toEqual(["&lt;script&gt;alert(\"x\")&lt;/script&gt;", "next"]);
  });

  it("highlights code and preserves the original line count", () => {
    const code = "/* first\nsecond */\nconst value = 1;";
    const lines = highlightLines(code, "main.ts");
    expect(lines).toHaveLength(3);
    expect(lines[0]).toContain("hljs-comment");
    expect(lines[0]?.endsWith("</span>")).toBe(true);
    expect(lines[1]).toContain("hljs-comment");
    expect(lines[2]).toContain("hljs-keyword");
  });
});
