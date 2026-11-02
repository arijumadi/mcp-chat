import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AppearanceSettings, ShortcutList } from "./AppearanceSettings";
import { useApp } from "@/app/store";

const saved: Array<{ key: string; value: string }> = [];

beforeEach(() => {
  saved.length = 0;
  window.__MCP_CHAT_MOCK__ = {
    invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
      if (command === "set_setting") saved.push({ key: String(args?.key), value: String(args?.value) });
      return null as T;
    },
    subscribe: () => () => undefined,
  };
  useApp.setState({ settings: {}, toasts: [] });
  const root = document.documentElement;
  root.removeAttribute("data-density");
  root.style.removeProperty("--chat-fs");
  root.style.removeProperty("--chat-width");
});

afterEach(() => {
  cleanup();
  delete window.__MCP_CHAT_MOCK__;
});

const checked = (name: string) =>
  screen.getByRole("radio", { name }).getAttribute("aria-checked");

describe("AppearanceSettings", () => {
  it("starts on the defaults", () => {
    render(<AppearanceSettings />);
    expect(checked("Comfortable")).toBe("true");
    expect(checked("Compact")).toBe("false");
    expect(checked("Default")).toBe("true");
    expect(screen.getByText("15px")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reset to defaults" })).toBeDisabled();
  });

  it("groups choices so assistive technology reads them as one control", () => {
    render(<AppearanceSettings />);
    expect(screen.getByRole("radiogroup", { name: "Density" })).toBeInTheDocument();
    expect(screen.getByRole("radiogroup", { name: "Chat width" })).toBeInTheDocument();
    expect(screen.getByRole("slider", { name: "Chat text size" })).toHaveAttribute(
      "aria-valuetext",
      "15 pixels",
    );
  });

  it("applies density at once and saves it", async () => {
    render(<AppearanceSettings />);
    fireEvent.click(screen.getByRole("radio", { name: "Compact" }));
    expect(document.documentElement.dataset.density).toBe("compact");
    expect(checked("Compact")).toBe("true");
    await waitFor(() => expect(saved.find((item) => item.key === "ui.density")?.value).toBe("compact"));
    expect(useApp.getState().settings["ui.density"]).toBe("compact");
  });

  it("applies the text size and the width", async () => {
    render(<AppearanceSettings />);
    fireEvent.change(screen.getByRole("slider", { name: "Chat text size" }), { target: { value: "18" } });
    expect(document.documentElement.style.getPropertyValue("--chat-fs")).toBe("18px");
    expect(screen.getByText("18px")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "Wide" }));
    expect(document.documentElement.style.getPropertyValue("--chat-width")).toBe("960px");
    fireEvent.click(screen.getByRole("radio", { name: "Full" }));
    expect(document.documentElement.style.getPropertyValue("--chat-width")).toBe("100%");
    await waitFor(() => expect(saved.some((item) => item.key === "ui.fontSize" && item.value === "18")).toBe(true));
  });

  it("never stores an out-of-range size", async () => {
    await useApp.getState().setAppearance({ fontSize: 400 });
    expect(useApp.getState().settings["ui.fontSize"]).toBe("19");
    expect(document.documentElement.style.getPropertyValue("--chat-fs")).toBe("19px");
    await useApp.getState().setAppearance({ fontSize: -3 });
    expect(useApp.getState().settings["ui.fontSize"]).toBe("13");
  });

  it("resets everything to the defaults", async () => {
    useApp.setState({ settings: { "ui.density": "compact", "ui.fontSize": "18", "ui.chatWidth": "full" } });
    render(<AppearanceSettings />);
    const reset = screen.getByRole("button", { name: "Reset to defaults" });
    expect(reset).toBeEnabled();
    fireEvent.click(reset);
    await waitFor(() => expect(useApp.getState().settings["ui.chatWidth"]).toBe("default"));
    expect(useApp.getState().settings["ui.fontSize"]).toBe("15");
    expect(document.documentElement.dataset.density).toBe("comfortable");
    expect(document.documentElement.style.getPropertyValue("--chat-width")).toBe("760px");
  });

  it("shows an error if saving fails, but keeps the change on screen", async () => {
    window.__MCP_CHAT_MOCK__ = {
      invoke: async () => {
        throw new Error("disk full");
      },
      subscribe: () => () => undefined,
    };
    await useApp.getState().setAppearance({ density: "compact" });
    expect(document.documentElement.dataset.density).toBe("compact");
    expect(useApp.getState().toasts.at(-1)?.kind).toBe("error");
  });
});

describe("ShortcutList", () => {
  it("lists the shortcuts of the command registry", () => {
    render(<ShortcutList />);
    expect(screen.getByText("Compact conversation")).toBeInTheDocument();
    expect(screen.getByText("Stop generating")).toBeInTheDocument();
    expect(screen.getByText("Allow this tool call once")).toBeInTheDocument();
    expect(screen.getAllByText(/Ctrl\+|⌘/).length).toBeGreaterThan(5);
  });
});
