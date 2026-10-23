import type { Conversation } from "./types";

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

export function formatTime(timestamp: number): string {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}

export function startOfDay(date: Date): number {
  const copy = new Date(date);
  copy.setHours(0, 0, 0, 0);
  return copy.getTime();
}

export type ConversationGroup = { label: string; items: Conversation[] };

export function groupConversations(conversations: Conversation[]): ConversationGroup[] {
  const today = startOfDay(new Date());
  const yesterday = today - 86_400_000;
  const week = today - 7 * 86_400_000;
  const groups: ConversationGroup[] = [
    { label: "Today", items: [] },
    { label: "Yesterday", items: [] },
    { label: "Previous 7 days", items: [] },
    { label: "Older", items: [] },
  ];
  for (const conversation of conversations) {
    if (conversation.updatedAt >= today) groups[0]?.items.push(conversation);
    else if (conversation.updatedAt >= yesterday) groups[1]?.items.push(conversation);
    else if (conversation.updatedAt >= week) groups[2]?.items.push(conversation);
    else groups[3]?.items.push(conversation);
  }
  return groups.filter((group) => group.items.length > 0);
}

export function formatJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export function shorten(text: string, max: number): string {
  if (text.length <= max) return text;
  return `${text.slice(0, max - 1)}…`;
}

export function parseJson(value: string | null | undefined): unknown {
  if (!value) return null;
  try {
    return JSON.parse(value) as unknown;
  } catch {
    return null;
  }
}
