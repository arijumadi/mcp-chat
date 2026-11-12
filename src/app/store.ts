import { maybeNotify, notificationsEnabled, primeNotifications } from "@/lib/notify";
import {
  appearanceSettings,
  applyAppearance,
  readAppearance,
  type Appearance,
} from "@/lib/appearance";
import { create } from "zustand";
import { call, subscribe } from "@/lib/ipc";
import type {
  AppInfo,
  ApprovalEvent,
  Attachment,
  Conversation,
  CoreEvent,
  InputRequestEvent,
  MessagePage,
  MessageWithParts,
  Part,
  ProviderView,
  RevertResult,
  SearchHit,
  ServerView,
  TurnStatus,
  UpdateInfo,
  UsageEvent,
} from "@/lib/types";

export type View = "chat" | "settings" | "servers";

/** Statuses in which a turn (or a compaction) owns the conversation. */
export const BUSY_STATUSES: readonly TurnStatus[] = [
  "thinking",
  "streaming",
  "compacting",
  "waiting_approval",
  "running_tool",
];
export type Theme = "system" | "light" | "dark";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface Toast {
  id: string;
  text: string;
  kind: "error" | "info";
  actions?: ToastAction[];
}

export interface AppStore {
  booted: boolean;
  conversations: Conversation[];
  conversationLimit: number;
  conversationHasMore: boolean;
  activeId: string | null;
  messages: MessageWithParts[];
  messageHasMore: boolean;
  loadingOlder: boolean;
  pendingAttachments: Attachment[];
  draft: string;
  providers: ProviderView[];
  servers: ServerView[];
  settings: Record<string, string>;
  appInfo: AppInfo | null;
  updateInfo: UpdateInfo | null;
  updateChecking: boolean;
  updateError: string | null;
  view: View;
  sidebarOpen: boolean;
  status: TurnStatus;
  streamingMessageId: string | null;
  approvals: ApprovalEvent[];
  inputRequests: InputRequestEvent[];
  usage: UsageEvent | null;
  step: { current: number; max: number } | null;
  limitReached: boolean;
  autoApprove: boolean;
  /** Open when set. `focus` is the optional text for the summarizer. */
  compactDialog: { focus: string } | null;
  toasts: Toast[];
  searchHits: SearchHit[];
  searching: boolean;
  init: () => Promise<void>;
  refreshConversations: () => Promise<void>;
  loadMoreConversations: () => Promise<void>;
  refreshProviders: () => Promise<void>;
  refreshServers: () => Promise<void>;
  refreshSettings: () => Promise<void>;
  checkForUpdate: (manual?: boolean) => Promise<void>;
  openConversation: (id: string) => Promise<void>;
  newChat: (seed?: string) => Promise<void>;
  sendMessage: (text: string) => Promise<void>;
  steerMessage: (text: string) => Promise<void>;
  stageAttachments: (paths: string[]) => Promise<void>;
  stageImageFile: (file: File) => Promise<void>;
  stageClipboardImage: () => Promise<void>;
  removeAttachment: (id: string) => Promise<void>;
  setDraft: (text: string) => void;
  stop: () => Promise<void>;
  /** Ask to compact the active conversation. Opens a confirmation first. */
  openCompact: (focus?: string) => void;
  closeCompact: () => void;
  compactConversation: (focus?: string) => Promise<void>;
  regenerate: () => Promise<void>;
  continueTurn: () => Promise<void>;
  setAutoApprove: (enabled: boolean) => Promise<void>;
  revertMessage: (messageId: string) => Promise<void>;
  setModel: (providerId: string, model: string) => Promise<void>;
  setConversationWorkspace: (conversationId: string, workspaceRoot: string) => Promise<void>;
  renameConversation: (id: string, title: string) => Promise<void>;
  setConversationPinned: (id: string, pinned: boolean) => Promise<void>;
  deleteConversation: (id: string) => Promise<void>;
  setView: (view: View) => void;
  toggleSidebar: () => void;
  search: (query: string) => Promise<void>;
  clearSearch: () => void;
  respondApproval: (
    callId: string,
    decision: "once" | "always" | "reject",
    feedback?: string,
  ) => Promise<void>;
  respondInput: (callId: string, responses: unknown, cancelled: boolean) => Promise<void>;
  toast: (
    text: string,
    kind?: "error" | "info",
    actions?: ToastAction[],
  ) => void;
  dismissToast: (id: string) => void;
  setTheme: (theme: Theme) => Promise<void>;
  /** Change reading comfort settings. Applies at once and is saved. */
  setAppearance: (change: Partial<Appearance>) => Promise<void>;
  loadOlderMessages: () => Promise<void>;
  reloadActive: () => Promise<void>;
}

