import { useEffect, useRef, useState } from "react";
import { ArrowUp, Loader2, Paperclip, Square, Upload } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
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
  const steer = useApp((state) => state.steerMessage);
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
  const [steering, setSteering] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [dragCount, setDragCount] = useState(0);
  const active = ACTIVE.includes(status);
  const disabled = providers.length === 0;
  const hasContent = value.trim().length > 0 || pendingAttachments.length > 0;
  const canSend = hasContent && !active && !disabled;
  const canSteer = hasContent && active && !disabled && !steering;
  const showDrop = dragging && !active && !disabled;

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

  useEffect(() => {
    if (active || disabled) {
      setDragging(false);
      setDragCount(0);
    }
  }, [active, disabled]);

  useEffect(() => {
    // Browser fallback (and the Playwright harness): do not let a file drop
    // navigate away from the app. Native Tauri drops are handled below.
    if (window.__MCP_CHAT_MOCK__ || !("__TAURI_INTERNALS__" in window)) {
      const hasFiles = (event: DragEvent) =>
        Array.from(event.dataTransfer?.types ?? []).includes("Files");
      const onDragOver = (event: DragEvent) => {
        if (hasFiles(event)) event.preventDefault();
      };
      const onDrop = (event: DragEvent) => {
        if (!hasFiles(event)) return;
        event.preventDefault();
        useApp
          .getState()
          .toast("Drag and drop is available in the desktop app.", "info");
      };
      window.addEventListener("dragover", onDragOver);
      window.addEventListener("drop", onDrop);
      return () => {
        window.removeEventListener("dragover", onDragOver);
        window.removeEventListener("drop", onDrop);
      };
    }
    if (active || disabled) return;

    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter") {
          setDragCount(event.payload.paths.length);
          setDragging(true);
        } else if (event.payload.type === "over") {
          setDragging(true);
        } else if (event.payload.type === "leave") {
          setDragging(false);
          setDragCount(0);
        } else if (event.payload.type === "drop") {
          setDragging(false);
          setDragCount(0);
          if (event.payload.paths.length === 0) return;
          setAttaching(true);
          void stageAttachments(event.payload.paths).finally(() => {
            setAttaching(false);
            ref.current?.focus();
          });
        }
      })
      .then((stop) => {
        if (cancelled) {
          stop();
        } else {
          unlisten = stop;
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [active, disabled, stageAttachments]);

  const submit = () => {
    const text = value.trim();
    if (disabled || steering) return;
    if (active) {
      if (!hasContent) return;
      setValue("");
      setSteering(true);
      void steer(text).finally(() => setSteering(false));
      return;
    }
    if (!hasContent) return;
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
      {showDrop ? (
        <div
          className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center p-6"
          style={{
            background: "color-mix(in srgb, var(--bg) 78%, transparent)",
            backdropFilter: "blur(3px)",
          }}
          role="status"
          aria-live="polite"
        >
          <div
            className="w-full max-w-md rounded-2xl border-2 border-dashed px-8 py-10 text-center shadow-xl"
            style={{
              borderColor: "var(--accent)",
              background: "var(--panel)",
            }}
          >
            <span
              className="mx-auto flex h-12 w-12 items-center justify-center rounded-2xl"
              style={{ background: "var(--accent-soft)", color: "var(--accent)" }}
            >
              <Upload size={22} />
            </span>
            <div className="mt-4 text-[15px] font-semibold">
              Drop {dragCount > 0 ? `${dragCount} file${dragCount === 1 ? "" : "s"}` : "files"} to attach
            </div>
            <div className="mt-1 text-[12px]" style={{ color: "var(--muted)" }}>
              Images, text and PDFs · up to 6 files · 25 MB total
            </div>
          </div>
        </div>
      ) : null}
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
              title="Attach files or images — or drop them anywhere"
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
                <>
                  {hasContent || steering ? (
                    <button
                      onClick={submit}
                      disabled={!canSteer}
                      className="focus-ring flex h-8 w-8 shrink-0 items-center justify-center rounded-xl transition hover:brightness-95 disabled:cursor-not-allowed disabled:opacity-40"
                      style={{ background: "var(--accent)", color: "var(--accent-fg)" }}
                      title="Steer: stop the current turn and send this message"
                      aria-label="Steer message"
                    >
                      {steering ? (
                        <Loader2 size={15} className="animate-spin" />
                      ) : (
                        <ArrowUp size={16} strokeWidth={2.2} />
                      )}
                    </button>
                  ) : null}
                  <button
                    onClick={() => void stop()}
                    className="focus-ring flex h-8 w-8 shrink-0 items-center justify-center rounded-xl transition hover:brightness-95"
                    style={{ background: "var(--fg)", color: "var(--bg)" }}
                    title="Stop generating (Esc)"
                    aria-label="Stop generating"
                  >
                    <Square size={12} fill="currentColor" />
                  </button>
                </>
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
            {steering
              ? "Steering… stopping the current turn"
              : "Enter to send · Shift+Enter newline · drag/drop or attach files · max 6 / 25 MB"}
          </span>
          <span>
            {active && step
              ? step.max > 0
                ? `step ${step.current}/${step.max} · `
                : `step ${step.current} · `
              : ""}
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
