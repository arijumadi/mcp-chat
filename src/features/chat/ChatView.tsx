import { useState } from "react";
import { CornerUpLeft, PanelLeft, Plus, Settings, Plug } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Logo } from "@/components/Logo";
import { cn } from "@/lib/cn";
import { ConversationTools } from "./ConversationTools";
import { SetupChecklist } from "./SetupChecklist";
import { Transcript } from "./Transcript";
import { Composer } from "./Composer";

export function ChatView() {
  const activeId = useApp((state) => state.activeId);
  const conversations = useApp((state) => state.conversations);
  const subagents = useApp((state) => state.subagents);
  const messages = useApp((state) => state.messages);
  const providers = useApp((state) => state.providers);
  const servers = useApp((state) => state.servers);
  const sidebarOpen = useApp((state) => state.sidebarOpen);
  const toggleSidebar = useApp((state) => state.toggleSidebar);
  const setView = useApp((state) => state.setView);
  const setDraft = useApp((state) => state.setDraft);
  const newChat = useApp((state) => state.newChat);
  const openConversation = useApp((state) => state.openConversation);
  const renameConversation = useApp((state) => state.renameConversation);
  const conversation =
    conversations.find((item) => item.id === activeId) ??
    (activeId ? subagents[activeId] : undefined);

  const [editingTitle, setEditingTitle] = useState(false);
  const [title, setTitle] = useState("");

  if (!conversation) {
    return (
      <EmptyChat
        providers={providers.length}
        hasProviderKey={providers.some(
          (provider) =>
            provider.hasKey ||
            /^(https?:\/\/)?(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])/i.test(
              provider.baseUrl,
            ),
        )}
        servers={servers.length}
        sidebarOpen={sidebarOpen}
        onToggleSidebar={toggleSidebar}
        onNewChat={(seed) => void newChat(seed)}
        onSettings={() => setView("settings")}
        onServers={() => setView("servers")}
      />
    );
  }

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
        {conversation.parentId ? (
          <button
            onClick={() => void openConversation(conversation.parentId!)}
            className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
            title="Back to parent chat"
            aria-label="Back to parent chat"
          >
            <CornerUpLeft size={16} />
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
            {conversation.parentId ? "subagent transcript" : "local transcript"}
          </div>
        </div>
        <ConversationTools conversation={conversation} />
      </header>

      {messages.length === 0 ? (
        <div className="flex min-h-0 flex-1 items-center justify-center px-6">
          <div className="w-full max-w-[640px] text-center">
            <div className="mx-auto mb-3 flex items-center justify-center">
              <Logo size={44} />
            </div>
            <div className="text-[16px] font-medium tracking-[-0.01em]">New conversation</div>
            <p className="mt-1 text-[12.5px]" style={{ color: "var(--muted)" }}>
              Ask a question, paste some code, or connect an MCP server and let the model use its
              tools.
            </p>
            {providers.length > 0 ? (
              <QuickStarts onSelect={(prompt) => setDraft(prompt)} />
            ) : null}
          </div>
        </div>
      ) : (
        <Transcript />
      )}
      {conversation.parentId ? (
        <div
          className="border-t px-4 py-3 text-center text-[11.5px]"
          style={{ borderColor: "var(--border)", color: "var(--faint)" }}
        >
          Subagent transcript · follow-ups are managed by the parent chat.
        </div>
      ) : (
        <Composer conversation={conversation} />
      )}

    </div>
  );
}

const QUICK_STARTS = [
  {
    title: "Map the workspace",
    body: "Orient me in this project and point to the files that matter.",
    prompt:
      "Map this workspace and tell me the main entry points, architecture, and the first files I should read.",
  },
  {
    title: "Find a bug",
    body: "Inspect the current state, tests, and likely failure points.",
    prompt:
      "Help me find and fix the next bug in this workspace. Start by checking the current state and tests.",
  },
  {
    title: "Use MCP tools",
    body: "See what tools are available and try a safe read-only call.",
    prompt:
      "Show me which MCP tools are available, then demonstrate one useful read-only call.",
  },
] as const;

function QuickStarts({ onSelect }: { onSelect: (prompt: string) => void }) {
  return (
    <div className="mt-6 grid w-full gap-2 text-left sm:grid-cols-3">
      {QUICK_STARTS.map((item) => (
        <button
          key={item.title}
          type="button"
          onClick={() => onSelect(item.prompt)}
          className="focus-ring rounded-xl border px-3 py-3 text-left transition hover:bg-[var(--panel-2)]"
          style={{ borderColor: "var(--border)", background: "var(--panel)" }}
        >
          <div className="text-[12.5px] font-medium">{item.title}</div>
          <div className="mt-0.5 text-[11.5px] leading-snug" style={{ color: "var(--muted)" }}>
            {item.body}
          </div>
        </button>
      ))}
    </div>
  );
}

function EmptyChat({
  providers,
  hasProviderKey,
  servers,
  sidebarOpen,
  onToggleSidebar,
  onNewChat,
  onSettings,
  onServers,
}: {
  providers: number;
  hasProviderKey: boolean;
  servers: number;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  onNewChat: (seed?: string) => void;
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
        <div className="ml-1 text-[12.5px]" style={{ color: "var(--faint)" }}>
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
        <div className="w-full max-w-[640px] text-center">
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
            <Button variant="primary" icon={<Plus size={14} />} onClick={() => onNewChat()}>
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
          {providers === 0 || !hasProviderKey ? (
            <SetupChecklist
              providers={providers}
              hasProviderKey={hasProviderKey}
              servers={servers}
              onSettings={onSettings}
              onServers={onServers}
            />
          ) : (
            <QuickStarts onSelect={(prompt) => onNewChat(prompt)} />
          )}
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
