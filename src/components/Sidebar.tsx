import { useEffect, useRef, useState } from "react";
import {
  Lock,
  MessageSquare,
  PanelLeftClose,
  Pin,
  Plug,
  Plus,
  Search,
  Settings,
  Trash2,
} from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { Logo } from "@/components/Logo";
import { cn } from "@/lib/cn";
import { groupConversations } from "@/lib/format";
import { inputClass } from "@/components/ui";

export function Sidebar() {
  const conversations = useApp((state) => state.conversations);
  const hasMoreConversations = useApp((state) => state.conversationHasMore);
  const loadMoreConversations = useApp((state) => state.loadMoreConversations);
  const activeId = useApp((state) => state.activeId);
  const view = useApp((state) => state.view);
  const openConversation = useApp((state) => state.openConversation);
  const newChat = useApp((state) => state.newChat);
  const setView = useApp((state) => state.setView);
  const toggleSidebar = useApp((state) => state.toggleSidebar);
  const renameConversation = useApp((state) => state.renameConversation);
  const setConversationPinned = useApp((state) => state.setConversationPinned);
  const deleteConversation = useApp((state) => state.deleteConversation);
  const searchHits = useApp((state) => state.searchHits);
  const search = useApp((state) => state.search);
  const searching = useApp((state) => state.searching);
  const clearSearch = useApp((state) => state.clearSearch);

  const [query, setQuery] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [deleting, setDeleting] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      void search(query);
    }, 180);
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
    };
  }, [query, search]);

  const groups = groupConversations(conversations);

  return (
    <aside
      className="flex h-full w-[260px] shrink-0 flex-col border-r"
      style={{ background: "var(--panel)", borderColor: "var(--border)" }}
    >
      <div className="flex items-center gap-2 px-3 pt-3.5 pb-2">
        <Logo size={24} />
        <div className="min-w-0 flex-1">
          <div className="text-[13.5px] font-semibold tracking-[-0.01em]">Jarvis</div>
          <div className="text-[10.5px]" style={{ color: "var(--faint)" }}>
            local-first · no telemetry
          </div>
        </div>
        <button
          onClick={toggleSidebar}
          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
          title="Collapse sidebar"
          aria-label="Collapse sidebar"
        >
          <PanelLeftClose size={16} />
        </button>
      </div>

      <div className="px-3 pb-2">
        <Button
          variant="secondary"
          className="w-full justify-start"
          icon={<Plus size={15} />}
          onClick={() => void newChat()}
        >
          New chat
        </Button>
      </div>

      <div className="relative px-3 pb-2">
        <Search
          size={14}
          className="pointer-events-none absolute top-1/2 left-[22px] -translate-y-1/2"
          style={{ color: "var(--faint)" }}
        />
        <input
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            if (!event.target.value) clearSearch();
          }}
          placeholder="Search conversations"
          className={cn(inputClass, "h-8 py-1 pl-8 text-[12.5px]")}
          style={{ borderColor: "var(--border)" }}
        />
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {query.trim() ? (
          <div className="pt-1">
            {searching ? (
              <div className="px-2 py-3 text-[12px]" style={{ color: "var(--faint)" }}>
                Searching…
              </div>
            ) : searchHits.length === 0 ? (
              <div className="px-2 py-3 text-[12px]" style={{ color: "var(--faint)" }}>
                No matches.
              </div>
            ) : (
              searchHits.map((hit) => (
                <button
                  key={hit.conversationId}
                  onClick={() => {
                    setQuery("");
                    clearSearch();
                    void openConversation(hit.conversationId);
                  }}
                  className="focus-ring mb-0.5 w-full rounded-lg px-2 py-2 text-left transition hover:bg-[var(--panel-2)]"
                >
                  <div className="truncate text-[12.5px] font-medium">{hit.title}</div>
                  {hit.snippet ? (
                    <div
                      className="line-clamp-2 text-[11.5px]"
                      style={{ color: "var(--muted)" }}
                    >
                      {hit.snippet}
                    </div>
                  ) : null}
                </button>
              ))
            )}
          </div>
        ) : conversations.length === 0 ? (
          <div className="px-3 py-6 text-[12px]" style={{ color: "var(--faint)" }}>
            No threads yet.
          </div>
        ) : (
          groups.map((group) => (
            <div key={group.label} className="mb-3">
              <div
                className="px-2 pt-2 pb-1 text-[10.5px] font-medium tracking-[0.08em] uppercase"
                style={{ color: "var(--faint)" }}
              >
                {group.label}
              </div>
              {group.items.map((conversation) => {
                const active = conversation.id === activeId && view === "chat";
                const editing = editingId === conversation.id;
                return (
                  <div
                    key={conversation.id}
                    className={cn(
                      "group relative mb-0.5 flex items-center rounded-lg transition",
                      active ? "bg-[var(--panel-2)]" : "hover:bg-[var(--panel-2)]",
                    )}
                  >
                    <MessageSquare
                      size={13}
                      className="ml-2 shrink-0"
                      style={{ color: active ? "var(--accent)" : "var(--faint)" }}
                    />
                    {editing ? (
                      <input
                        autoFocus
                        value={draft}
                        onChange={(event) => setDraft(event.target.value)}
                        onBlur={() => {
                          setEditingId(null);
                          if (draft.trim() && draft !== conversation.title) {
                            void renameConversation(conversation.id, draft.trim());
                          }
                        }}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") {
                            event.currentTarget.blur();
                          }
                          if (event.key === "Escape") {
                            setEditingId(null);
                          }
                        }}
                        className="min-w-0 flex-1 bg-transparent px-1.5 py-2 text-[12.5px] outline-none"
                      />
                    ) : (
                      <button
                        onClick={() => void openConversation(conversation.id)}
                        onDoubleClick={() => {
                          setEditingId(conversation.id);
                          setDraft(conversation.title);
                        }}
                        className="focus-ring min-w-0 flex-1 truncate py-2 pr-1 pl-1.5 text-left text-[12.5px]"
                        title={conversation.title}
                      >
                        {conversation.title}
                      </button>
                    )}
                    <button
                      onClick={() =>
                        void setConversationPinned(
                          conversation.id,
                          !conversation.pinned,
                        )
                      }
                      className={cn(
                        "focus-ring rounded p-1 transition group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 hover:bg-[var(--panel)]",
                        conversation.pinned ? "opacity-100" : "opacity-0",
                      )}
                      style={{
                        color: conversation.pinned ? "var(--accent)" : undefined,
                      }}
                      title={conversation.pinned ? "Unpin conversation" : "Pin conversation"}
                      aria-label={
                        conversation.pinned ? "Unpin conversation" : "Pin conversation"
                      }
                      aria-pressed={conversation.pinned}
                    >
                      <Pin
                        size={12}
                        fill={conversation.pinned ? "currentColor" : "none"}
                      />
                    </button>
                    <button
                      onClick={() => setDeleting(conversation.id)}
                      className="focus-ring mr-1 rounded p-1 opacity-0 transition group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 hover:text-[var(--danger)]"
                      title="Delete conversation"
                      aria-label="Delete conversation"
                    >
                      <Trash2 size={12.5} />
                    </button>
                  </div>
                );
              })}
            </div>
          ))
        )}
        {!query.trim() && hasMoreConversations ? (
          <button
            type="button"
            onClick={() => void loadMoreConversations()}
            className="focus-ring mt-1 w-full rounded-lg px-2 py-2 text-[11.5px] transition hover:bg-[var(--panel-2)]"
            style={{ color: "var(--muted)" }}
          >
            Load older conversations
          </button>
        ) : null}
      </div>

      <div
        className="border-t px-2 py-2"
        style={{ borderColor: "var(--border)" }}
      >
        <button
          onClick={() => setView("servers")}
          className={cn(
            "focus-ring flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[12.5px] transition",
            view === "servers"
              ? "bg-[var(--panel-2)]"
              : "hover:bg-[var(--panel-2)]",
          )}
        >
          <Plug size={14} />
          <span className="flex-1">MCP servers</span>
          <span className="flex items-center gap-0.5 text-[10px]" style={{ color: "var(--faint)" }}>
            <Lock size={10} /> local
          </span>
        </button>
        <button
          onClick={() => setView("settings")}
          className={cn(
            "focus-ring flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[12.5px] transition",
            view === "settings"
              ? "bg-[var(--panel-2)]"
              : "hover:bg-[var(--panel-2)]",
          )}
        >
          <Settings size={14} />
          <span className="flex-1">Settings</span>
        </button>
      </div>

      <Modal
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        title="Delete this conversation?"
        subtitle="Messages, tool results and summaries will be removed locally."
        width="sm"
        footer={
          <>
            <Button size="sm" variant="ghost" onClick={() => setDeleting(null)}>
              Cancel
            </Button>
            <Button
              size="sm"
              variant="danger"
              onClick={() => {
                const id = deleting;
                setDeleting(null);
                if (id) void deleteConversation(id);
              }}
            >
              Delete
            </Button>
          </>
        }
      >
        <p className="text-[13px]" style={{ color: "var(--muted)" }}>
          This cannot be undone.
        </p>
      </Modal>
    </aside>
  );
}
