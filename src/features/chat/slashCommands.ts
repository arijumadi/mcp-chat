/** A composer slash command, parsed from the draft text. */
export interface CompactCommand {
  /** Optional text telling the summarizer what to keep in detail. */
  focus: string;
}

/**
 * `/compact` or `/compact <focus>`. The whole draft must be the command, so
 * ordinary text that merely starts with "/compact" (a path, say) is not one.
 */
export function parseCompactCommand(text: string): CompactCommand | null {
  const match = /^\/compact(?:\s+([\s\S]*))?$/i.exec(text.trim());
  return match ? { focus: (match[1] ?? "").trim() } : null;
}
