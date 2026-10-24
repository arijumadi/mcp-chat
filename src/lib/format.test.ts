import { describe, expect, it } from "vitest";
import { formatDuration, groupConversations } from "./format";
import type { Conversation } from "./types";

function conversation(id: string, updatedAt: number): Conversation {
  return {
    id,
    title: id,
    createdAt: updatedAt,
    updatedAt,
    providerId: null,
    model: null,
    systemPrompt: null,
    archived: false,
    maxSteps: 25,
    toolMode: "direct",
    serversConfigured: false,
  };
}

describe("format helpers", () => {
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
});
