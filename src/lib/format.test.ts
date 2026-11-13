import { describe, expect, it } from "vitest";
import { formatDuration, groupConversations, shortModelName } from "./format";
import type { Conversation } from "./types";

function conversation(
  id: string,
  updatedAt: number,
  pinned = false,
): Conversation {
  return {
    id,
    title: id,
    createdAt: updatedAt,
    updatedAt,
    providerId: null,
    model: null,
    reasoningEffort: "auto",
    workspaceRoot: null,
    systemPrompt: null,
    archived: false,
    pinned,
    maxSteps: 25,
    toolMode: "direct",
    serversConfigured: false,
  };
}

describe("format helpers", () => {
  it("shows only the model name after the last slash", () => {
    expect(shortModelName("deepseek/deepseek-v4.1-flash")).toBe(
      "deepseek-v4.1-flash",
    );
    expect(shortModelName("mock-text")).toBe("mock-text");
  });

  it("formats durations", () => {
    expect(formatDuration(340)).toBe("340ms");
    expect(formatDuration(1500)).toBe("1.5s");
    expect(formatDuration(65_000)).toBe("1m 5s");
  });

  it("groups conversations by day", () => {
    const now = new Date();
    now.setHours(12, 0, 0, 0);
    const today = now.getTime();
    const groups = groupConversations([
      conversation("today", today),
      conversation("old", today - 20 * 86_400_000),
    ]);
    expect(groups[0]?.label).toBe("Today");
    expect(groups.at(-1)?.label).toBe("Older");
  });

  it("puts pinned conversations in their own leading group", () => {
    const now = new Date();
    now.setHours(12, 0, 0, 0);
    const groups = groupConversations([
      conversation("today", now.getTime()),
      conversation("pinned", now.getTime() - 20 * 86_400_000, true),
    ]);
    expect(groups[0]?.label).toBe("Pinned");
    expect(groups[0]?.items[0]?.id).toBe("pinned");
  });
});
