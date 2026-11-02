import { beforeEach, describe, expect, it, vi } from "vitest";
import { useApp } from "@/app/store";
import { useUi } from "@/app/ui";
import type { ApprovalEvent, ProviderView } from "@/lib/types";
import readme from "../../../README.md?raw";
import { COMMANDS, commandById, filterCommands, matchesQuery } from "./registry";
import { handleShortcut, resolveShortcut, type OverlayState } from "./useShortcuts";
import { formatShortcut, parseChord } from "./shortcuts";

const noOverlays: OverlayState = { quickOpen: false, paletteOpen: false, compactOpen: false };

const approval: ApprovalEvent = {
  type: "approval",
  conversationId: "c1",
  callId: "call1",
  server: "mcp-chat",
  tool: "bash",
  args: { command: "npm test" },
  allowAlways: true,
};

function press(init: KeyboardEventInit & { key: string }, target?: HTMLElement): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  if (target) Object.defineProperty(event, "target", { value: target });
  return event;
}

const textarea = (label?: string) => {
  const element = document.createElement("textarea");
  if (label) element.setAttribute("aria-label", label);
  return element;
};

beforeEach(() => {
  useApp.setState({
    activeId: "c1",
    view: "chat",
    status: "idle",
    approvals: [],
    providers: [{ id: "p1" } as ProviderView],
    toasts: [],
    compactDialog: null,
  });
  useUi.setState({ quickOpen: false, palette: null });
});

describe("the command list", () => {
  it("has unique ids and unique, well-formed shortcuts", () => {
    const ids = COMMANDS.map((command) => command.id);
    expect(new Set(ids).size).toBe(ids.length);
    const chords = COMMANDS.filter((c) => c.shortcut).map((c) => c.shortcut!);
    expect(new Set(chords).size).toBe(chords.length);
    for (const chord of chords) expect(() => parseChord(chord)).not.toThrow();
  });

  it("covers every action the keyboard must reach", () => {
    for (const id of [
      "chat.new", "chat.search", "turn.stop", "chat.compact", "model.switch",
      "workspace.switch", "approval.once", "approval.always", "approval.reject",
    ]) {
      expect(commandById(id), id).toBeDefined();
      expect(commandById(id)?.shortcut, `${id} has a shortcut`).toBeTruthy();
    }
  });

  it("shows readable shortcut text", () => {
    expect(formatShortcut(commandById("approval.once")!.shortcut!, false)).toBe("Ctrl+Enter");
  });
});

describe("documentation", () => {
  it("lists every shortcut of the registry in the README", () => {
    for (const command of COMMANDS.filter((item) => item.shortcut)) {
      const written = command.shortcut!.replaceAll("Mod", "Ctrl/Cmd");
      expect(readme, `${command.id} (${written})`).toContain(`\`${written}\``);
    }
  });
});

describe("palette search", () => {
  it("needs every word to match the title or keywords", () => {
    const stop = commandById("turn.stop")!;
    expect(matchesQuery(stop, "stop")).toBe(true);
    expect(matchesQuery(stop, "cancel")).toBe(true);
    expect(matchesQuery(stop, "stop gen")).toBe(true);
    expect(matchesQuery(stop, "stop banana")).toBe(false);
    expect(matchesQuery(stop, "   ")).toBe(true);
  });

  it("ranks title matches before keyword-only matches", () => {
    const titles = filterCommands("model").map((command) => command.id);
    expect(titles[0]).toBe("model.switch");
    expect(filterCommands("zzzz")).toEqual([]);
    expect(filterCommands("").length).toBe(COMMANDS.length);
  });
});

describe("when a command can run", () => {
  const state = () => useApp.getState();
  const reason = (id: string) => commandById(id)!.disabled(state());

  it("allows chat commands in an idle chat", () => {
    for (const id of ["chat.compact", "model.switch", "workspace.switch", "chat.new"]) {
      expect(reason(id), id).toBeNull();
    }
  });

  it("explains why a busy chat cannot compact or switch", () => {
    useApp.setState({ status: "streaming" });
    for (const id of ["chat.compact", "model.switch", "workspace.switch"]) {
      expect(reason(id), id).toBe("Stop the current turn first.");
    }
    expect(reason("turn.stop")).toBeNull();
  });

  it("only stops when something is running", () => {
    expect(reason("turn.stop")).toBe("Nothing is running.");
    useApp.setState({ status: "running_tool" });
    expect(reason("turn.stop")).toBeNull();
  });

  it("needs a provider before switching models", () => {
    useApp.setState({ providers: [] });
    expect(reason("model.switch")).toMatch(/provider/i);
  });

  it("is limited to the chat view", () => {
    useApp.setState({ view: "settings" });
    expect(reason("chat.compact")).toBe("Open a conversation first.");
    expect(reason("turn.stop")).toBe("Open a conversation first.");
    useApp.setState({ view: "chat", activeId: null });
    expect(reason("workspace.switch")).toBe("Open a conversation first.");
  });

  it("answers approval commands only for a call waiting in the open chat", () => {
    expect(reason("approval.once")).toMatch(/No tool/);
    useApp.setState({ approvals: [{ ...approval, conversationId: "other" }] });
    expect(reason("approval.once")).toMatch(/No tool/);
    useApp.setState({ approvals: [approval] });
    expect(reason("approval.once")).toBeNull();
    expect(reason("approval.reject")).toBeNull();
    expect(reason("approval.always")).toBeNull();
    useApp.setState({ approvals: [{ ...approval, allowAlways: false }] });
    expect(reason("approval.always")).toMatch(/permanently/);
  });
});

