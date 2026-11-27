import { useEffect, useRef, useState } from "react";
import { ChevronDown, Settings2 } from "lucide-react";
import { useApp } from "@/app/store";
import type { AgentConfigOption, Conversation } from "@/lib/types";

/**
 * Model, thinking level, mode and any other option an external agent exposes.
 *
 * These are the agent's own settings, not Jarvis's: the list comes from the
 * agent's `session/new` response and its `config_option_update` notifications.
 * They replace the model and reasoning pickers for an ACP chat, because those
 * belong to the built-in runner.
 */
export function AgentConfigPicker({
  conversation,
  disabled = false,
}: {
  conversation: Conversation;
  disabled?: boolean;
}) {
  const options = useApp((state) => state.agentConfig);
  const setConfig = useApp((state) => state.setAgentConfig);
  const [open, setOpen] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onClick = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) setOpen(null);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  const selectable = options.filter((option) => option.kind === "select" && option.choices.length > 0);
  if (selectable.length === 0) return null;

  const label = (option: AgentConfigOption) =>
    option.choices.find((choice) => choice.value === option.currentValue)?.name ??
    option.currentValue;

  return (
    <div ref={ref} className="flex min-w-0 flex-wrap items-center gap-1 sm:flex-nowrap">
      {selectable.map((option) => (
        <div key={option.id} className="relative min-w-0">
          <button
            type="button"
            onClick={() => setOpen((value) => (value === option.id ? null : option.id))}
            disabled={disabled}
            className="focus-ring flex max-w-[190px] items-center gap-1 rounded-md border border-transparent px-1.5 py-1 text-[10px] transition hover:bg-[var(--panel-2)] disabled:cursor-not-allowed disabled:opacity-50"
            style={{ color: "var(--muted)" }}
            title={option.description ?? option.name}
          >
            <Settings2 size={10} className="shrink-0" />
            {/*
              The option name is hidden on a phone: "Thinking Level High" does
              not fit a 393px screen and wrapping is what made the composer tall.
              The value is what the user is actually choosing.
            */}
            <span className="hidden truncate sm:inline" style={{ color: "var(--faint)" }}>
              {option.name}
            </span>
            <span className="truncate font-medium" style={{ color: "var(--fg)" }}>
              {label(option)}
            </span>
            <ChevronDown size={10} />
          </button>
          {open === option.id ? (
            <div
              className="absolute bottom-full left-0 z-30 mb-1 max-h-[min(50vh,320px)] w-[260px] overflow-y-auto overscroll-contain rounded-xl border p-1 shadow-xl"
              style={{ background: "var(--panel)", borderColor: "var(--border)" }}
            >
              {option.choices.map((choice) => {
                const chosen = choice.value === option.currentValue;
                return (
                  <button
                    key={choice.value}
                    type="button"
                    onClick={() => {
                      void setConfig(conversation.id, option.id, choice.value);
                      setOpen(null);
                    }}
                    className="focus-ring flex w-full flex-col rounded-lg px-2 py-1.5 text-left transition hover:bg-[var(--panel-2)]"
                    style={{ background: chosen ? "var(--panel-2)" : "transparent" }}
                  >
                    <span className="truncate text-[11.5px] font-medium">{choice.name}</span>
                    {choice.description ? (
                      <span className="truncate text-[10.5px]" style={{ color: "var(--muted)" }}>
                        {choice.description}
                      </span>
                    ) : null}
                  </button>
                );
              })}
            </div>
          ) : null}
        </div>
      ))}
    </div>
  );
}
