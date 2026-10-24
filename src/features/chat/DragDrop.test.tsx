import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";

const bridge = vi.hoisted(() => ({
  handler: null as null | ((event: { payload: unknown }) => void),
  unlisten: vi.fn(),
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: (
      handler: (event: { payload: unknown }) => void,
    ) => {
      bridge.handler = handler;
      return Promise.resolve(bridge.unlisten);
    },
  }),
}));

import { Composer } from "./Composer";
import { useApp } from "@/app/store";
import type { Conversation, ProviderView } from "@/lib/types";

const conversation: Conversation = {
  id: "c1",
  title: "Test",
  createdAt: 0,
  updatedAt: 0,
  providerId: "p1",
  model: "mock",
  workspaceRoot: "/home/you/project",
  systemPrompt: null,
  archived: false,
  pinned: false,
  maxSteps: 25,
  toolMode: "lazy",
  serversConfigured: false,
};

const provider: ProviderView = {
  id: "p1",
  name: "Mock",
  kind: "openai_compat",
  baseUrl: "http://localhost/v1",
  model: "mock",
  apiKeyRef: "p1",
  contextWindow: 128000,
  maxOutputTokens: 8192,
  createdAt: 0,
  updatedAt: 0,
  hasKey: true,
  keyHint: null,
  models: [],
};

const stageAttachments = vi.fn().mockResolvedValue(undefined);

beforeEach(() => {
  delete window.__MCP_CHAT_MOCK__;
  (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  bridge.handler = null;
  bridge.unlisten.mockClear();
  stageAttachments.mockClear();
  useApp.setState({
    providers: [provider],
    status: "idle",
    step: null,
    limitReached: false,
    draft: "",
    pendingAttachments: [],
    stageAttachments,
  });
});

afterEach(() => {
  cleanup();
  delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  useApp.setState({ pendingAttachments: [], draft: "" });
  vi.restoreAllMocks();
});

describe("attachment drag and drop", () => {
  it("shows a drop overlay and stages dropped paths", async () => {
    render(<Composer conversation={conversation} />);
    await waitFor(() => expect(bridge.handler).not.toBeNull());

    act(() => {
      bridge.handler?.({
        payload: {
          type: "enter",
          paths: ["/tmp/photo.png", "/tmp/notes.pdf"],
          position: { x: 10, y: 10 },
        },
      });
    });
    expect(screen.getByText("Drop 2 files to attach")).toBeInTheDocument();

    act(() => {
      bridge.handler?.({
        payload: {
          type: "drop",
          paths: ["/tmp/photo.png", "/tmp/notes.pdf"],
          position: { x: 10, y: 10 },
        },
      });
    });

    await waitFor(() =>
      expect(stageAttachments).toHaveBeenCalledWith([
        "/tmp/photo.png",
        "/tmp/notes.pdf",
      ]),
    );
    expect(screen.queryByText("Drop 2 files to attach")).not.toBeInTheDocument();
  });

  it("unlistens when the composer unmounts", async () => {
    const view = render(<Composer conversation={conversation} />);
    await waitFor(() => expect(bridge.handler).not.toBeNull());
    view.unmount();
    expect(bridge.unlisten).toHaveBeenCalled();
  });
});
