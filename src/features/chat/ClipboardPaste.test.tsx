import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: () => Promise.resolve(() => undefined),
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
  reasoningEffort: "default",
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

const stageClipboardImage = vi.fn().mockResolvedValue(undefined);
const stageImageFile = vi.fn().mockResolvedValue(undefined);

beforeEach(() => {
  delete window.__MCP_CHAT_MOCK__;
  (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  stageClipboardImage.mockClear();
  stageImageFile.mockClear();
  useApp.setState({
    providers: [provider],
    status: "idle",
    step: null,
    limitReached: false,
    draft: "",
    pendingAttachments: [],
    stageClipboardImage,
    stageImageFile,
  });
});

afterEach(() => {
  cleanup();
  delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  useApp.setState({ pendingAttachments: [], draft: "" });
  vi.restoreAllMocks();
});

describe("clipboard image paste", () => {
  it("uses the native clipboard reader when WebKit exposes no image blob", () => {
    render(<Composer conversation={conversation} />);
    fireEvent.paste(screen.getByLabelText("Message"), {
      clipboardData: {
        items: [],
        getData: () => "",
      },
    });
    expect(stageClipboardImage).toHaveBeenCalledTimes(1);
  });

  it("leaves text paste to the textarea", () => {
    render(<Composer conversation={conversation} />);
    fireEvent.paste(screen.getByLabelText("Message"), {
      clipboardData: {
        items: [],
        getData: (type: string) => (type === "text/plain" ? "hello" : ""),
      },
    });
    expect(stageClipboardImage).not.toHaveBeenCalled();
  });
});
