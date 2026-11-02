import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { NotificationSettings } from "./NotificationSettings";
import { useApp } from "@/app/store";
import { resetPrimedForTests, setBackendForTests, type Permission } from "@/lib/notify";

const saved: Array<{ key: string; value: string }> = [];
const sent: Array<{ title: string; body: string }> = [];

function backend(permission: Permission, answer: Permission) {
  setBackendForTests({
    permission: async () => permission,
    request: async () => answer,
    send: (notice) => {
      sent.push(notice);
    },
  });
}

beforeEach(() => {
  saved.length = 0;
  sent.length = 0;
  resetPrimedForTests();
  window.__MCP_CHAT_MOCK__ = {
    invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
      if (command === "set_setting") saved.push({ key: String(args?.key), value: String(args?.value) });
      return null as T;
    },
    subscribe: () => () => undefined,
  };
  useApp.setState({ settings: {}, toasts: [] });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  setBackendForTests(null);
  delete window.__MCP_CHAT_MOCK__;
});

describe("NotificationSettings", () => {
  it("is on by default and says what it shows", () => {
    render(<NotificationSettings />);
    expect(screen.getByRole("checkbox", { name: /Notify me when I am away/ })).toBeChecked();
    expect(screen.getByText(/never your messages or commands/)).toBeInTheDocument();
  });

  it("turns off at once and saves the choice", async () => {
    render(<NotificationSettings />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Notify me when I am away/ }));
    expect(screen.getByRole("checkbox", { name: /Notify me when I am away/ })).not.toBeChecked();
    await waitFor(() => expect(saved).toEqual([{ key: "notifications.enabled", value: "false" }]));
    expect(useApp.getState().settings["notifications.enabled"]).toBe("false");
  });

  it("turns back on", async () => {
    useApp.setState({ settings: { "notifications.enabled": "false" } });
    render(<NotificationSettings />);
    fireEvent.click(screen.getByRole("checkbox", { name: /Notify me when I am away/ }));
    await waitFor(() => expect(saved.at(-1)?.value).toBe("true"));
  });

  it("sends a test notification when allowed", async () => {
    backend("granted", "granted");
    render(<NotificationSettings />);
    fireEvent.click(screen.getByRole("button", { name: "Send a test notification" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(useApp.getState().toasts.at(-1)?.text).toBe("Test notification sent.");
  });

  it("explains how to fix it when notifications are blocked", async () => {
    backend("default", "denied");
    render(<NotificationSettings />);
    fireEvent.click(screen.getByRole("button", { name: "Send a test notification" }));
    await waitFor(() => expect(useApp.getState().toasts.at(-1)?.kind).toBe("error"));
    expect(useApp.getState().toasts.at(-1)?.text).toMatch(/system settings/);
    expect(sent).toHaveLength(0);
  });
});
