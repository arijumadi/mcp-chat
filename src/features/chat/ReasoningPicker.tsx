import { useEffect, useRef, useState } from "react";
import { Brain, Check, ChevronDown } from "lucide-react";
import { useApp } from "@/app/store";
import type { Conversation, ReasoningEffort } from "@/lib/types";

const OPTIONS: Array<{
  value: ReasoningEffort;
  label: string;
  hint: string;
}> = [
  { value: "default", label: "Default", hint: "Provider default; no extra parameter" },
  { value: "low", label: "Low", hint: "Faster, less reasoning" },
  { value: "medium", label: "Medium", hint: "Balanced" },
  { value: "high", label: "High", hint: "More reasoning, usually slower" },
  { value: "max", label: "Max", hint: "Highest effort supported by the model" },
];

export function ReasoningPicker({
  conversation,
  disabled = false,
}: {
  conversation: Conversation;
  disabled?: boolean;
}) {
  const setReasoningEffort = useApp((state) => state.setReasoningEffort);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const current = conversation.reasoningEffort ?? "default";
  const selected = OPTIONS.find((option) => option.value === current) ?? OPTIONS[0]!;

  useEffect(() => {
    if (!open) return;
    const onClick = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  const choose = async (effort: ReasoningEffort) => {
    setOpen(false);
    await setReasoningEffort(conversation.id, effort);
  };

  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        disabled={disabled}
        className="focus-ring flex max-w-[140px] items-center gap-1.5 rounded-lg border border-transparent px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)] disabled:cursor-not-allowed disabled:opacity-50"
        style={{ color: "var(--muted)" }}
        title={`Reasoning effort: ${selected.label}`}
        aria-label={`Reasoning effort: ${selected.label}`}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <Brain size={12} className="shrink-0" />
        <span className="truncate font-medium" style={{ color: "var(--fg)" }}>
          {selected.label}
        </span>
        <ChevronDown size={12} className="shrink-0" />
      </button>

      {open ? (
        <div
          className="animate-in absolute bottom-full left-0 z-30 mb-1.5 w-[270px] rounded-xl border p-3 shadow-xl"
          style={{ background: "var(--panel)", borderColor: "var(--border)" }}
        >
          <div className="mb-1 text-[12.5px] font-medium">Reasoning effort</div>
          <div className="mb-2 text-[10.5px] leading-relaxed" style={{ color: "var(--faint)" }}>
            Default sends no extra parameter. Other levels depend on model support.
          </div>
          <div role="listbox" aria-label="Reasoning effort">
            {OPTIONS.map((option) => {
              const active = option.value === current;
              return (
                <button
                  key={option.value}
                  type="button"
                  role="option"
                  aria-selected={active}
                  onClick={() => void choose(option.value)}
                  className="focus-ring flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition hover:bg-[var(--panel-2)]"
                  style={{ background: active ? "var(--panel-2)" : "transparent" }}
                >
                  <span className="min-w-0 flex-1">
                    <span className="block text-[12.5px]">{option.label}</span>
                    <span className="block text-[10.5px]" style={{ color: "var(--faint)" }}>
                      {option.hint}
                    </span>
                  </span>
                  {active ? (
                    <Check size={12} className="mt-0.5 shrink-0" style={{ color: "var(--accent)" }} />
                  ) : null}
                </button>
              );
            })}
          </div>
        </div>
      ) : null}
    </div>
  );
}
