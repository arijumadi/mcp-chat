import { create } from "zustand";
import { call, subscribe } from "@/lib/ipc";
import type {
  AppInfo,
  ApprovalEvent,
  Attachment,
  Conversation,
  CoreEvent,
  InputRequestEvent,
  MessageWithParts,
  Part,
  ProviderView,
  RevertResult,
  SearchHit,
  ServerView,
  TurnStatus,
  UsageEvent,
} from "@/lib/types";

export type View = "chat" | "settings" | "servers";
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
  pendingAttachments: Attachment[];
  draft: string;
  providers: ProviderView[];
  servers: ServerView[];
  settings: Record<string, string>;
  appInfo: AppInfo | null;
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
  toasts: Toast[];
  searchHits: SearchHit[];
  searching: boolean;
  init: () => Promise<void>;
  refreshConversations: () => Promise<void>;
  loadMoreConversations: () => Promise<void>;
  refreshProviders: () => Promise<void>;
  refreshServers: () => Promise<void>;
  refreshSettings: () => Promise<void>;
  openConversation: (id: string) => Promise<void>;
  newChat: () => Promise<void>;
  sendMessage: (text: string) => Promise<void>;
  steerMessage: (text: string) => Promise<void>;
  stageAttachments: (paths: string[]) => Promise<void>;
  stageImageFile: (file: File) => Promise<void>;
  removeAttachment: (id: string) => Promise<void>;
  setDraft: (text: string) => void;
  stop: () => Promise<void>;
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
  reloadActive: () => Promise<void>;
}

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
  pendingAttachments: [],
  draft: "",
  providers: [],
  servers: [],
  settings: {},
  appInfo: null,
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
  toasts: [],
  searchHits: [],
  searching: false,

  async init() {
    if (get().booted || initializing) return;
    initializing = true;
    if (!unsubscribe) {
      unsubscribe = await subscribe((event) => applyCoreEvent(event, set, get));
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

  async openConversation(id) {
    discardStaged(get().pendingAttachments);
    set({
      activeId: id,
      view: "chat",
      status: "idle",
      approvals: [],
      inputRequests: [],
      usage: null,
      step: null,
      limitReached: false,
      autoApprove: false,
      draft: "",
      pendingAttachments: [],
    });
    await get().reloadActive();
    const autoApprove = await call<boolean>("get_auto_approve", {
      conversationId: id,
    }).catch(() => false);
    if (get().activeId === id) set({ autoApprove });
    const conversation = get().conversations.find((item) => item.id === id);
    if (conversation) {
      document.title = `${conversation.title} · Jarvis`;
    }
  },

  async newChat() {
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
        pendingAttachments: [],
        draft: "",
        view: "chat",
        status: "idle",
        approvals: [],
        inputRequests: [],
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
      approvals: [],
      inputRequests: [],
      usage: null,
      step: null,
      limitReached: false,
    }));
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
          approvals: [],
          inputRequests: [],
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
    try {
      await call("stop_generation", { conversationId });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async regenerate() {
    const conversationId = get().activeId;
    if (!conversationId) return;
    try {
      set({
        status: "thinking",
        approvals: [],
        inputRequests: [],
        step: null,
        limitReached: false,
      });
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
      set({
        draft: result.text,
        pendingAttachments: result.attachments,
        status: "idle",
        usage: null,
        step: null,
        limitReached: false,
        approvals: [],
        inputRequests: [],
      });
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
      set({
        status: "thinking",
        approvals: [],
        inputRequests: [],
        step: null,
        limitReached: false,
      });
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

  async setTheme(theme) {
    applyTheme(theme);
    set((state) => ({ settings: { ...state.settings, theme } }));
    try {
      await call("set_setting", { key: "theme", value: theme });
    } catch (error) {
      get().toast(String(error), "error");
    }
  },

  async reloadActive() {
    const id = get().activeId;
    if (!id) return;
    try {
      const messages = await call<MessageWithParts[]>("get_messages", {
        conversationId: id,
      });
      if (get().activeId === id) set({ messages });
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
    if (event.type === "conversation_updated") void get().refreshConversations();
    return;
  }
  switch (event.type) {
    case "status": {
      const patch: Partial<AppStore> = { status: event.status };
      if (event.status === "idle" || event.status === "error") {
        patch.streamingMessageId = null;
        patch.approvals = [];
        patch.inputRequests = [];
      }
      set(patch);
      if (event.message && event.status === "error") get().toast(event.message, "error");
      break;
    }
    case "message_start": {
      set((state) => {
        if (state.messages.some((message) => message.id === event.messageId)) {
          return { streamingMessageId: event.messageId };
        }
        return {
          streamingMessageId: event.messageId,
          messages: [
            ...state.messages,
            {
              id: event.messageId,
              conversationId: event.conversationId,
              role: "assistant",
              createdAt: Date.now(),
              status: "streaming",
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
    case "error": {
      get().toast(event.message, "error");
      break;
    }
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
