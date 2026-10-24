import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Loader2, SlidersHorizontal } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { Conversation, ConversationToolsView } from "@/lib/types";
import { Button } from "@/components/Button";

const LAZY_TOOL_MODE = "lazy";

export function ConversationTools({
  conversation,
}: {
  conversation: Conversation;
}) {
  const servers = useApp((state) => state.servers);
  const refreshConversations = useApp((state) => state.refreshConversations);
  const toast = useApp((state) => state.toast);
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [allServers, setAllServers] = useState(true);
  const [serverIds, setServerIds] = useState<string[]>([]);
  const ref = useRef<HTMLDivElement>(null);

  const enabledServers = servers.filter(
    (server) => server.enabled && server.policy !== "deny",
  );
  const selectedCount = allServers
    ? enabledServers.length
    : enabledServers.filter((server) => serverIds.includes(server.id)).length;

  useEffect(() => {
    setOpen(false);
  }, [conversation.id]);

  useEffect(() => {
    if (!open) return;
    const onClick = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  const load = async () => {
    setLoading(true);
    try {
      const tools = await call<ConversationToolsView>("get_conversation_tools", {
        conversationId: conversation.id,
      });
      setAllServers(tools.allServers);
      setServerIds(tools.serverIds);
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setLoading(false);
    }
  };

  const toggleOpen = () => {
    const next = !open;
    setOpen(next);
    if (next) void load();
  };

  const toggleServer = (id: string) => {
    if (allServers) {
      setAllServers(false);
      setServerIds(
        enabledServers
          .map((server) => server.id)
          .filter((item) => item !== id),
      );
      return;
    }
    setServerIds((current) =>
      current.includes(id)
        ? current.filter((item) => item !== id)
        : [...current, id],
    );
  };

  const save = async () => {
    setSaving(true);
    try {
      await call("set_conversation_tools", {
        conversationId: conversation.id,
        serverIds: allServers ? null : serverIds,
        toolMode: LAZY_TOOL_MODE,
      });
      await refreshConversations();
      toast("Tool settings saved for this chat.", "info");
      setOpen(false);
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setSaving(false);
    }
  };

  return (
    <div ref={ref} className="relative">
      <button
        onClick={toggleOpen}
        className="focus-ring flex items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)]"
        style={{ borderColor: "var(--border)", color: "var(--muted)" }}
        title="Choose MCP tools for this chat"
      >
        <SlidersHorizontal size={13} />
        <span>MCP</span>
        <span className="font-medium" style={{ color: "var(--fg)" }}>
          {selectedCount}/{enabledServers.length}
        </span>
        <ChevronDown size={11} />
      </button>

      {open ? (
        <div
          className="animate-in absolute top-full right-0 z-30 mt-1.5 w-[360px] rounded-xl border p-3 shadow-xl"
          style={{ background: "var(--panel)", borderColor: "var(--border)" }}
        >
          {loading ? (
            <div
              className="flex items-center gap-2 py-6 text-[12px]"
              style={{ color: "var(--muted)" }}
            >
              <Loader2 size={13} className="animate-spin" /> Loading tools…
            </div>
          ) : (
            <>
              <div className="mb-2 flex items-center justify-between">
                <div className="text-[12px] font-medium">MCP servers for this chat</div>
                <button
                  onClick={() => {
                    setAllServers(true);
                    setServerIds([]);
                  }}
                  className="focus-ring flex items-center gap-1 rounded px-1 text-[10.5px] transition hover:bg-[var(--panel-2)]"
                  style={{ color: "var(--muted)" }}
                >
                  {allServers ? <Check size={11} /> : null} All
                </button>
              </div>
              <div className="mb-3 max-h-60 overflow-y-auto">
                {enabledServers.length === 0 ? (
                  <div
                    className="rounded-lg border px-3 py-2 text-[11.5px]"
                    style={{
                      borderColor: "var(--border)",
                      color: "var(--muted)",
                    }}
                  >
                    No enabled MCP servers.
                  </div>
                ) : (
                  enabledServers.map((server) => {
                    const checked = allServers || serverIds.includes(server.id);
                    return (
                      <label
                        key={server.id}
                        className="flex cursor-pointer items-center gap-2 rounded-lg px-2 py-1.5 text-[12px] transition hover:bg-[var(--panel-2)]"
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() => toggleServer(server.id)}
                        />
                        <span className="min-w-0 flex-1 truncate">{server.name}</span>
                        <span className="text-[10px]" style={{ color: "var(--faint)" }}>
                          {server.slug} · {server.policy}
                        </span>
                      </label>
                    );
                  })
                )}
              </div>

              <div className="flex justify-end gap-2">
                <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
                  Cancel
                </Button>
                <Button
                  size="sm"
                  variant="primary"
                  loading={saving}
                  onClick={() => void save()}
                >
                  Save
                </Button>
              </div>
            </>
          )}
        </div>
      ) : null}
    </div>
  );
}