describe("keyboard dispatch", () => {
  const ctrl = { ctrlKey: true };

  it("never approves or rejects from a text field", () => {
    useApp.setState({ approvals: [approval] });
    const area = textarea("Message");
    for (const init of [
      { key: "Enter", ...ctrl },
      { key: "Enter", ...ctrl, shiftKey: true },
      { key: "Backspace", ...ctrl },
    ]) {
      expect(resolveShortcut(press(init, area), noOverlays)).toBeNull();
    }
    // Outside a text field the same chords work.
    const button = document.createElement("button");
    expect(resolveShortcut(press({ key: "Enter", ...ctrl }, button), noOverlays)?.id).toBe("approval.once");
    expect(resolveShortcut(press({ key: "Enter", ...ctrl, shiftKey: true }, button), noOverlays)?.id).toBe("approval.always");
    expect(resolveShortcut(press({ key: "Backspace", ...ctrl }, button), noOverlays)?.id).toBe("approval.reject");
  });

  it("answers the approval and consumes the key", () => {
    const respondApproval = vi.fn().mockResolvedValue(undefined);
    useApp.setState({ approvals: [approval], respondApproval });
    const event = press({ key: "Enter", ...ctrl }, document.createElement("button"));
    expect(handleShortcut(event, useApp.getState(), noOverlays)).toBe(true);
    expect(respondApproval).toHaveBeenCalledWith("call1", "once");
    expect(event.defaultPrevented).toBe(true);
  });

  it("leaves the key alone when there is nothing to approve", () => {
    const event = press({ key: "Backspace", ...ctrl }, document.createElement("button"));
    expect(handleShortcut(event, useApp.getState(), noOverlays)).toBe(false);
    // Ctrl+Backspace must still delete a word in the composer.
    expect(event.defaultPrevented).toBe(false);
    expect(useApp.getState().toasts).toHaveLength(0);
  });

  it("stops a turn with Escape from the message box only", () => {
    const stop = vi.fn().mockResolvedValue(undefined);
    useApp.setState({ status: "streaming", stop });
    expect(resolveShortcut(press({ key: "Escape" }, textarea("Message")), noOverlays)?.id).toBe("turn.stop");
    // Escape in a title field cancels the edit; it must not stop the turn.
    const title = document.createElement("input");
    expect(resolveShortcut(press({ key: "Escape" }, title), noOverlays)).toBeNull();
    expect(resolveShortcut(press({ key: "Escape" }, textarea("Something else")), noOverlays)).toBeNull();
    expect(resolveShortcut(press({ key: "Escape" }, document.body), noOverlays)?.id).toBe("turn.stop");
  });

  it("does nothing on Escape when idle, and does not complain", () => {
    const event = press({ key: "Escape" }, textarea("Message"));
    expect(handleShortcut(event, useApp.getState(), noOverlays)).toBe(false);
    expect(event.defaultPrevented).toBe(false);
    expect(useApp.getState().toasts).toHaveLength(0);
  });

  it("explains a refused command instead of failing silently", () => {
    useApp.setState({ status: "streaming" });
    const event = press({ key: "K", ctrlKey: true, shiftKey: true }, document.body);
    expect(handleShortcut(event, useApp.getState(), noOverlays)).toBe(false);
    expect(useApp.getState().toasts.at(-1)?.text).toBe("Stop the current turn first.");
    expect(event.defaultPrevented).toBe(true);
  });

  it("stays out of the way while an overlay is open", () => {
    useApp.setState({ approvals: [approval], status: "streaming" });
    for (const overlays of [
      { ...noOverlays, quickOpen: true },
      { ...noOverlays, paletteOpen: true },
      { ...noOverlays, compactOpen: true },
    ]) {
      expect(resolveShortcut(press({ key: "Escape" }, document.body), overlays)).toBeNull();
      expect(resolveShortcut(press({ key: "Enter", ...ctrl }, document.body), overlays)).toBeNull();
    }
  });

  it("ignores key repeat and IME composition", () => {
    useApp.setState({ approvals: [approval] });
    const button = document.createElement("button");
    expect(resolveShortcut(press({ key: "Enter", ...ctrl, repeat: true }, button), noOverlays)).toBeNull();
    expect(resolveShortcut(press({ key: "Enter", ...ctrl, isComposing: true }, button), noOverlays)).toBeNull();
  });

  it("opens the overlays and starts a new chat from any field", () => {
    const area = textarea("Message");
    const newChat = vi.fn().mockResolvedValue(undefined);
    useApp.setState({ newChat });
    handleShortcut(press({ key: "n", ...ctrl }, area), useApp.getState(), noOverlays);
    expect(newChat).toHaveBeenCalled();
    handleShortcut(press({ key: "k", ...ctrl }, area), useApp.getState(), noOverlays);
    expect(useUi.getState().quickOpen).toBe(true);
    handleShortcut(press({ key: "P", ...ctrl, shiftKey: true }, area), useApp.getState(), {
      ...noOverlays,
      quickOpen: false,
    });
    expect(useUi.getState().palette).toEqual({ page: "root", query: "" });
    expect(useUi.getState().quickOpen).toBe(false);
  });
});
