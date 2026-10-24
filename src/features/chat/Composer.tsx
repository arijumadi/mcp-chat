import { useEffect, useRef } from "react";
import { ArrowUp, Square } from "lucide-react";
import { useApp } from "@/app/store";
import { cn } from "@/lib/cn";
import { formatTokens } from "@/lib/format";

const ACTIVE: string[] = ["thinking", "streaming", "waiting_approval", "running_tool"];

export function Composer() {
  const value = useApp((state) => state.draft);
  const setValue = useApp((state) => state.setDraft);
  const ref = useRef<HTMLTextAreaElement>(null);
  const send = useApp((state) => state.sendMessage);
  const stop = useApp((state) => state.stop);
  const status = useApp((state) => state.status);
  const providers = useApp((state) => state.providers);
  const usage = useApp((state) => state.usage);
  const setView = useApp((state) => state.setView);
  const active = ACTIVE.includes(status);
  const disabled = providers.length === 0;

  useEffect(() => {
    const textarea = ref.current;
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 240)}px`;
  }, [value]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && ACTIVE.includes(useApp.getState().status)) {
        event.preventDefault();
        void useApp.getState().stop();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const submit = () => {
    const text = value.trim();
    if (!text || active || disabled) return;
    setValue("");
    void send(text);
  };

  return (
    <div className="shrink-0 px-4 pb-4">
      <div className="mx-auto w-full max-w-[760px]">
        {disabled ? (
          <div
            className="mb-2 flex items-center justify-between gap-3 rounded-lg border px-3 py-2 text-[12px]"
            style={{ borderColor: "var(--border)", background: "var(--panel)" }}
          >
            <span style={{ color: "var(--muted)" }}>
              Add a provider and API key to start chatting.
            </span>
            <button
              onClick={() => setView("settings")}
              className="focus-ring rounded px-1 font-medium"
              style={{ color: "var(--accent)" }}
            >
              Open settings
            </button>
          </div>
        ) : null}
        <div
          className={cn(
            "flex items-end gap-2 rounded-2xl border p-2 transition",
            active && "opacity-95",
          )}
          style={{
            borderColor: "var(--border-strong)",
            background: "var(--panel)",
            boxShadow: "0 8px 28px rgb(0 0 0 / 0.06)",
          }}
        >
          <textarea
            ref={ref}
            value={value}
            onChange={(event) => setValue(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                submit();
              }
            }}
            rows={1}
            disabled={disabled}
            placeholder={disabled ? "Configure a provider first…" : "Message the model…"}
            className="max-h-[240px] min-h-[38px] flex-1 resize-none bg-transparent px-2 py-2 text-[14.5px] leading-relaxed outline-none placeholder:text-[var(--faint)] disabled:cursor-not-allowed"
            aria-label="Message"
          />
          {active ? (
            <button
              onClick={() => void stop()}
              className="focus-ring mb-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-xl transition hover:brightness-95"
              style={{ background: "var(--fg)", color: "var(--bg)" }}
              title="Stop generating (Esc)"
              aria-label="Stop generating"
            >
              <Square size={13} fill="currentColor" />
            </button>
          ) : (
            <button
              onClick={submit}
              disabled={!value.trim() || disabled}
              className="focus-ring mb-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-xl transition hover:brightness-95 disabled:cursor-not-allowed disabled:opacity-30"
              style={{ background: "var(--accent)", color: "var(--accent-fg)" }}
              title="Send (Enter)"
              aria-label="Send message"
            >
              <ArrowUp size={17} strokeWidth={2.2} />
            </button>
          )}
        </div>
        <div
          className="mt-1.5 flex items-center justify-between px-1 text-[10.5px]"
          style={{ color: "var(--faint)" }}
        >
          <span>Enter to send · Shift+Enter for a new line</span>
          <span>
            {usage
              ? `${active ? "Esc to stop · " : ""}ctx ~${formatTokens(usage.promptTokens)} / ${formatTokens(usage.contextWindow)}${
                  usage.completionTokens > 0
                    ? ` · out ${formatTokens(usage.completionTokens)}`
                    : ""
                }`
              : active
                ? "Esc to stop"
                : "Answers can be wrong. Check tool output."}
          </span>
        </div>
      </div>
    </div>
  );
}
