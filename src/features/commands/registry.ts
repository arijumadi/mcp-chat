import type { AppStore } from "@/app/store";
import { BUSY_STATUSES } from "@/app/store";
import type { PalettePage } from "@/app/ui";
import { useUi } from "@/app/ui";

/**
 * Every action the user can start from the keyboard.
 *
 * The palette, the global shortcut handler and the shortcut list in settings
 * are all built from this one list, so a shortcut can never exist without a
 * palette entry or the other way round.
 */
export interface Command {
  id: string;
  title: string;
  /** Extra words the palette also matches. */
  keywords: string;
  /** Shortcut in the `Mod+Shift+P` notation, if it has one. */
  shortcut?: string;
  /**
   * Whether the shortcut also fires while typing in a text field: `true` for
   * any field, a CSS selector for only matching fields, `false` (the default)
   * for none.
   */
  worksWhileTyping?: boolean | string;
  /**
   * Stay quiet when the command cannot run. Right for keys pressed out of
   * habit (Escape, the approval chords); other commands explain themselves.
   */
  silentWhenDisabled?: boolean;
  /** Return a reason to disable the command, or null when it can run. */
  disabled: (state: AppStore) => string | null;
  run: (state: AppStore) => void | Promise<void>;
}

/** Selector of the message box. */
export const COMPOSER = 'textarea[aria-label="Message"]';

export const BUSY_REASON = "Stop the current turn first.";

function needsChat(state: AppStore): string | null {
  if (state.view !== "chat" || !state.activeId) return "Open a conversation first.";
  return null;
}

function needsIdleChat(state: AppStore): string | null {
  return needsChat(state) ?? (BUSY_STATUSES.includes(state.status) ? BUSY_REASON : null);
}

/** The approval the user is being asked about, in the open conversation. */
export function pendingApproval(state: AppStore) {
  return state.approvals.find((item) => item.conversationId === state.activeId);
}

function needsApproval(state: AppStore): string | null {
  return pendingApproval(state) ? null : "No tool is waiting for approval.";
}

function openPage(page: PalettePage): (state: AppStore) => void {
  return () => useUi.getState().openPalette(page);
}

// Approval commands use chords that are also ordinary typing (Ctrl+Backspace
// deletes a word, Ctrl+Enter submits). They must never fire from a text field,
// or someone writing a rejection reason could approve the call by accident.
export const COMMANDS: readonly Command[] = [
  {
    id: "chat.new",
    title: "New chat",
    keywords: "create conversation start",
    shortcut: "Mod+N",
    worksWhileTyping: true,
    disabled: () => null,
    run: (state) => void state.newChat(),
  },
  {
    id: "chat.search",
    title: "Search chats",
    keywords: "find conversation switch open history",
    shortcut: "Mod+K",
    worksWhileTyping: true,
    disabled: () => null,
    run: () => useUi.getState().openQuick(),
  },
  {
    id: "palette.open",
    title: "Show all commands",
    keywords: "command palette",
    shortcut: "Mod+Shift+P",
    worksWhileTyping: true,
    disabled: () => null,
    run: () => useUi.getState().openPalette(),
  },
  {
    id: "turn.stop",
    title: "Stop generating",
    keywords: "cancel interrupt abort",
    shortcut: "Esc",
    // Escape stops the turn from the message box, as before, but not from
    // other fields: Escape also cancels a title edit and must not stop a turn.
    worksWhileTyping: COMPOSER,
    silentWhenDisabled: true,
    disabled: (state) =>
      needsChat(state) ??
      (BUSY_STATUSES.includes(state.status) ? null : "Nothing is running."),
    run: (state) => void state.stop(),
  },
  {
    id: "chat.compact",
    title: "Compact conversation",
    keywords: "summarize shrink context",
    shortcut: "Mod+Shift+K",
    worksWhileTyping: true,
    disabled: needsIdleChat,
    run: (state) => state.openCompact(),
  },
  {
    id: "model.switch",
    title: "Switch model…",
    keywords: "provider llm change",
    shortcut: "Mod+Shift+M",
    worksWhileTyping: true,
    disabled: (state) =>
      needsIdleChat(state) ??
      (state.providers.length === 0 ? "Add a provider in Settings first." : null),
    run: openPage("models"),
  },
  {
    id: "workspace.switch",
    title: "Switch workspace…",
    keywords: "folder directory project",
    shortcut: "Mod+Shift+O",
    worksWhileTyping: true,
    disabled: needsIdleChat,
    run: openPage("workspaces"),
  },
  {
    id: "approval.once",
    title: "Allow this tool call once",
    keywords: "approve yes permit",
    shortcut: "Mod+Enter",
    worksWhileTyping: false,
    silentWhenDisabled: true,
    disabled: needsApproval,
    run: (state) => {
      const approval = pendingApproval(state);
      if (approval) void state.respondApproval(approval.callId, "once");
    },
  },
  {
    id: "approval.always",
    title: "Always allow this tool",
    keywords: "approve remember rule",
    shortcut: "Mod+Shift+Enter",
    worksWhileTyping: false,
    silentWhenDisabled: true,
    disabled: (state) =>
      needsApproval(state) ??
      (pendingApproval(state)?.allowAlways ? null : "This call cannot be allowed permanently."),
    run: (state) => {
      const approval = pendingApproval(state);
      if (approval?.allowAlways) void state.respondApproval(approval.callId, "always");
    },
  },
  {
    id: "approval.reject",
    title: "Reject this tool call",
    keywords: "deny decline no",
    shortcut: "Mod+Backspace",
    worksWhileTyping: false,
    silentWhenDisabled: true,
    disabled: needsApproval,
    run: (state) => {
      const approval = pendingApproval(state);
      if (approval) void state.respondApproval(approval.callId, "reject");
    },
  },
  {
    id: "view.sidebar",
    title: "Toggle sidebar",
    keywords: "hide show conversations list",
    shortcut: "Mod+B",
    worksWhileTyping: true,
    disabled: () => null,
    run: (state) => state.toggleSidebar(),
  },
  {
    id: "view.settings",
    title: "Open settings",
    keywords: "preferences providers appearance",
    shortcut: "Mod+,",
    worksWhileTyping: true,
    disabled: () => null,
    run: (state) => state.setView("settings"),
  },
  {
    id: "view.servers",
    title: "Open MCP servers",
    keywords: "tools mcp",
    disabled: () => null,
    run: (state) => state.setView("servers"),
  },
  {
    id: "view.skills",
    title: "Open skills",
    keywords: "workflows reusable global manage",
    disabled: () => null,
    run: (state) => state.setView("skills"),
  },
];

export function commandById(id: string): Command | undefined {
  return COMMANDS.find((command) => command.id === id);
}

/** Match a palette query against title and keywords: every word must appear. */
export function matchesQuery(command: Command, query: string): boolean {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const haystack = `${command.title} ${command.keywords}`.toLowerCase();
  return words.every((word) => haystack.includes(word));
}

/** Rank title matches above keyword-only matches, keeping the list order otherwise. */
export function filterCommands(query: string): Command[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const matched = COMMANDS.filter((command) => matchesQuery(command, query));
  if (words.length === 0) return matched;
  const inTitle = (command: Command) =>
    words.every((word) => command.title.toLowerCase().includes(word));
  return [...matched.filter(inTitle), ...matched.filter((command) => !inTitle(command))];
}
