import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { Composer } from "./Composer";
import { useApp } from "@/app/store";
import type { Attachment, Conversation, ProviderView } from "@/lib/types";

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

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  useApp.setState({ pendingAttachments: [], draft: "" });
});

const attachment: Attachment = {
  id: "a1",
  name: "notes.txt",
  mime: "text/plain",
  size: 12,
  kind: "text",
  path: "/tmp/notes.txt",
  text: "hello",
};

describe("Composer", () => {
  it("offers Continue after the step limit", () => {
    const continueTurn = vi.fn().mockResolvedValue(undefined);
    useApp.setState({
      providers: [provider],
      status: "idle",
      step: { current: 26, max: 25 },
      limitReached: true,
      continueTurn,
      draft: "",
    });
    render(<Composer conversation={conversation} />);
    expect(screen.getByText(/Tool step limit reached/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(continueTurn).toHaveBeenCalledOnce();
  });

  it("can send a message that only has attachments", () => {
    const sendMessage = vi.fn().mockResolvedValue(undefined);
    useApp.setState({
      providers: [provider],
      status: "idle",
      step: null,
      limitReached: false,
      continueTurn: vi.fn(),
      draft: "",
      pendingAttachments: [attachment],
      sendMessage,
    });
    render(<Composer conversation={conversation} />);
    const sendButton = screen.getByRole("button", { name: "Send message" });
    expect(sendButton).toBeEnabled();
    fireEvent.click(sendButton);
    expect(sendMessage).toHaveBeenCalledWith("");
  });
});
