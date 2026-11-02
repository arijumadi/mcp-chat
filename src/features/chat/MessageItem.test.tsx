import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { installFakeScroll, removeFakeScroll } from "@/test/fakeScroll";
import { MessageItem } from "./MessageItem";
import { useApp } from "@/app/store";
import { formatMessageDateTime } from "@/lib/format";
import type { MessageWithParts, Part } from "@/lib/types";

function part(id: string, kind: string, payload: unknown): Part {
  return {
    id,
    messageId: "m1",
    idx: 0,
    kind,
    payloadJson: JSON.stringify(payload),
  };
}

function message(role: "user" | "assistant" | "system", parts: Part[]): MessageWithParts {
  return {
    id: "m1",
    conversationId: "c1",
    role,
    createdAt: new Date(2026, 9, 24, 13, 39).getTime(),
    status: "completed",
    error: null,
    parts,
  };
}

const writeText = vi.fn().mockResolvedValue(undefined);
const write = vi.fn().mockResolvedValue(undefined);

beforeEach(() => {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText, write },
  });
  writeText.mockClear();
  write.mockClear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("MessageItem", () => {
  it("hides the stored timestamp from the bubble and exposes it on hover", async () => {
    const revertMessage = vi.fn().mockResolvedValue(undefined);
    useApp.setState({ status: "idle", revertMessage });
    const item = message("user", [
      part(
        "time",
        "time",
        { text: "[Message sent at: Saturday, 24 October 2026, 13:39 (UTC+07:00)]" },
      ),
      part("text", "text", { text: "Hello there" }),
    ]);
    render(<MessageItem message={item} canRetry={false} />);

    expect(screen.getByText("Hello there")).toBeInTheDocument();
    expect(screen.queryByText(/Message sent at/)).not.toBeInTheDocument();
    expect(
      screen.getByText(formatMessageDateTime(item.createdAt)),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Copy message" }));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith("Hello there"),
    );

    fireEvent.click(screen.getByRole("button", { name: "Revert message" }));
    expect(screen.getByText("Revert to this message?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Revert" }));
    expect(revertMessage).toHaveBeenCalledWith("m1");
  });

  it("renders a file attachment on a user message", () => {
    useApp.setState({ status: "idle" });
    const item = message("user", [
      part("attachment", "attachment", {
        id: "a1",
        name: "notes.txt",
        mime: "text/plain",
        size: 12,
        fileKind: "text",
        path: "/tmp/notes.txt",
        text: "hello",
      }),
    ]);
    render(<MessageItem message={item} canRetry={false} />);
    expect(screen.getByText("notes.txt")).toBeInTheDocument();
    expect(screen.getByTestId("user-message")).toBeInTheDocument();
  });

  it("renders a compaction summary in a system message", () => {
    useApp.setState({ status: "idle" });
    const item = message("system", [
      part("summary", "summary", { text: "Earlier work", coversUntil: "m4" }),
    ]);
    render(<MessageItem message={item} canRetry={false} />);
    expect(screen.getByText("Earlier conversation summary")).toBeInTheDocument();
    expect(screen.getByText("Earlier work")).toBeInTheDocument();
  });

  it("copies only text from an assistant response", async () => {
    useApp.setState({ status: "idle" });
    const item = message("assistant", [
      part("reasoning", "reasoning", { text: "private reasoning" }),
      part("text", "text", { text: "Final answer" }),
    ]);
    render(<MessageItem message={item} canRetry={false} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy message" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("Final answer"));
    expect(
      screen.queryByRole("button", { name: "Revert message" }),
    ).not.toBeInTheDocument();
  });

  it("copies assistant markdown as rendered rich text", async () => {
    class TestClipboardItem {
      items: Record<string, Blob>;
      constructor(items: Record<string, Blob>) {
        this.items = items;
      }
    }
    vi.stubGlobal("ClipboardItem", TestClipboardItem);
    useApp.setState({ status: "idle" });
    const item = message("assistant", [
      part("text", "text", {
        text: "**Bold** answer\n\n- one\n- two\n\n```js\nconst x = 1\n```",
      }),
    ]);
    render(<MessageItem message={item} canRetry={false} />);

    fireEvent.click(screen.getByRole("button", { name: "Copy message" }));
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1));

    const items = write.mock.calls[0]?.[0] as TestClipboardItem[];
    const clipboardItem = items[0];
    expect(clipboardItem).toBeDefined();
    const html = await clipboardItem?.items["text/html"]?.text();
    const plain = await clipboardItem?.items["text/plain"]?.text();
    expect(html).toContain("<strong>Bold</strong>");
    expect(html).toContain("<li>one</li>");
    expect(html).toContain("<pre");
    expect(html).not.toContain(">Copy<");
    expect(plain).toContain("Bold answer");
    expect(plain).not.toContain("**Bold**");
  });

  it("keeps reasoning collapsed to a Thought row until it is opened", () => {
    const item = message("assistant", [
      part("r1", "reasoning", { text: "Private chain of thought", durationMs: 12_000 }),
      part("t1", "text", { text: "The answer." }),
    ]);
    render(<MessageItem message={item} canRetry={false} />);
    const row = screen.getByRole("button", { name: /Thought/ });
    expect(row).toHaveTextContent("Thought");
    expect(row).toHaveTextContent("12s");
    expect(row).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Private chain of thought")).not.toBeInTheDocument();
    expect(screen.getByText("The answer.")).toBeInTheDocument();

    fireEvent.click(row);
    expect(screen.getByText("Private chain of thought")).toBeInTheDocument();
  });

  it("shows only the label when a thought has no timing", () => {
    const item = message("assistant", [part("r1", "reasoning", { text: "Hmm" })]);
    render(<MessageItem message={item} canRetry={false} />);
    expect(screen.getByRole("button", { name: /Thought/ }).textContent).toBe("Thought");
  });

  it("passes the streaming state to its thought so it follows the newest text", () => {
    installFakeScroll();
    try {
      const text = "x".repeat(400);
      const streaming = { ...message("assistant", [part("r1", "reasoning", { text })]), status: "streaming" };
      const { unmount } = render(<MessageItem message={streaming} canRetry={false} />);
      fireEvent.click(screen.getByRole("button", { name: /^Thought/ }));
      expect(screen.getByRole("region", { name: "Thought text" }).scrollTop).toBe(300);
      unmount();

      const done = message("assistant", [part("r1", "reasoning", { text })]);
      render(<MessageItem message={done} canRetry={false} />);
      fireEvent.click(screen.getByRole("button", { name: /^Thought/ }));
      expect(screen.getByRole("region", { name: "Thought text" }).scrollTop).toBe(0);
    } finally {
      removeFakeScroll();
    }
  });
});
