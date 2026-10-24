import { useEffect, useRef, useState } from "react";
import {
  Check,
  ChevronDown,
  Loader2,
  SlidersHorizontal,
} from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type {
  ContextStatus,
  Conversation,
  ConversationToolsView,
} from "@/lib/types";
import { Button } from "@/components/Button";
import { cn } from "@/lib/cn";

export function ConversationTools({
  conversation,
}: {
  conversation: Conversation;
}) {
  const servers = useApp((state) => state.servers);
  const status = useApp((state) => state.status);
  const refreshConversations = useApp((state) => state.refreshConversations);
  const reloadActive = useApp((state) => state.reloadActive);
  const toast = useApp((state) => state.toast);
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [compacting, setCompacting] = useState(false);
  const [allServers, setAllServers] = useState(true);
  const [serverIds, setServerIds] = useState<string[]>([]);
  const [toolMode, setToolMode] = useState<"direct" | "lazy">("direct");
  const [context, setContext] = useState<ContextStatus | null>(null);
  const [contextError, setContextError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  const enabledServers = servers.filter((server) => server.enabled && server.policy !== "deny");
  const selectedCount = allServers
    ? enabledServers.length
    : enabledServers.filter((server) => serverIds.includes(server.id)).length;
  const busy = ["thinking", "streaming", "waiting_approval", "running_tool"].includes(status);

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
    setContextError(null);
    try {
      const [tools, status] = await Promise.all([
        call<ConversationToolsView>("get_conversation_tools", {
          conversationId: conversation.id,
        }),
        call<ContextStatus>("context_status", {
          conversationId: conversation.id,
        }).catch((error) => {
          setContextError(String(error));
          return null;
        }),
      ]);
      setAllServers(tools.allServers);
      setServerIds(tools.serverIds);
      setToolMode(tools.toolMode);
      setContext(status);
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
      setServerIds(enabledServers.map((server) => server.id).filter((item) => item !== id));
      return;
    }
    setServerIds((current) =>
      current.includes(id) ? current.filter((item) => item !== id) : [...current, id],
    );
  };

  const save = async () => {
    setSaving(true);
    try {
      await call("set_conversation_tools", {
        conversationId: conversation.id,
        serverIds: allServers ? null : serverIds,
        toolMode,
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

  const compact = async () => {
    setCompacting(true);
    try {
      await call("compact_conversation", { conversationId: conversation.id });
      await reloadActive();
      const next = await call<ContextStatus>("context_status", {
        conversationId: conversation.id,
      });
      setContext(next);
      toast("Conversation compacted.", "info");
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setCompacting(false);
    }
  };

  return (
    <div ref={ref} className="relative">
      <button
        onClick={toggleOpen}
        className="focus-ring flex items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)]"
        style={{ borderColor: "var(--border)", color: "var(--muted)" }}
        title="MCP tools and context for this chat"
      >
        <SlidersHorizontal size={13} />
        <span>Tools</span>
        <span className="font-medium" style={{ color: "var(--fg)" }}>
          {selectedCount}/{enabledServers.length}
        </span>
        {toolMode === "lazy" ? (
          <span
            className="text-[9px] font-medium tracking-[0.06em] uppercase"
            style={{ color: "var(--accent)" }}
          >
            lazy
          </span>
        ) : null}
        <ChevronDown size={11} />
      </button>

      {open ? (
        <div
          className="animate-in absolute top-full right-0 z-30 mt-1.5 w-[360px] rounded-xl border p-3 shadow-xl"
          style={{ background: "var(--panel)", borderColor: "var(--border)" }}
        >
          {loading ? (
            <div className="flex items-center gap-2 py-6 text-[12px]" style={{ color: "var(--muted)" }}>
              <Loader2 size={13} className="animate-spin" /> Loading tools and context…
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
              <div className="mb-3 max-h-40 overflow-y-auto">
                {enabledServers.length === 0 ? (
                  <div className="rounded-lg border px-3 py-2 text-[11.5px]" style={{ borderColor: "var(--border)", color: "var(--muted)" }}>
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

              <div className="mb-3">
                <div className="mb-1 text-[12px] font-medium">Tool delivery</div>
                <div className="grid grid-cols-2 gap-1.5">
                  {(
                    [
                      ["direct", "Direct schemas", "All selected tools in every request"],
                      ["lazy", "Lazy search + call", "Model finds tools on demand"],
                    ] as const
                  ).map(([value, label, hint]) => (
                    <button
                      key={value}
                      onClick={() => setToolMode(value)}
                      className={cn(
                        "focus-ring rounded-lg border px-2.5 py-2 text-left transition",
                        toolMode === value
                          ? "bg-[var(--panel-2)]"
                          : "hover:bg-[var(--panel-2)]",
                      )}
                      style={{
                        borderColor:
                          toolMode === value ? "var(--accent)" : "var(--border)",
                      }}
                    >
                      <div className="text-[11.5px] font-medium">{label}</div>
                      <div className="text-[9.5px] leading-tight" style={{ color: "var(--faint)" }}>
                        {hint}
                      </div>
                    </button>
                  ))}
                </div>
              </div>

              <div
                className="mb-3 rounded-lg border px-3 py-2.5"
                style={{ borderColor: "var(--border)", background: "var(--panel-2)" }}
              >
                <div className="flex items-center justify-between text-[11px]">
                  <span style={{ color: "var(--muted)" }}>Context estimate</span>
                  <span style={{ color: "var(--fg)" }}>
                    {context
                      ? `~${formatTokens(context.tokens)} / ${formatTokens(context.window)}`
                      : "unavailable"}
                  </span>
                </div>
                {context ? (
                  <>
                    <div
                      className="mt-1.5 h-1.5 overflow-hidden rounded-full"
                      style={{ background: "var(--border)" }}
                    >
                      <div
                        className="h-full rounded-full"
                        style={{
                          width: `${Math.min(100, (context.tokens / context.window) * 100)}%`,
                          background:
                            context.tokens > context.threshold
                              ? "var(--warn)"
                              : "var(--accent)",
                        }}
                      />
                    </div>
                    <div className="mt-1 flex items-center justify-between text-[9.5px]" style={{ color: "var(--faint)" }}>
                      <span>
                        compact at ~{formatTokens(context.threshold)} · keep{" "}
                        {formatTokens(context.keepTokens)}
                      </span>
                      <span>{context.toolCount} tools</span>
                    </div>
                  </>
                ) : contextError ? (
                  <div className="mt-1 text-[10px]" style={{ color: "var(--danger)" }}>
                    {contextError}
                  </div>
                ) : null}
                <div className="mt-2 flex justify-end">
                  <Button
                    size="sm"
                    variant="ghost"
                    loading={compacting}
                    disabled={busy || !context}
                    onClick={() => void compact()}
                  >
                    Compact now
                  </Button>
                </div>
              </div>

              <div className="flex justify-end gap-2">
                <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
                  Cancel
                </Button>
                <Button size="sm" variant="primary" loading={saving} onClick={() => void save()}>
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

function formatTokens(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (value >= 1000) return `${(value / 1000).toFixed(value >= 10_000 ? 0 : 1)}k`;
  return String(value);
}
