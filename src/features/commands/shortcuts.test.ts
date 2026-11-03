import { describe, expect, it } from "vitest";
import { ariaKeyShortcuts, formatShortcut, isEditable, matchesChord, parseChord } from "./shortcuts";

function key(init: KeyboardEventInit & { key: string }): KeyboardEvent {
  return new KeyboardEvent("keydown", init);
}

describe("parseChord", () => {
  it("reads modifiers and keys case-insensitively", () => {
    expect(parseChord("Mod+Shift+P")).toEqual({ mod: true, shift: true, alt: false, key: "p" });
    expect(parseChord("ctrl+enter")).toEqual({ mod: true, shift: false, alt: false, key: "enter" });
    expect(parseChord("Esc")).toEqual({ mod: false, shift: false, alt: false, key: "escape" });
    expect(parseChord("Alt+Backspace").alt).toBe(true);
  });

  it("rejects a shortcut with no key", () => {
    expect(() => parseChord("Mod+Shift")).toThrow(/no key/);
  });
});

describe("matchesChord", () => {
  it("uses Ctrl for Mod on Windows and Linux", () => {
    const chord = parseChord("Mod+K");
    expect(matchesChord(key({ key: "k", ctrlKey: true }), chord, false)).toBe(true);
    expect(matchesChord(key({ key: "K", ctrlKey: true }), chord, false)).toBe(true);
    expect(matchesChord(key({ key: "k" }), chord, false)).toBe(false);
    // The Windows/Super key is not Mod.
    expect(matchesChord(key({ key: "k", metaKey: true }), chord, false)).toBe(false);
  });

  it("uses Cmd for Mod on macOS, leaving Ctrl to the text field", () => {
    const chord = parseChord("Mod+K");
    expect(matchesChord(key({ key: "k", metaKey: true }), chord, true)).toBe(true);
    // Ctrl+K is "kill line" in a Mac text field.
    expect(matchesChord(key({ key: "k", ctrlKey: true }), chord, true)).toBe(false);
    expect(matchesChord(key({ key: "k", metaKey: true, ctrlKey: true }), chord, true)).toBe(false);
  });

  it("matches a chord without Mod only when no Mod key is held", () => {
    const esc = parseChord("Esc");
    expect(matchesChord(key({ key: "Escape" }), esc, false)).toBe(true);
    expect(matchesChord(key({ key: "Escape", ctrlKey: true }), esc, false)).toBe(false);
  });

  it("does not match when extra modifiers are held", () => {
    const chord = parseChord("Mod+K");
    expect(matchesChord(key({ key: "k", ctrlKey: true, shiftKey: true }), chord, false)).toBe(false);
    expect(matchesChord(key({ key: "k", ctrlKey: true, altKey: true }), chord, false)).toBe(false);
    // Ctrl+Shift+Enter is a different chord from Ctrl+Enter.
    expect(matchesChord(key({ key: "Enter", ctrlKey: true, shiftKey: true }), parseChord("Mod+Enter"), false)).toBe(false);
    expect(matchesChord(key({ key: "Enter", ctrlKey: true, shiftKey: true }), parseChord("Mod+Shift+Enter"), false)).toBe(true);
  });

  it("matches named keys", () => {
    expect(matchesChord(key({ key: "Escape" }), parseChord("Esc"), false)).toBe(true);
    expect(matchesChord(key({ key: "Backspace", ctrlKey: true }), parseChord("Mod+Backspace"), false)).toBe(true);
  });
});

describe("formatShortcut", () => {
  it("writes Ctrl-style text on Windows and Linux", () => {
    expect(formatShortcut("Mod+Shift+P", false)).toBe("Ctrl+Shift+P");
    expect(formatShortcut("Mod+Enter", false)).toBe("Ctrl+Enter");
    expect(formatShortcut("Esc", false)).toBe("Escape");
  });

  it("writes symbols on macOS", () => {
    expect(formatShortcut("Mod+Shift+P", true)).toBe("⇧⌘P");
    expect(formatShortcut("Mod+Enter", true)).toBe("⌘↩");
    expect(formatShortcut("Mod+Backspace", true)).toBe("⌘⌫");
    expect(formatShortcut("Esc", true)).toBe("Esc");
  });
});

describe("ariaKeyShortcuts", () => {
  it("writes the value assistive technology expects", () => {
    expect(ariaKeyShortcuts("Mod+Enter", false)).toBe("Control+Enter");
    expect(ariaKeyShortcuts("Mod+Shift+Enter", false)).toBe("Control+Shift+Enter");
    expect(ariaKeyShortcuts("Mod+Enter", true)).toBe("Meta+Enter");
    expect(ariaKeyShortcuts("Mod+Backspace", false)).toBe("Control+Backspace");
    expect(ariaKeyShortcuts("Esc", false)).toBe("Escape");
    expect(ariaKeyShortcuts("Mod+K", false)).toBe("Control+K");
  });
});

describe("isEditable", () => {
  it("recognizes text inputs but not buttons", () => {
    expect(isEditable(document.createElement("textarea"))).toBe(true);
    expect(isEditable(document.createElement("input"))).toBe(true);
    expect(isEditable(document.createElement("button"))).toBe(false);
    expect(isEditable(null)).toBe(false);
    const editable = document.createElement("div");
    Object.defineProperty(editable, "isContentEditable", { value: true });
    expect(isEditable(editable)).toBe(true);
  });
});
