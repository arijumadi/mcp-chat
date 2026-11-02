import { describe, expect, it } from "vitest";
import table from "../../shared/capabilities.json";
import { ALL_CAPABILITIES, BUILTIN_SERVER, capabilityOf, toolForRole } from "./capabilities";

describe("capabilities", () => {
  it("lists every tool of the shared table in order", () => {
    expect(ALL_CAPABILITIES.map((tool) => tool.name)).toEqual(table.tools.map((tool) => tool.name));
  });

  it("marks exactly the file-writing tools as producing a diff", () => {
    expect(ALL_CAPABILITIES.filter((tool) => tool.producesDiff).map((tool) => tool.name)).toEqual([
      "edit",
      "apply_patch",
      "write",
    ]);
    for (const tool of ALL_CAPABILITIES.filter((tool) => tool.producesDiff)) {
      expect(tool.risk).toBe("file_write");
    }
  });

  it("only treats built-in tools as capabilities", () => {
    expect(capabilityOf(BUILTIN_SERVER, "edit")?.label).toBe("Edit");
    expect(capabilityOf("Other MCP", "edit")).toBeUndefined();
    expect(capabilityOf(BUILTIN_SERVER, "nope")).toBeUndefined();
  });

  it("exposes the guard flags the backend validates", () => {
    expect(capabilityOf(BUILTIN_SERVER, "bash")?.overrideParam).toBe("allowFileWrites");
    expect(capabilityOf(BUILTIN_SERVER, "write")?.overrideParam).toBe("overwrite");
    expect(capabilityOf(BUILTIN_SERVER, "read")?.overrideParam).toBeNull();
  });

  it("resolves roles to tool names", () => {
    expect(toolForRole("modify_file")).toBe("apply_patch");
    expect(toolForRole("create_file")).toBe("write");
    expect(toolForRole("replace_once")).toBe("edit");
    expect(toolForRole("nope")).toBeUndefined();
  });

  it("gives every tool a label, and every role a real tool", () => {
    for (const tool of ALL_CAPABILITIES) expect(tool.label.length).toBeGreaterThan(0);
    for (const tool of Object.values(table.roles)) {
      expect(ALL_CAPABILITIES.some((capability) => capability.name === tool)).toBe(true);
    }
  });
});
