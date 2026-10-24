import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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

function message(role: "user" | "assistant", parts: Part[]): MessageWithParts {
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

beforeEach(() => {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  writeText.mockClear();
});

afterEach(() => {
  cleanup();
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
    expect(revertMessage).toHaveBeenCalledWith("m1");
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
});
