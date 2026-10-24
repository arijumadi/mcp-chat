import { useEffect, useRef, useState } from "react";
import { ArrowUp, Loader2, Paperclip, Square } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { cn } from "@/lib/cn";
import { formatTokens } from "@/lib/format";
import type { Conversation } from "@/lib/types";
import { ModelPicker } from "./ModelPicker";
import { WorkspacePicker } from "./WorkspacePicker";
import { AutoApproveButton } from "./AutoApproveButton";
import { AttachmentGrid } from "./Attachments";

const ACTIVE: string[] = ["thinking", "streaming", "waiting_approval", "running_tool"];

export function Composer({ conversation }: { conversation: Conversation }) {
  const value = useApp((state) => state.draft);
  const setValue = useApp((state) => state.setDraft);
  const ref = useRef<HTMLTextAreaElement>(null);
  const send = useApp((state) => state.sendMessage);
  const stop = useApp((state) => state.stop);
  const status = useApp((state) => state.status);
  const providers = useApp((state) => state.providers);
  const usage = useApp((state) => state.usage);
  const step = useApp((state) => state.step);
  const limitReached = useApp((state) => state.limitReached);
  const continueTurn = useApp((state) => state.continueTurn);
  const setView = useApp((state) => state.setView);
  const pendingAttachments = useApp((state) => state.pendingAttachments);
  const stageAttachments = useApp((state) => state.stageAttachments);
  const removeAttachment = useApp((state) => state.removeAttachment);
  const [attaching, setAttaching] = useState(false);
  const active = ACTIVE.includes(status);
  const disabled = providers.length === 0;
  const canSend =
    (value.trim().length > 0 || pendingAttachments.length > 0) &&
    !active &&
    !disabled;

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
    if ((!text && pendingAttachments.length === 0) || active || disabled) return;
    setValue("");
    void send(text);
  };

  const chooseFiles = async () => {
    if (active || disabled || attaching) return;
    if (window.__MCP_CHAT_MOCK__) {
      useApp
        .getState()
        .toast("The native file picker is unavailable in the browser demo.", "info");
      return;
    }
    setAttaching(true);
    try {
      const selected = await open({
        multiple: true,
        directory: false,
        title: "Attach files",
      });
      const paths =
        typeof selected === "string"
          ? [selected]
          : Array.isArray(selected)
            ? selected
            : [];
      if (paths.length > 0) await stageAttachments(paths);
    } catch (error) {
      useApp.getState().toast(String(error), "error");
    } finally {
      setAttaching(false);
    }
  };

  return (
    <div className="shrink-0 px-4 pb-4">
      <div className="mx-auto w-full max-w-[760px]">
        {limitReached && !active ? (
          <div
            className="mb-2 flex items-center justify-between gap-3 rounded-lg border px-3 py-2 text-[12px]"
            style={{ borderColor: "var(--border)", background: "var(--panel)" }}
          >
            <span style={{ color: "var(--muted)" }}>
              Tool step limit reached{step ? ` (${step.max})` : ""}. The model stopped with a summary.
            </span>
            <Button
              size="sm"
              variant="primary"
              onClick={() => void continueTurn()}
            >
              Continue
            </Button>
          </div>
        ) : null}
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
            "rounded-2xl border p-2 transition",
            active && "opacity-95",
          )}
          style={{
            borderColor: "var(--border-strong)",
            background: "var(--panel)",
            boxShadow: "0 8px 28px rgb(0 0 0 / 0.06)",
          }}
        >
          {pendingAttachments.length > 0 ? (
            <div className="px-1 pt-1.5">
              <AttachmentGrid
                attachments={pendingAttachments}
                compact
                onRemove={(id) => void removeAttachment(id)}
              />
            </div>
          ) : null}
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
            className="max-h-[240px] min-h-[38px] w-full resize-none bg-transparent px-2 py-2 text-[14.5px] leading-relaxed outline-none placeholder:text-[var(--faint)] disabled:cursor-not-allowed"
            aria-label="Message"
          />
          <div className="mt-1 flex items-center gap-1.5 px-1">
            <button
              type="button"
              onClick={() => void chooseFiles()}
              disabled={active || disabled || attaching}
              className="focus-ring flex h-8 w-8 shrink-0 items-center justify-center rounded-lg transition hover:bg-[var(--panel-2)] disabled:cursor-not-allowed disabled:opacity-40"
              style={{ color: "var(--muted)" }}
              title="Attach files or images"
              aria-label="Attach files or images"
            >
              {attaching ? (
                <Loader2 size={14} className="animate-spin" />
              ) : (
                <Paperclip size={14} />
              )}
            </button>
            <ModelPicker conversation={conversation} disabled={active} />
            <WorkspacePicker conversation={conversation} disabled={active} />
            <AutoApproveButton />
            <div className="ml-auto flex items-center gap-2">
              {active ? (
                <button
                  onClick={() => void stop()}
                  className="focus-ring flex h-8 w-8 shrink-0 items-center justify-center rounded-xl transition hover:brightness-95"
                  style={{ background: "var(--fg)", color: "var(--bg)" }}
                  title="Stop generating (Esc)"
                  aria-label="Stop generating"
                >
                  <Square size={12} fill="currentColor" />
                </button>
              ) : (
                <button
                  onClick={submit}
                  disabled={!canSend}
                  className="focus-ring flex h-8 w-8 shrink-0 items-center justify-center rounded-xl transition hover:brightness-95 disabled:cursor-not-allowed disabled:opacity-30"
                  style={{ background: "var(--accent)", color: "var(--accent-fg)" }}
                  title="Send (Enter)"
                  aria-label="Send message"
                >
                  <ArrowUp size={16} strokeWidth={2.2} />
                </button>
              )}
            </div>
          </div>
        </div>
        <div
          className="mt-1.5 flex items-center justify-between px-1 text-[10.5px]"
          style={{ color: "var(--faint)" }}
        >
          <span>
            Enter to send · Shift+Enter for a new line · up to 6 files · 5 MB/image · 25 MB total
          </span>
          <span>
            {active && step ? `step ${step.current}/${step.max} · ` : ""}
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
