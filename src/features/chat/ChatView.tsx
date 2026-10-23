import { useState } from "react";
import {
  Download,
  FileJson,
  FileText,
  PanelLeft,
  Plus,
  Settings,
  Plug,
  Trash2,
} from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Logo } from "@/components/Logo";
import { Modal } from "@/components/Modal";
import { cn } from "@/lib/cn";
import { call, downloadText } from "@/lib/ipc";
import type { ExportResult } from "@/lib/types";
import { ModelPicker } from "./ModelPicker";
import { Transcript } from "./Transcript";
import { Composer } from "./Composer";

export function ChatView() {
  const activeId = useApp((state) => state.activeId);
  const conversations = useApp((state) => state.conversations);
  const messages = useApp((state) => state.messages);
  const providers = useApp((state) => state.providers);
  const servers = useApp((state) => state.servers);
  const sidebarOpen = useApp((state) => state.sidebarOpen);
  const toggleSidebar = useApp((state) => state.toggleSidebar);
  const setView = useApp((state) => state.setView);
  const newChat = useApp((state) => state.newChat);
  const deleteConversation = useApp((state) => state.deleteConversation);
  const renameConversation = useApp((state) => state.renameConversation);
  const toast = useApp((state) => state.toast);
  const conversation = conversations.find((item) => item.id === activeId);

  const [editingTitle, setEditingTitle] = useState(false);
  const [title, setTitle] = useState("");
  const [exportOpen, setExportOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  if (!conversation) {
    return (
      <EmptyChat
        providers={providers.length}
        servers={servers.length}
        sidebarOpen={sidebarOpen}
        onToggleSidebar={toggleSidebar}
        onNewChat={() => void newChat()}
        onSettings={() => setView("settings")}
        onServers={() => setView("servers")}
      />
    );
  }

  const doExport = async (format: "markdown" | "json") => {
    setExportOpen(false);
    try {
      const result = await call<ExportResult>("export_conversation", {
        conversationId: conversation.id,
        format,
      });
      downloadText(result.filename, result.content, result.mime);
      toast(`Exported ${result.filename}`, "info");
    } catch (error) {
      toast(String(error), "error");
    }
  };

  const saveTitle = () => {
    setEditingTitle(false);
    const trimmed = title.trim();
    if (trimmed && trimmed !== conversation.title) {
      void renameConversation(conversation.id, trimmed);
    }
  };

  return (
    <div className="flex h-full min-w-0 flex-1 flex-col">
      <header
        className="flex h-14 shrink-0 items-center gap-2 border-b px-3"
        style={{ borderColor: "var(--border)", background: "var(--bg)" }}
      >
        {!sidebarOpen ? (
          <button
            onClick={toggleSidebar}
            className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
            title="Open sidebar"
            aria-label="Open sidebar"
          >
            <PanelLeft size={16} />
          </button>
        ) : null}
        <div className="min-w-0 flex-1 px-1">
          {editingTitle ? (
            <input
              autoFocus
              value={title}
              onChange={(event) => setTitle(event.target.value)}
              onBlur={saveTitle}
              onKeyDown={(event) => {
                if (event.key === "Enter") saveTitle();
                if (event.key === "Escape") setEditingTitle(false);
              }}
              className="w-full max-w-[420px] rounded border bg-transparent px-1.5 py-0.5 text-[14px] font-medium outline-none"
              style={{ borderColor: "var(--accent)" }}
            />
          ) : (
            <button
              onClick={() => {
                setTitle(conversation.title);
                setEditingTitle(true);
              }}
              className="focus-ring block max-w-full truncate rounded text-left text-[14px] font-medium tracking-[-0.01em]"
              title="Rename conversation"
            >
              {conversation.title}
            </button>
          )}
          <div className="truncate text-[10.5px]" style={{ color: "var(--faint)" }}>
            {conversation.model ?? "no model"} · local transcript
          </div>
        </div>
        <ModelPicker conversation={conversation} />
        <div className="relative">
          <button
            onClick={() => setExportOpen((value) => !value)}
            className="focus-ring flex items-center gap-1 rounded-lg border px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)]"
            style={{ borderColor: "var(--border)", color: "var(--muted)" }}
            title="Export conversation"
          >
            <Download size={13} /> Export
          </button>
          {exportOpen ? (
            <>
              <button
                className="fixed inset-0 z-20 cursor-default"
                aria-label="Close export menu"
                onClick={() => setExportOpen(false)}
              />
              <div
                className="animate-in absolute top-full right-0 z-30 mt-1.5 w-44 overflow-hidden rounded-xl border py-1 shadow-xl"
                style={{ background: "var(--panel)", borderColor: "var(--border)" }}
              >
                <button
                  onClick={() => void doExport("markdown")}
                  className="flex w-full items-center gap-2 px-3 py-2 text-left text-[12.5px] transition hover:bg-[var(--panel-2)]"
                >
                  <FileText size={13} /> Markdown
                </button>
                <button
                  onClick={() => void doExport("json")}
                  className="flex w-full items-center gap-2 px-3 py-2 text-left text-[12.5px] transition hover:bg-[var(--panel-2)]"
                >
                  <FileJson size={13} /> JSON
                </button>
              </div>
            </>
          ) : null}
        </div>
        <button
          onClick={() => setConfirmDelete(true)}
          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--danger-soft)] hover:text-[var(--danger)]"
          title="Delete conversation"
          aria-label="Delete conversation"
        >
          <Trash2 size={15} />
        </button>
      </header>

      {messages.length === 0 ? (
        <div className="flex min-h-0 flex-1 items-center justify-center px-6">
          <div className="max-w-sm text-center">
            <div className="mx-auto mb-3 flex items-center justify-center">
              <Logo size={44} />
            </div>
            <div className="text-[16px] font-medium tracking-[-0.01em]">New conversation</div>
            <p className="mt-1 text-[12.5px]" style={{ color: "var(--muted)" }}>
              Ask a question, paste some code, or connect an MCP server and let the model use its
              tools.
            </p>
          </div>
        </div>
      ) : (
        <Transcript />
      )}
      <Composer />

      <Modal
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title="Delete this conversation?"
        subtitle="Messages, tool results and summaries will be removed locally."
        width="sm"
        footer={
          <>
            <Button size="sm" variant="ghost" onClick={() => setConfirmDelete(false)}>
              Cancel
            </Button>
            <Button
              size="sm"
              variant="danger"
              onClick={() => {
                setConfirmDelete(false);
                void deleteConversation(conversation.id);
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
    </div>
  );
}

function EmptyChat({
  providers,
  servers,
  sidebarOpen,
  onToggleSidebar,
  onNewChat,
  onSettings,
  onServers,
}: {
  providers: number;
  servers: number;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  onNewChat: () => void;
  onSettings: () => void;
  onServers: () => void;
}) {
  return (
    <div className="flex h-full min-w-0 flex-1 flex-col">
      <header
        className="flex h-14 shrink-0 items-center gap-2 border-b px-3"
        style={{ borderColor: "var(--border)" }}
      >
        {!sidebarOpen ? (
          <button
            onClick={onToggleSidebar}
            className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
            aria-label="Open sidebar"
          >
            <PanelLeft size={16} />
          </button>
        ) : null}
        <div className="ml-1 text-[12px]" style={{ color: "var(--faint)" }}>
          No conversation open
        </div>
        <div className="ml-auto flex items-center gap-1.5">
          <Button size="sm" variant="ghost" icon={<Plug size={13} />} onClick={onServers}>
            Servers
          </Button>
          <Button size="sm" variant="ghost" icon={<Settings size={13} />} onClick={onSettings}>
            Settings
          </Button>
        </div>
      </header>
      <div className="flex min-h-0 flex-1 items-center justify-center px-6">
        <div className="max-w-[430px] text-center">
          <div className="mx-auto mb-5 flex justify-center">
            <Logo size={64} />
          </div>
          <h1 className="text-[24px] font-semibold tracking-[-0.03em]">
            A quiet place to talk to your tools.
          </h1>
          <p className="mt-2 text-[13.5px] leading-relaxed" style={{ color: "var(--muted)" }}>
            Stream answers from your own model endpoint and run remote MCP tools with explicit
            approval. Everything stays on this machine.
          </p>
          <div className="mt-6 flex items-center justify-center gap-2">
            <Button variant="primary" icon={<Plus size={14} />} onClick={onNewChat}>
              New chat
            </Button>
            {providers === 0 ? (
              <Button variant="secondary" onClick={onSettings}>
                Add a provider
              </Button>
            ) : (
              <Button variant="secondary" icon={<Plug size={14} />} onClick={onServers}>
                MCP servers
              </Button>
            )}
          </div>
          <div
            className="mt-6 flex items-center justify-center gap-4 text-[10.5px]"
            style={{ color: "var(--faint)" }}
          >
            <span className={cn(providers > 0 ? "text-[var(--accent)]" : "")}>
              {providers} provider{providers === 1 ? "" : "s"}
            </span>
            <span>·</span>
            <span>
              {servers} MCP server{servers === 1 ? "" : "s"}
            </span>
            <span>·</span>
            <span>⌘K to switch</span>
          </div>
        </div>
      </div>
    </div>
  );
}
