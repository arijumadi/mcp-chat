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
  reasoningEffort: "auto",
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
  useApp.setState({ pendingAttachments: [], draft: "", status: "idle" });
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

  it("stages an image pasted into the composer", () => {
    const stageImageFile = vi.fn().mockResolvedValue(undefined);
    useApp.setState({
      providers: [provider],
      status: "idle",
      step: null,
      limitReached: false,
      continueTurn: vi.fn(),
      draft: "",
      pendingAttachments: [],
      stageImageFile,
    });
    render(<Composer conversation={conversation} />);
    const file = new File([new Uint8Array([1, 2, 3])], "clipboard.png", {
      type: "image/png",
    });
    fireEvent.paste(screen.getByRole("textbox", { name: "Message" }), {
      clipboardData: {
        items: [
          {
            kind: "file",
            type: "image/png",
            getAsFile: () => file,
          },
        ],
      },
    });
    expect(stageImageFile).toHaveBeenCalledWith(file);
  });

  it("steers an active turn instead of blocking the send", () => {
    const steerMessage = vi.fn().mockResolvedValue(undefined);
    useApp.setState({
      providers: [provider],
      status: "streaming",
      step: { current: 3, max: 0 },
      limitReached: false,
      continueTurn: vi.fn(),
      draft: "change direction",
      pendingAttachments: [],
      steerMessage,
    });
    render(<Composer conversation={conversation} />);
    const steerButton = screen.getByRole("button", { name: "Steer message" });
    expect(steerButton).toBeEnabled();
    fireEvent.click(steerButton);
    expect(steerMessage).toHaveBeenCalledWith("change direction");
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

  it("treats /compact as a command: it opens the confirmation and sends nothing", () => {
    const sendMessage = vi.fn().mockResolvedValue(undefined);
    const steerMessage = vi.fn().mockResolvedValue(undefined);
    const openCompact = vi.fn();
    useApp.setState({
      providers: [provider],
      status: "idle",
      step: null,
      limitReached: false,
      continueTurn: vi.fn(),
      draft: "/compact keep the schema decisions",
      pendingAttachments: [],
      sendMessage,
      steerMessage,
      openCompact,
    });
    render(<Composer conversation={conversation} />);
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    expect(openCompact).toHaveBeenCalledWith("keep the schema decisions");
    expect(sendMessage).not.toHaveBeenCalled();
    expect(steerMessage).not.toHaveBeenCalled();
    // Cancel must lose nothing, so the draft is still there.
    expect(useApp.getState().draft).toBe("/compact keep the schema decisions");
  });

  it("does not steer a running turn with /compact", () => {
    const steerMessage = vi.fn().mockResolvedValue(undefined);
    const openCompact = vi.fn();
    useApp.setState({
      providers: [provider],
      status: "streaming",
      step: { current: 1, max: 0 },
      limitReached: false,
      continueTurn: vi.fn(),
      draft: "/compact",
      pendingAttachments: [],
      steerMessage,
      openCompact,
    });
    render(<Composer conversation={conversation} />);
    fireEvent.click(screen.getByRole("button", { name: "Steer message" }));
    expect(steerMessage).not.toHaveBeenCalled();
    // The store explains why it cannot compact right now.
    expect(openCompact).toHaveBeenCalledWith("");
  });

  it("sends ordinary text that merely resembles the command", () => {
    const sendMessage = vi.fn().mockResolvedValue(undefined);
    const openCompact = vi.fn();
    useApp.setState({
      providers: [provider],
      status: "idle",
      step: null,
      limitReached: false,
      continueTurn: vi.fn(),
      draft: "/compactor is a path",
      pendingAttachments: [],
      sendMessage,
      openCompact,
    });
    render(<Composer conversation={conversation} />);
    fireEvent.click(screen.getByRole("button", { name: "Send message" }));
    expect(sendMessage).toHaveBeenCalledWith("/compactor is a path");
    expect(openCompact).not.toHaveBeenCalled();
  });
});
