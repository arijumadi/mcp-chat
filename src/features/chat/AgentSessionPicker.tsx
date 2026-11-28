import { useCallback, useEffect, useMemo, useState } from "react";
import { Check, ChevronDown, Loader2, RefreshCw } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { AcpAgentView, AgentSession, AgentSessionPage, Conversation } from "@/lib/types";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass } from "@/components/ui";

/**
 * Continue a conversation that already exists on an agent.
 *
 * Lists sessions with `session/list` and binds one with `session/load`. The
 * session's own `cwd` is always sent, never the chat's workspace: OpenCode
 * rejects a mismatch and Antigravity would silently move the session, so
 * rewriting the directory is never what the user asked for.
 *
 * Coverage differs per agent and is not something Jarvis can change. Hermes only
 * lists the sessions it created through ACP; Antigravity and OpenCode also show
 * the ones from their own CLI.
 */
export function AgentSessionPicker({
  conversation,
  open,
  onClose,
}: {
  conversation: Conversation;
  open: boolean;
  onClose: () => void;
}) {
  const toast = useApp((state) => state.toast);
  const refreshConversations = useApp((state) => state.refreshConversations);
  const refreshAgentConfig = useApp((state) => state.refreshAgentConfig);
  const refreshAgentCommands = useApp((state) => state.refreshAgentCommands);
  const openConversation = useApp((state) => state.openConversation);

  const [agents, setAgents] = useState<AcpAgentView[]>([]);
  const [agentId, setAgentId] = useState<string>("");
  const [page, setPage] = useState<AgentSessionPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [attaching, setAttaching] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    void (async () => {
      try {
        const list = await call<AcpAgentView[]>("list_acp_agents_for_chat");
        setAgents(list);
        // Default to the agent this chat already runs on: listing Antigravity
        // from an OpenCode chat is never what the user meant.
        const bound = list.find((agent) => agent.slug === conversation.acpAgent);
        setAgentId((current) => current || bound?.id || list[0]?.id || "");
      } catch (cause) {
        setError(String(cause));
      }
    })();
  }, [open]);

  const load = useCallback(async () => {
    if (!agentId) return;
    setLoading(true);
    setError(null);
    setPage(null);
    try {
      // No cwd filter: the point is to see everything the agent will show, and
      // each row carries the directory it belongs to.
      const result = await call<AgentSessionPage>("list_agent_sessions", {
        agentId,
      });
      setPage(result);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }, [agentId]);

  useEffect(() => {
    if (open && agentId) void load();
  }, [open, agentId, load]);

  const filtered = useMemo(() => {
    const sessions = page?.sessions ?? [];
    const needle = query.trim().toLowerCase();
    if (!needle) return sessions;
    return sessions.filter(
      (session) =>
        (session.title ?? "").toLowerCase().includes(needle) ||
        session.cwd.toLowerCase().includes(needle) ||
        session.sessionId.toLowerCase().includes(needle),
    );
  }, [page, query]);

  const attach = async (session: AgentSession) => {
    setAttaching(session.sessionId);
    try {
      await call("attach_agent_session", {
        conversationId: conversation.id,
        agentId,
        sessionId: session.sessionId,
        cwd: session.cwd,
      });
      await Promise.all([
        refreshConversations(),
        refreshAgentConfig(conversation.id),
        refreshAgentCommands(conversation.id),
        // The agent replayed the history on load and Jarvis dropped it, so the
        // transcript is unchanged; reloading is still right because the parts
        // the agent wrote while attaching are not in it.
        openConversation(conversation.id),
      ]);
      toast("This chat continues that agent session.", "info");
      onClose();
    } catch (cause) {
      toast(String(cause), "error");
    } finally {
      setAttaching(null);
    }
  };

  const current = conversation.acpSessionId ?? null;

  return (
    <Modal
      title="Continue an agent session"
      open={open}
      onClose={onClose}
      width="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button variant="ghost" onClick={() => void load()} disabled={loading || !agentId}>
            <RefreshCw size={13} className={loading ? "animate-spin" : ""} /> Reload
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <p className="text-[length:var(--fs-status)] leading-relaxed" style={{ color: "var(--muted)" }}>
          Sessions made in the agent&apos;s own CLI are listed here when that
          agent supports it. Attaching copies the agent&apos;s history into this
          chat, so it stays readable and moves with you if you switch agents
          later. Very long sessions are imported from the most recent turns.
        </p>

        <div className="flex flex-wrap items-center gap-2">
          <label className="text-[length:var(--fs-mono)]" style={{ color: "var(--muted)" }}>
            Agent
          </label>
          <div className="relative">
            <select
              value={agentId}
              onChange={(event) => setAgentId(event.target.value)}
              className={`${inputClass} pr-7`}
            >
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name}
                </option>
              ))}
            </select>
            <ChevronDown
              size={11}
              className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2"
              style={{ color: "var(--muted)" }}
            />
          </div>
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Filter by title or directory…"
            className={`${inputClass} min-w-0 flex-1`}
          />
        </div>

        {error ? (
          <div
            className="rounded-lg border px-3 py-2 text-[length:var(--fs-status)]"
            style={{
              borderColor: "color-mix(in srgb, var(--danger) 40%, var(--border))",
              background: "var(--danger-soft)",
            }}
          >
            {error}
          </div>
        ) : null}

        {loading ? (
          <div className="flex items-center gap-2 py-6 text-[length:var(--fs-mono)]" style={{ color: "var(--muted)" }}>
            <Loader2 size={13} className="animate-spin" /> Asking the agent for its sessions…
          </div>
        ) : page && !page.supported ? (
          <div
            className="rounded-lg border px-3 py-2 text-[length:var(--fs-status)]"
            style={{ borderColor: "var(--border)", color: "var(--muted)" }}
          >
            This agent does not implement session listing, so it cannot show
            sessions Jarvis did not create.
          </div>
        ) : filtered.length === 0 ? (
          <div className="py-6 text-[length:var(--fs-status)]" style={{ color: "var(--muted)" }}>
            {query
              ? "No session matches that filter."
              : "This agent has no sessions to show."}
          </div>
        ) : (
          <div className="max-h-[min(50vh,420px)] overflow-y-auto overscroll-contain rounded-lg border" style={{ borderColor: "var(--border)" }}>
            {filtered.map((session) => {
              const isCurrent = session.sessionId === current;
              const busy = attaching === session.sessionId;
              return (
                <button
                  key={session.sessionId}
                  type="button"
                  onClick={() => void attach(session)}
                  disabled={attaching !== null || isCurrent}
                  className="focus-ring flex w-full items-start gap-2 border-b px-3 py-2 text-left transition last:border-b-0 hover:bg-[var(--panel-2)] disabled:cursor-default"
                  style={{ borderColor: "var(--border)" }}
                >
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[length:var(--fs-status)] font-medium">
                      {session.title ?? session.sessionId}
                    </div>
                    <div className="truncate font-mono text-[length:var(--fs-meta)]" style={{ color: "var(--muted)" }}>
                      {session.cwd}
                    </div>
                  </div>
                  {busy ? (
                    <Loader2 size={13} className="mt-0.5 shrink-0 animate-spin" />
                  ) : isCurrent ? (
                    <Check size={13} className="mt-0.5 shrink-0" style={{ color: "var(--accent)" }} />
                  ) : null}
                </button>
              );
            })}
          </div>
        )}
      </div>
    </Modal>
  );
}
