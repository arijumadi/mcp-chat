import { describe, expect, it } from "vitest";
import { parseCompactCommand } from "./slashCommands";

describe("parseCompactCommand", () => {
  it("recognizes /compact with and without a focus", () => {
    expect(parseCompactCommand("/compact")).toEqual({ focus: "" });
    expect(parseCompactCommand("  /compact  ")).toEqual({ focus: "" });
    expect(parseCompactCommand("/compact keep the schema decisions")).toEqual({
      focus: "keep the schema decisions",
    });
    expect(parseCompactCommand("/COMPACT Keep This")).toEqual({ focus: "Keep This" });
    expect(parseCompactCommand("/compact line one\nline two")).toEqual({
      focus: "line one\nline two",
    });
  });

  it("ignores text that only starts with the same letters", () => {
    expect(parseCompactCommand("/compactor")).toBeNull();
    expect(parseCompactCommand("/compact-all")).toBeNull();
    expect(parseCompactCommand("please /compact this")).toBeNull();
    expect(parseCompactCommand("compact")).toBeNull();
    expect(parseCompactCommand("")).toBeNull();
  });
});