const MESSAGE_PAGE_SIZE = 80;

// A send crosses IPC before the backend turn is registered. If the user hits
// Stop in that window, remember it and cancel as soon as the send returns.
let sendingConversationId: string | null = null;
let stopRequestedFor: string | null = null;

let unsubscribe: (() => void) | null = null;
let initializing = false;

function discardStaged(attachments: Attachment[]): void {
  if (attachments.length === 0) return;
  void call("discard_staged_attachments", {
    ids: attachments.map((attachment) => attachment.id),
  }).catch(() => undefined);
}

export const useApp = create<AppStore>()((set, get) => ({
  booted: false,
  conversations: [],
  conversationLimit: 200,
  conversationHasMore: false,
  activeId: null,
  messages: [],
  messageHasMore: false,
  loadingOlder: false,
  pendingAttachments: [],
  draft: "",
  providers: [],
  servers: [],
  settings: {},
  appInfo: null,
  updateInfo: null,
  updateChecking: false,
  updateError: null,
  view: "chat",
  sidebarOpen: true,
  status: "idle",
  streamingMessageId: null,
  approvals: [],
  inputRequests: [],
  usage: null,
  step: null,
  limitReached: false,
  autoApprove: false,
  compactDialog: null,
  toasts: [],
  searchHits: [],
  searching: false,

  async init() {
    if (get().booted || initializing) return;
    initializing = true;
    if (!unsubscribe) {
      unsubscribe = await subscribe((event) => handleCoreEvent(event, set, get));
    }
    try {
      const [appInfo, providers, servers, settings, conversations] = await Promise.all([
        call<AppInfo>("app_info"),
        call<ProviderView[]>("list_providers"),
        call<ServerView[]>("list_servers"),
        call<Record<string, string>>("list_settings"),
        call<Conversation[]>("list_conversations", { limit: 200, offset: 0 }),
      ]);
      applyTheme((settings.theme as Theme) ?? "system");
      applyAppearance(readAppearance(settings));
      set({
        appInfo,
        providers,
        servers,
        settings,
        conversations,
        conversationLimit: 200,
        conversationHasMore: conversations.length >= 200,
        booted: true,
      });
      if (settings["update.check_on_startup"] !== "0") {
        void get().checkForUpdate(false);
      }
    } catch (error) {
      set({ booted: true });
      get().toast(String(error), "error");
    } finally {
      initializing = false;
    }
  },

  async refreshConversations() {
    try {
      const limit = Math.min(get().conversationLimit, 500);
      const conversations = await call<Conversation[]>("list_conversations", {
        limit,
        offset: 0,
      });
      // Do not touch activeId/messages here. A slow list response from before a
      // new chat was created must not make the open conversation disappear.
      set({
        conversations,
        conversationHasMore: conversations.length >= limit && limit < 500,
      });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async loadMoreConversations() {
    const current = get().conversations;
    try {
      const next = await call<Conversation[]>("list_conversations", {
        limit: 100,
        offset: current.length,
      });
      const seen = new Set(current.map((conversation) => conversation.id));
      const merged = [...current, ...next.filter((conversation) => !seen.has(conversation.id))];
      set({
        conversations: merged,
        conversationLimit: merged.length,
        conversationHasMore: next.length >= 100 && merged.length < 500,
      });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async refreshProviders() {
    try {
      set({ providers: await call<ProviderView[]>("list_providers") });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async refreshServers() {
    try {
      set({ servers: await call<ServerView[]>("list_servers") });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async refreshSettings() {
    try {
      set({ settings: await call<Record<string, string>>("list_settings") });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async checkForUpdate(manual = false) {
    if (get().updateChecking) return;
    set({
      updateChecking: true,
      ...(manual ? { updateError: null, updateInfo: null } : {}),
    });
    try {
      const info = await call<UpdateInfo>("check_for_update");
      set({ updateInfo: info, updateChecking: false, updateError: null });
      if (info.available) {
        get().toast(`Jarvis ${info.latestVersion} is available.`, "info", [
          {
            label: "View release",
            onClick: () => {
              void call("open_external", { url: info.url }).catch((error) =>
                get().toast(String(error), "error"),
              );
            },
          },
        ]);
      } else if (manual) {
        get().toast(`Jarvis ${info.currentVersion} is up to date.`, "info");
      }
    } catch (error) {
      const message = String(error);
      set({ updateChecking: false, updateError: message });
      if (manual) get().toast(message, "error");
    }
  },

  async openConversation(id) {
    discardStaged(get().pendingAttachments);
    set({
      activeId: id,
      messages: [],
      messageHasMore: false,
      loadingOlder: false,
      view: "chat",
      status: "idle",
      usage: null,
      step: null,
      limitReached: false,
      autoApprove: false,
      draft: "",
      pendingAttachments: [],
    });
    await get().reloadActive();
    const generating = await call<boolean>("is_generating", { conversationId: id }).catch(
      () => false,
    );
    if (get().activeId !== id) return;
    const pendingApproval =
      generating && get().approvals.some((item) => item.conversationId === id);
    const pendingInput =
      generating && get().inputRequests.some((item) => item.conversationId === id);
    set({
      status: pendingApproval
        ? "waiting_approval"
        : pendingInput
          ? "running_tool"
          : generating
            ? "thinking"
            : "idle",
      ...(generating
        ? {}
        : {
            approvals: get().approvals.filter((item) => item.conversationId !== id),
            inputRequests: get().inputRequests.filter((item) => item.conversationId !== id),
          }),
    });
    const autoApprove = await call<boolean>("get_auto_approve", {
      conversationId: id,
    }).catch(() => false);
    if (get().activeId === id) set({ autoApprove });
    const conversation = get().conversations.find((item) => item.id === id);
    if (conversation) {
      document.title = `${conversation.title} · Jarvis`;
    }
  },

  async newChat(seed = "") {
    try {
      const current = get().conversations.find((item) => item.id === get().activeId);
      const conversation = await call<Conversation>("create_conversation", {
        input: {
          workspaceRoot: current?.workspaceRoot ?? null,
        },
      });
      discardStaged(get().pendingAttachments);
      set((state) => ({
        conversations: [conversation, ...state.conversations],
        activeId: conversation.id,
        messages: [],
        messageHasMore: false,
        loadingOlder: false,
        pendingAttachments: [],
        draft: seed,
        view: "chat",
        status: "idle",
        usage: null,
        step: null,
        limitReached: false,
        autoApprove: false,
      }));
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async sendMessage(text) {
    const conversationId = get().activeId;
    const attachments = get().pendingAttachments;
    if (!conversationId || (!text.trim() && attachments.length === 0)) return;
    if (get().providers.length === 0) {
      get().toast("Add a provider in Settings before sending a message.", "error");
      set({ view: "settings" });
      return;
    }
    // The user is in front of the app right now, so a system permission prompt
    // would be seen. Asking later, from a background event, would not be.
    void primeNotifications(notificationsEnabled(get().settings));
    const tempId = `temp-${Date.now()}`;
    const parts: Part[] = [];
    if (text.trim()) {
      parts.push({
        id: `${tempId}-part`,
        messageId: tempId,
        idx: 0,
        kind: "text",
        payloadJson: JSON.stringify({ text }),
      });
    }
    attachments.forEach((attachment, index) => {
      parts.push({
        id: `${tempId}-attachment-${index}`,
        messageId: tempId,
        idx: parts.length,
        kind: "attachment",
        payloadJson: JSON.stringify({
          id: attachment.id,
          name: attachment.name,
          mime: attachment.mime,
          size: attachment.size,
          fileKind: attachment.kind,
          path: attachment.path,
          text: attachment.text ?? null,
          truncated: Boolean(attachment.truncated),
        }),
      });
    });
    sendingConversationId = conversationId;
    stopRequestedFor = null;
    const optimistic: MessageWithParts = {
      id: tempId,
      conversationId,
      role: "user",
      createdAt: Date.now(),
      status: "completed",
      error: null,
      parts,
    };
    set((state) => ({
      messages: [...state.messages, optimistic],
      draft: "",
      status: "thinking",
      approvals: state.approvals.filter(
        (item) => item.conversationId !== conversationId,
      ),
      inputRequests: state.inputRequests.filter(
        (item) => item.conversationId !== conversationId,
      ),
      usage: null,
      step: null,
      limitReached: false,
    }));
    let sent = false;
    try {
      const message = await call<MessageWithParts>("send_message", {
        conversationId,
        text,
        attachmentIds: attachments.map((attachment) => attachment.id),
      });
      set((state) => ({
        pendingAttachments: [],
        messages: state.messages.map((item) =>
          item.id === tempId
            ? {
                ...item,
                id: message.id,
                parts: message.parts,
              }
            : item,
        ),
      }));
      sent = true;
      void get().refreshConversations();
    } catch (error) {
      set((state) => ({
        messages: state.messages.filter((item) => item.id !== tempId),
        status: "error",
        draft: text,
      }));
      get().toast(String(error), "error", [
        { label: "Retry", onClick: () => void get().sendMessage(text) },
      ]);
    } finally {
      if (sendingConversationId === conversationId) sendingConversationId = null;
      if (stopRequestedFor === conversationId) {
        stopRequestedFor = null;
        if (sent) {
          // The send is now registered on the backend. Queueing the stop here
          // means a fast Escape cannot be lost in the IPC gap.
          void call("stop_generation", { conversationId }).catch((error) =>
            get().toast(String(error), "error"),
          );
        }
      }
    }
  },

  async steerMessage(text) {
    const conversationId = get().activeId;
    const attachments = get().pendingAttachments;
    if (!conversationId || (!text.trim() && attachments.length === 0)) return;
    if (get().providers.length === 0) {
      get().toast("Add a provider in Settings before sending a message.", "error");
      set({ view: "settings" });
      return;
    }
    set({ draft: "" });
    try {
      const message = await call<MessageWithParts>("steer_message", {
        conversationId,
        text,
        attachmentIds: attachments.map((attachment) => attachment.id),
      });
      set((state) => {
        const exists = state.messages.some((item) => item.id === message.id);
        const messages = exists
          ? state.messages
          : [...state.messages, message].sort(
              (a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id),
            );
        return {
          pendingAttachments: [],
          messages,
          status: "thinking",
          streamingMessageId: null,
          approvals: state.approvals.filter(
            (item) => item.conversationId !== conversationId,
          ),
          inputRequests: state.inputRequests.filter(
            (item) => item.conversationId !== conversationId,
          ),
          usage: null,
          step: null,
          limitReached: false,
        };
      });
      void get().refreshConversations();
    } catch (error) {
      set({ draft: text });
      get().toast(String(error), "error", [
        { label: "Retry", onClick: () => void get().steerMessage(text) },
      ]);
    }
  },

  async stageAttachments(paths) {
    const existing = get().pendingAttachments;
    const room = 6 - existing.length;
    if (room <= 0) {
      get().toast("You can attach up to 6 files per message.", "error");
      return;
    }
    const selected = paths.slice(0, room);
    if (paths.length > room) {
      get().toast(`Only ${room} more attachment${room === 1 ? "" : "s"} fit.`, "error");
    }
    try {
      const staged = await call<Attachment[]>("stage_attachments", {
        paths: selected,
      });
      set((state) => ({
        pendingAttachments: [...state.pendingAttachments, ...staged].slice(0, 6),
      }));
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async stageImageFile(file) {
    const existing = get().pendingAttachments;
    if (existing.length >= 6) {
      get().toast("You can attach up to 6 files per message.", "error");
      return;
    }
    if (file.type && !file.type.startsWith("image/")) {
      get().toast("Only image data can be pasted as an attachment.", "error");
      return;
    }
    if (file.size > 5 * 1024 * 1024) {
      get().toast("Clipboard image is larger than 5 MB.", "error");
      return;
    }
    try {
      const data = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result ?? ""));
        reader.onerror = () => reject(reader.error ?? new Error("Could not read clipboard image."));
        reader.readAsDataURL(file);
      });
      const staged = await call<Attachment>("stage_attachment_bytes", {
        data,
        name: file.name || "clipboard.png",
      });
      set((state) => ({
        pendingAttachments: [...state.pendingAttachments, staged].slice(0, 6),
      }));
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async stageClipboardImage() {
    if (get().pendingAttachments.length >= 6) {
      get().toast("You can attach up to 6 files per message.", "error");
      return;
    }
    try {
      const staged = await call<Attachment>("read_clipboard_image");
      set((state) => ({
        pendingAttachments: [...state.pendingAttachments, staged].slice(0, 6),
      }));
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async removeAttachment(id) {
    set((state) => ({
      pendingAttachments: state.pendingAttachments.filter(
        (attachment) => attachment.id !== id,
      ),
    }));
    try {
      await call("discard_staged_attachments", { ids: [id] });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  setDraft(text) {
    set({ draft: text });
  },

  async stop() {
    const conversationId = get().activeId;
    if (!conversationId) return;
    if (sendingConversationId === conversationId) {
      stopRequestedFor = conversationId;
      return;
    }
    try {
      await call("stop_generation", { conversationId });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  openCompact(focus = "") {
    const state = get();
    if (!state.activeId || state.view !== "chat") {
      state.toast("Open a conversation to compact it.", "info");
      return;
    }
    if (BUSY_STATUSES.includes(state.status)) {
      state.toast("Stop the current turn before compacting.", "info");
      return;
    }
    set({ compactDialog: { focus } });
  },

  closeCompact() {
    set({ compactDialog: null });
  },

  async compactConversation(focus = "") {
    const conversationId = get().activeId;
    set({ compactDialog: null });
    if (!conversationId) return;
    try {
      // Progress arrives as status events (Compacting, then Idle) and the
      // summary as a system row, exactly like an automatic compaction.
      await call("compact_conversation", {
        conversationId,
        focus: focus.trim() === "" ? null : focus.trim(),
      });
      get().toast("Conversation compacted.", "info");
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async regenerate() {
    const conversationId = get().activeId;
    if (!conversationId) return;
    try {
      set((state) => ({
        status: "thinking",
        approvals: state.approvals.filter(
          (item) => item.conversationId !== conversationId,
        ),
        inputRequests: state.inputRequests.filter(
          (item) => item.conversationId !== conversationId,
        ),
        step: null,
        limitReached: false,
      }));
      await call("regenerate", { conversationId });
      await get().reloadActive();
    } catch (error) {
      set({ status: "error" });
      get().toast(String(error), "error", [
        { label: "Retry", onClick: () => void get().regenerate() },
      ]);
    }
  },

  async revertMessage(messageId) {
    const conversationId = get().activeId;
    if (!conversationId) return;
    try {
      const result = await call<RevertResult>("revert_message", {
        conversationId,
        messageId,
      });
      if (get().activeId !== conversationId) return;
      await get().reloadActive();
      if (get().activeId !== conversationId) return;
      discardStaged(get().pendingAttachments);
      set((state) => ({
        draft: result.text,
        pendingAttachments: result.attachments,
        status: "idle",
        usage: null,
        step: null,
        limitReached: false,
        approvals: state.approvals.filter(
          (item) => item.conversationId !== conversationId,
        ),
        inputRequests: state.inputRequests.filter(
          (item) => item.conversationId !== conversationId,
        ),
      }));
      requestAnimationFrame(() => {
        document
          .querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')
          ?.focus();
      });
      get().toast("Message moved to the input box.", "info");
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async continueTurn() {
    const conversationId = get().activeId;
    if (!conversationId) return;
    try {
      set((state) => ({
        status: "thinking",
        approvals: state.approvals.filter(
          (item) => item.conversationId !== conversationId,
        ),
        inputRequests: state.inputRequests.filter(
          (item) => item.conversationId !== conversationId,
        ),
        step: null,
        limitReached: false,
      }));
      await call("continue_turn", { conversationId });
    } catch (error) {
      set({ status: "error" });
      get().toast(String(error), "error");
    }
  },

  async setAutoApprove(enabled) {
    const conversationId = get().activeId;
    if (!conversationId) return;
    try {
      const next = await call<boolean>("set_auto_approve", {
        conversationId,
        enabled,
      });
      set({ autoApprove: next });
      get().toast(
        next
          ? "Auto-approve enabled for this chat. Dangerous shell commands still require approval."
          : "Auto-approve disabled.",
        "info",
      );
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async setConversationWorkspace(conversationId, workspaceRoot) {
    const resolved = await call<string>("set_conversation_workspace", {
      conversationId,
      workspaceRoot,
    });
    set((state) => ({
      conversations: state.conversations.map((conversation) =>
        conversation.id === conversationId
          ? { ...conversation, workspaceRoot: resolved }
          : conversation,
      ),
    }));
    await get().refreshSettings();
    await get().refreshConversations();
  },

  async setModel(providerId, model) {
    const conversationId = get().activeId;
    if (!conversationId) return;
    try {
      await call("set_conversation_model", { conversationId, providerId, model });
      await get().refreshConversations();
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async renameConversation(id, title) {
    try {
      await call("rename_conversation", { conversationId: id, title });
      await get().refreshConversations();
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async setConversationPinned(id, pinned) {
    set((state) => ({
      conversations: state.conversations.map((conversation) =>
        conversation.id === id ? { ...conversation, pinned } : conversation,
      ),
    }));
    try {
      await call("set_conversation_pinned", { conversationId: id, pinned });
      await get().refreshConversations();
    } catch (error) {
      await get().refreshConversations();
      get().toast(String(error), "error");
    }
  },

  async deleteConversation(id) {
    try {
      await call("delete_conversation", { conversationId: id });
      if (get().activeId === id) {
        discardStaged(get().pendingAttachments);
        set({
          activeId: null,
          messages: [],
          messageHasMore: false,
          loadingOlder: false,
          pendingAttachments: [],
          draft: "",
          status: "idle",
          usage: null,
          step: null,
          limitReached: false,
          autoApprove: false,
        });
      }
      await get().refreshConversations();
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  setView(view) {
    set({ view });
  },

  toggleSidebar() {
    set((state) => ({ sidebarOpen: !state.sidebarOpen }));
  },

  async search(query) {
    if (!query.trim()) {
      set({ searchHits: [], searching: false });
      return;
    }
    set({ searching: true });
    try {
      const searchHits = await call<SearchHit[]>("search_conversations", { query });
      set({ searchHits, searching: false });
    } catch (error) {
      set({ searching: false });
      get().toast(String(error), "error");
    }
  },

  clearSearch() {
    set({ searchHits: [], searching: false });
  },

  async respondApproval(callId, decision, feedback) {
    set((state) => ({
      approvals: state.approvals.filter((approval) => approval.callId !== callId),
    }));
    try {
      await call("respond_approval", { callId, decision, feedback });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async respondInput(callId, responses, cancelled) {
    set((state) => ({
      inputRequests: state.inputRequests.filter((request) => request.callId !== callId),
    }));
    try {
      await call("respond_input", { callId, responses, cancelled });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  toast(text, kind = "error", actions = []) {
    if (
      get().toasts.some(
        (toast) => toast.text === text && toast.kind === kind,
      )
    ) {
      return;
    }
    const id = `toast-${Date.now()}-${Math.random()}`;
    set((state) => ({
      toasts: [...state.toasts.slice(-4), { id, text, kind, actions }],
    }));
    window.setTimeout(() => {
      set((state) => ({
        toasts: state.toasts.filter((toast) => toast.id !== id),
      }));
    }, kind === "error" ? 12_000 : 5_000);
  },

  dismissToast(id) {
    set((state) => ({ toasts: state.toasts.filter((toast) => toast.id !== id) }));
  },

  async setAppearance(change) {
    const next = { ...readAppearance(get().settings), ...change };
    // Normalize through the reader so an out-of-range value cannot be stored.
    const stored = appearanceSettings(next);
    const appearance = readAppearance(stored);
    applyAppearance(appearance);
    set((state) => ({ settings: { ...state.settings, ...appearanceSettings(appearance) } }));
    try {
      for (const [key, value] of Object.entries(appearanceSettings(appearance))) {
        await call("set_setting", { key, value });
      }
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async setTheme(theme) {
    applyTheme(theme);
    set((state) => ({ settings: { ...state.settings, theme } }));
    try {
      await call("set_setting", { key: "theme", value: theme });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async loadOlderMessages() {
    const id = get().activeId;
    if (!id || !get().messageHasMore || get().loadingOlder) return;
    const oldest = get().messages[0]?.id;
    if (!oldest) return;
    set({ loadingOlder: true });
    try {
      const page = await call<MessagePage>("get_messages_page", {
        conversationId: id,
        before: oldest,
        limit: MESSAGE_PAGE_SIZE,
      });
      if (get().activeId !== id) return;
      const known = new Set(get().messages.map((message) => message.id));
      const older = page.messages.filter((message) => !known.has(message.id));
      set((state) => ({
        messages: [...older, ...state.messages],
        messageHasMore: page.hasMore,
      }));
    } catch (error) {
      get().toast(String(error), "error");
    } finally {
      if (get().activeId === id) set({ loadingOlder: false });
    }
  },

  async reloadActive() {
    const id = get().activeId;
    if (!id) return;
    try {
      const page = await call<MessagePage>("get_messages_page", {
        conversationId: id,
        limit: MESSAGE_PAGE_SIZE,
      });
      if (get().activeId === id) {
        set({ messages: page.messages, messageHasMore: page.hasMore });
      }
    } catch (error) {
      get().toast(String(error), "error");
    }
  },
}));

export function applyCoreEvent(
  event: CoreEvent,
  set: (partial: Partial<AppStore> | ((state: AppStore) => Partial<AppStore>)) => void,
  get: () => AppStore,
) {
  const active = get().activeId;
  if ("conversationId" in event && event.conversationId && event.conversationId !== active) {
    const conversationId = event.conversationId;
    if (event.type === "approval") {
      set((state) => ({
        approvals: state.approvals.some((item) => item.callId === event.callId)
          ? state.approvals
          : [...state.approvals, event],
      }));
    } else if (event.type === "input_request") {
      set((state) => ({
        inputRequests: state.inputRequests.some((item) => item.callId === event.callId)
          ? state.inputRequests
          : [...state.inputRequests, event],
      }));
    } else if (
      event.type === "status" &&
      (event.status === "idle" || event.status === "error")
    ) {
      set((state) => ({
        approvals: state.approvals.filter((item) => item.conversationId !== conversationId),
        inputRequests: state.inputRequests.filter(
          (item) => item.conversationId !== conversationId,
        ),
      }));
    } else if (event.type === "conversation_updated") {
      void get().refreshConversations();
    }
    return;
  }
  switch (event.type) {
    case "status": {
      const patch: Partial<AppStore> = { status: event.status };
      if (event.status === "idle" || event.status === "error") {
        patch.streamingMessageId = null;
        patch.approvals = get().approvals.filter(
          (item) => item.conversationId !== event.conversationId,
        );
        patch.inputRequests = get().inputRequests.filter(
          (item) => item.conversationId !== event.conversationId,
        );
      }
      set(patch);
      if (event.message && event.status === "error") get().toast(event.message, "error");
      break;
    }
    case "message_start": {
      const system = event.role === "system";
      set((state) => {
        if (state.messages.some((message) => message.id === event.messageId)) {
          return system ? {} : { streamingMessageId: event.messageId };
        }
        return {
          ...(system ? {} : { streamingMessageId: event.messageId }),
          messages: [
            ...state.messages,
            {
              id: event.messageId,
              conversationId: event.conversationId,
              role: system ? "system" : "assistant",
              createdAt: Date.now(),
              status: system ? "completed" : "streaming",
              error: null,
              parts: [],
            },
          ],
        };
      });
      break;
    }
    case "delta": {
      set((state) => ({
        messages: state.messages.map((message) => {
          if (message.id !== event.messageId) return message;
          const existing = message.parts.find((part) => part.id === event.partId);
          if (!existing) {
            return {
              ...message,
              parts: [
                ...message.parts,
                {
                  id: event.partId,
                  messageId: event.messageId,
                  idx: message.parts.length,
                  kind: event.deltaType,
                  payloadJson: JSON.stringify({ text: event.text }),
                },
              ],
            };
          }
          const payload = parsePayload(existing);
          return {
            ...message,
            parts: message.parts.map((part) =>
              part.id === event.partId
                ? {
                    ...part,
                    payloadJson: JSON.stringify({
                      ...payload,
                      text: String(payload.text ?? "") + event.text,
                    }),
                  }
                : part,
            ),
          };
        }),
      }));
      break;
    }
    case "part": {
      set((state) => ({
        messages: upsertMessagePart(state.messages, event),
      }));
      break;
    }
    case "tool": {
      set((state) => ({
        messages: state.messages.map((message) => ({
          ...message,
          parts: message.parts.map((part) => {
            if (part.kind !== "tool_call") return part;
            const payload = parsePayload(part);
            if (payload.callId !== event.callId) return part;
            const next: Record<string, unknown> = { ...payload, state: event.state };
            if (event.progress != null) next.progress = event.progress;
            if (event.message && (event.state === "completed" || event.state === "error")) {
              next.resultPreview = event.message;
            }
            if (event.state === "error" && event.message) next.error = event.message;
            if (event.diff) next.diff = event.diff;
            if (typeof event.exitCode === "number") next.exitCode = event.exitCode;
            if (event.image) next.image = event.image;
            // A tool that is running or finished has been answered. Without
            // this the row kept saying "needs approval" until the next reload.
            if (payload.approval === "pending" && event.state !== "queued" && event.state !== "streaming") {
              next.approval = event.state === "error" || event.state === "aborted" ? "rejected" : "allowed";
            }
            if (event.state === "completed" || event.state === "error" || event.state === "aborted") {
              next.endedAt = Date.now();
            }
            return { ...part, payloadJson: JSON.stringify(next) };
          }),
        })),
      }));
      break;
    }
    case "approval": {
      set((state) => ({
        approvals: state.approvals.some((item) => item.callId === event.callId)
          ? state.approvals
          : [...state.approvals, event],
        status: "waiting_approval",
      }));
      break;
    }
    case "usage": {
      set({ usage: event });
      break;
    }
    case "step": {
      set({
        step: { current: event.step, max: event.maxSteps },
        limitReached: event.maxSteps > 0 && event.step > event.maxSteps,
      });
      break;
    }
    case "input_request": {
      set((state) => ({
        inputRequests: state.inputRequests.some((item) => item.callId === event.callId)
          ? state.inputRequests
          : [...state.inputRequests, event],
        status: "running_tool",
      }));
      break;
    }
    case "message_end": {
      set((state) => ({
        messages: state.messages.map((message) =>
          message.id === event.messageId
            ? { ...message, status: event.status, error: event.error ?? null }
            : message,
        ),
        streamingMessageId:
          state.streamingMessageId === event.messageId ? null : state.streamingMessageId,
      }));
      // Do not reload the transcript here. A tool turn emits MessageEnd for
      // the tool step before the final answer streams; a delayed SQLite reload
      // would replace newer state with the older snapshot. The explicit
      // reloads (open conversation / regenerate) and the DB on restart are
      // enough.
      break;
    }
    case "conversation_updated": {
      void get().refreshConversations();
      break;
    }
    case "resync": {
      const id = get().activeId;
      void (async () => {
        const active = id
          ? await call<boolean>("is_generating", { conversationId: id }).catch(() => false)
          : false;
        if (get().activeId !== id) return;
        await get().reloadActive();
        if (get().activeId !== id) return;
        const pendingApproval =
          active && get().approvals.some((item) => item.conversationId === id);
        const pendingInput =
          active && get().inputRequests.some((item) => item.conversationId === id);
        set((state) => ({
          status: pendingApproval
            ? "waiting_approval"
            : pendingInput
              ? "running_tool"
              : active
                ? "thinking"
                : "idle",
          streamingMessageId: active ? state.streamingMessageId : null,
          ...(active
            ? {}
            : {
                approvals: state.approvals.filter(
                  (item) => item.conversationId !== id,
                ),
                inputRequests: state.inputRequests.filter(
                  (item) => item.conversationId !== id,
                ),
              }),
        }));
      })();
      get().toast(event.reason, "info");
      break;
    }
    case "error": {
      get().toast(event.message, "error");
      break;
    }
  }
}

type StoreSet = (
  partial: Partial<AppStore> | ((state: AppStore) => Partial<AppStore>),
) => void;
type DeltaEvent = Extract<CoreEvent, { type: "delta" }>;

let pendingDeltas: DeltaEvent[] = [];
let deltaFlushTimer: ReturnType<typeof setTimeout> | null = null;

function flushPendingDeltas(set: StoreSet, get: () => AppStore) {
  if (deltaFlushTimer != null) {
    clearTimeout(deltaFlushTimer);
    deltaFlushTimer = null;
  }
  const batch = pendingDeltas;
  pendingDeltas = [];
  const merged: DeltaEvent[] = [];
  for (const event of batch) {
    const last = merged[merged.length - 1];
    if (
      last &&
      last.conversationId === event.conversationId &&
      last.messageId === event.messageId &&
      last.partId === event.partId &&
      last.deltaType === event.deltaType
    ) {
      merged[merged.length - 1] = { ...last, text: last.text + event.text };
    } else {
      merged.push(event);
    }
  }
  for (const event of merged) applyCoreEvent(event, set, get);
}

/**
 * Tell the user, with a desktop notification, about an event they cannot see
 * because the window is in the background or another chat is open.
 */
function announce(event: CoreEvent, get: () => AppStore): void {
  if (event.type !== "status" && event.type !== "approval" && event.type !== "input_request") {
    return;
  }
  const state = get();
  void maybeNotify(event, {
    enabled: notificationsEnabled(state.settings),
    activeId: state.activeId,
    titleOf: (id) => state.conversations.find((item) => item.id === id)?.title,
  });
}

function handleCoreEvent(event: CoreEvent, set: StoreSet, get: () => AppStore) {
  if (event.type !== "delta") {
    flushPendingDeltas(set, get);
    // Look at the state as it was before this event changed it.
    announce(event, get);
    applyCoreEvent(event, set, get);
    return;
  }
  pendingDeltas.push(event);
  if (deltaFlushTimer == null) {
    deltaFlushTimer = window.setTimeout(() => {
      deltaFlushTimer = null;
      flushPendingDeltas(set, get);
    }, 16);
  }
}

function upsertMessagePart(
  messages: MessageWithParts[],
  event: Extract<CoreEvent, { type: "part" }>,
): MessageWithParts[] {
  const payloadJson = JSON.stringify(event.payload);
  const part: Part = {
    id: event.partId,
    messageId: event.messageId,
    idx: 0,
    kind: event.kind,
    payloadJson,
  };
  const hasMessage = messages.some((message) => message.id === event.messageId);
  const base = hasMessage
    ? messages
    : [
        ...messages,
        {
          id: event.messageId,
          conversationId: event.conversationId,
          role: "assistant",
          createdAt: Date.now(),
          status: "streaming",
          error: null,
          parts: [],
        } satisfies MessageWithParts,
      ];
  return base.map((message) => {
    if (message.id !== event.messageId) return message;
    const exists = message.parts.some((item) => item.id === event.partId);
    return {
      ...message,
      parts: exists
        ? message.parts.map((item) =>
            item.id === event.partId ? { ...item, payloadJson } : item,
          )
        : [...message.parts, { ...part, idx: message.parts.length }],
    };
  });
}

export function parsePayload(part: Part): Record<string, unknown> {
  try {
    const value = JSON.parse(part.payloadJson) as unknown;
    return value && typeof value === "object" ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

export function partText(part: Part): string {
  const payload = parsePayload(part);
  return typeof payload.text === "string" ? payload.text : "";
}

let themeCleanup: (() => void) | null = null;

export function applyTheme(theme: Theme): void {
  themeCleanup?.();
  themeCleanup = null;
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const resolve = () => {
    const effective = theme === "system" ? (media.matches ? "dark" : "light") : theme;
    document.documentElement.dataset.theme = effective;
    document.documentElement.style.colorScheme = effective;
  };
  resolve();
  if (theme === "system") {
    media.addEventListener("change", resolve);
    themeCleanup = () => media.removeEventListener("change", resolve);
  }
}
