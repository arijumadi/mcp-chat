import { useEffect, useRef, useState } from "react";
import { Bot, ChevronDown } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { AcpAgentView, Conversation } from "@/lib/types";

/**
 * Switch a chat between the built-in model and an external ACP agent.
 *
 * The agent owns the conversation once bound: Jarvis forwards only the newest
 * user message and renders whatever the agent streams back. That is why the
 * picker says so instead of only naming the agent.
 */
export function AgentPicker({
  conversation,
  disabled = false,
}: {
  conversation: Conversation;
  disabled?: boolean;
}) {
  const toast = useApp((state) => state.toast);
  const refreshConversations = useApp((state) => state.refreshConversations);
  const refreshAgentConfig = useApp((state) => state.refreshAgentConfig);
  const refreshAgentCommands = useApp((state) => state.refreshAgentCommands);
  const [agents, setAgents] = useState<AcpAgentView[]>([]);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const list = await call<AcpAgentView[]>("list_acp_agents_for_chat");
        if (!cancelled) setAgents(list);
      } catch {
        // An empty list simply hides the picker; the settings page reports errors.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    const onClick = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  if (agents.length === 0) return null;

  const bound = conversation.acpAgent ?? null;
  const label = bound ? (agents.find((a) => a.slug === bound)?.name ?? bound) : "Built-in model";

  const choose = async (slug: string | null) => {
    setBusy(true);
    try {
      await call("set_conversation_acp_agent", {
        conversationId: conversation.id,
        slug,
      });
      await refreshConversations();
      setOpen(false);
      setBusy(false);

      // The switch itself is already done. Opening the agent happens in the
      // background and takes a second or two, so poll briefly instead of
      // holding the menu open: the user can keep working meanwhile.
      if (slug) {
        setPreparing(true);
        for (let attempt = 0; attempt < 40; attempt += 1) {
          const [config] = await Promise.all([
            refreshAgentConfig(conversation.id),
            refreshAgentCommands(conversation.id),
          ]);
          if (config.length > 0) break;
          await new Promise((resolve) => setTimeout(resolve, 150));
        }
        setPreparing(false);
      } else {
        await Promise.all([
          refreshAgentConfig(conversation.id),
          refreshAgentCommands(conversation.id),
        ]);
      }

      toast(
        slug
          ? "This chat now runs on the ACP agent."
          : "This chat is back on the built-in model.",
        "info",
      );
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        disabled={disabled || busy}
        className="focus-ring flex max-w-[220px] items-center gap-1.5 rounded-lg border border-transparent px-2.5 py-1.5 text-[10.5px] transition hover:bg-[var(--panel-2)] disabled:cursor-not-allowed disabled:opacity-50"
        style={{ color: "var(--muted)" }}
        title={bound ? "This chat runs on an ACP agent" : "Change agent"}
      >
        <Bot size={12} />
        <span className="truncate font-medium" style={{ color: "var(--fg)" }}>
          {label}
        </span>
        {preparing ? (
          <span className="truncate text-[10px]" style={{ color: "var(--muted)" }}>
            starting…
          </span>
        ) : null}
        <ChevronDown size={12} />
      </button>
      {open ? (
        <div
          className="absolute bottom-full left-0 z-30 mb-1 w-[260px] rounded-xl border p-1 shadow-xl"
          style={{ background: "var(--panel)", borderColor: "var(--border)" }}
        >
          <button
            type="button"
            onClick={() => void choose(null)}
            className="focus-ring flex w-full items-start gap-2 rounded-lg px-2 py-1.5 text-left transition hover:bg-[var(--panel-2)]"
          >
            <div className="min-w-0">
              <div className="text-[11.5px] font-medium">Built-in model</div>
              <div className="text-[10.5px]" style={{ color: "var(--muted)" }}>
                Jarvis runs the turn with its own tools and prompt.
              </div>
            </div>
          </button>
          {agents.map((agent) => (
            <button
              key={agent.id}
              type="button"
              onClick={() => void choose(agent.slug)}
              className="focus-ring flex w-full items-start gap-2 rounded-lg px-2 py-1.5 text-left transition hover:bg-[var(--panel-2)]"
            >
              <div className="min-w-0">
                <div className="truncate text-[11.5px] font-medium">{agent.name}</div>
                <div className="truncate text-[10.5px]" style={{ color: "var(--muted)" }}>
                  {[agent.command, ...agent.args].join(" ")}
                </div>
              </div>
            </button>
          ))}
          <p
            className="border-t px-2 pt-1.5 pb-1 text-[10px]"
            style={{ borderColor: "var(--border)", color: "var(--muted)" }}
          >
            An ACP agent keeps its own history. Jarvis tools, skills, and the
            system prompt do not apply to it.
          </p>
        </div>
      ) : null}
    </div>
  );
}
