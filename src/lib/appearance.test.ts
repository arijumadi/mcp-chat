import { describe, expect, it } from "vitest";
import {
  DEFAULT_APPEARANCE,
  FONT_SIZE,
  appearanceSettings,
  applyAppearance,
  readAppearance,
  widthPx,
} from "./appearance";

describe("readAppearance", () => {
  it("returns the original look when nothing is stored", () => {
    expect(readAppearance({})).toEqual({
      density: "comfortable",
      fontSize: 15,
      chatWidth: "default",
    });
    expect(readAppearance({})).toEqual(DEFAULT_APPEARANCE);
  });

  it("reads stored values", () => {
    expect(
      readAppearance({ "ui.density": "compact", "ui.fontSize": "17", "ui.chatWidth": "wide" }),
    ).toEqual({ density: "compact", fontSize: 17, chatWidth: "wide" });
  });

  it("falls back to defaults for anything it does not recognize", () => {
    expect(
      readAppearance({ "ui.density": "cozy", "ui.fontSize": "big", "ui.chatWidth": "huge" }),
    ).toEqual(DEFAULT_APPEARANCE);
    expect(readAppearance({ "ui.fontSize": "" })).toEqual(DEFAULT_APPEARANCE);
    expect(readAppearance({ "ui.fontSize": "   " })).toEqual(DEFAULT_APPEARANCE);
    expect(readAppearance({ "ui.fontSize": "NaN" })).toEqual(DEFAULT_APPEARANCE);
    expect(readAppearance({ "ui.fontSize": "Infinity" })).toEqual(DEFAULT_APPEARANCE);
  });

  it("keeps the font size inside the supported range and on whole pixels", () => {
    expect(readAppearance({ "ui.fontSize": "2" }).fontSize).toBe(FONT_SIZE.min);
    expect(readAppearance({ "ui.fontSize": "-40" }).fontSize).toBe(FONT_SIZE.min);
    expect(readAppearance({ "ui.fontSize": "400" }).fontSize).toBe(FONT_SIZE.max);
    expect(readAppearance({ "ui.fontSize": "15.6" }).fontSize).toBe(16);
  });
});

describe("appearanceSettings", () => {
  it("round-trips through readAppearance", () => {
    const appearance = { density: "compact", fontSize: 18, chatWidth: "full" } as const;
    expect(readAppearance(appearanceSettings(appearance))).toEqual(appearance);
  });
});

describe("applyAppearance", () => {
  it("sets the density attribute and the CSS variables", () => {
    const root = document.createElement("div");
    applyAppearance({ density: "compact", fontSize: 17, chatWidth: "wide" }, root);
    expect(root.dataset.density).toBe("compact");
    expect(root.style.getPropertyValue("--chat-fs")).toBe("17px");
    expect(root.style.getPropertyValue("--chat-width")).toBe("960px");
  });

  it("lets the transcript use the full width", () => {
    const root = document.createElement("div");
    applyAppearance({ ...DEFAULT_APPEARANCE, chatWidth: "full" }, root);
    expect(root.style.getPropertyValue("--chat-width")).toBe("100%");
    expect(widthPx("full")).toBeNull();
  });

  it("applies the defaults as today's exact values", () => {
    const root = document.createElement("div");
    applyAppearance(DEFAULT_APPEARANCE, root);
    expect(root.dataset.density).toBe("comfortable");
    expect(root.style.getPropertyValue("--chat-fs")).toBe("15px");
    expect(root.style.getPropertyValue("--chat-width")).toBe("760px");
  });
});
