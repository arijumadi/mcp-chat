import { useRef } from "react";
import { AlertTriangle, RotateCcw, Undo2 } from "lucide-react";
import type { MessageWithParts, Part } from "@/lib/types";
import { parsePayload, partText, useApp } from "@/app/store";
import { Markdown } from "@/components/Markdown";
import { Button } from "@/components/Button";
import { ToolCard } from "./ToolCard";
import { cn } from "@/lib/cn";
import { formatMessageDateTime } from "@/lib/format";
import { CopyButton } from "./CopyButton";

export function MessageItem({
  message,
  canRetry,
  partsOverride,
  hideThinking,
}: {
  message: MessageWithParts;
  canRetry: boolean;
  partsOverride?: Part[];
  hideThinking?: boolean;
}) {
  const regenerate = useApp((state) => state.regenerate);
  const revertMessage = useApp((state) => state.revertMessage);
  const status = useApp((state) => state.status);
  const busy = ["thinking", "streaming", "waiting_approval", "running_tool"].includes(
    status,
  );
  const copyRoot = useRef<HTMLDivElement>(null);
  if (message.role === "tool") return null;

  if (message.role === "user") {
    const text = (partsOverride ?? message.parts)
      .filter((part) => part.kind === "text")
      .map(partText)
      .join("");
    return (
      <div className="animate-in flex justify-end py-2">
        <div
          className="group flex max-w-[85%] flex-col items-end"
          data-testid="user-message"
          data-message-id={message.id}
        >
          <div
            className="max-w-full rounded-2xl rounded-br-md px-4 py-2.5 text-[14.5px] whitespace-pre-wrap"
            style={{ background: "var(--user)" }}
          >
            {text}
          </div>
          <div className="mt-0.5 flex h-6 items-center justify-end gap-0.5 opacity-0 transition group-hover:opacity-100 focus-within:opacity-100">
            <span
              className="mr-1 text-[10.5px] tabular-nums"
              style={{ color: "var(--muted)" }}
            >
              {formatMessageDateTime(message.createdAt)}
            </span>
            <CopyButton payload={() => ({ text })} />
            {!busy ? (
              <button
                type="button"
                onClick={() => void revertMessage(message.id)}
                className="focus-ring flex h-6 w-6 items-center justify-center rounded-md transition hover:bg-[var(--panel-2)] hover:text-[var(--fg)]"
                style={{ color: "var(--muted)" }}
                title="Revert to this message"
                aria-label="Revert message"
              >
                <Undo2 size={12} />
              </button>
            ) : null}
          </div>
        </div>
      </div>
    );
  }

  const parts = partsOverride ?? message.parts;
  const streaming = message.status === "streaming";
  const textParts = parts.filter((part) => part.kind === "text");
  const copyText = textParts
    .map(partText)
    .join("\n\n")
    .trim();
  const copyPayload = () => {
    const nodes = copyRoot.current?.querySelectorAll<HTMLElement>(
      "[data-markdown-copy]",
    );
    if (!nodes || nodes.length === 0) return { text: copyText };
    const holder = document.createElement("div");
    holder.style.position = "fixed";
    holder.style.left = "-10000px";
    holder.style.top = "0";
    holder.style.width = "720px";
    holder.style.pointerEvents = "none";
    document.body.appendChild(holder);
    const htmlParts: string[] = [];
    try {
      for (const node of nodes) {
        const clone = node.cloneNode(true) as HTMLElement;
        clone
          .querySelectorAll("button, .stream-cursor")
          .forEach((element) => element.remove());
        htmlParts.push(clone.innerHTML);
        holder.appendChild(clone);
      }
      const plain = (holder.innerText || holder.textContent || "").trim();
      return {
        text: plain || copyText,
        html: htmlParts.join("\n"),
      };
    } finally {
      holder.remove();
    }
  };
  const lastTextId = textParts.at(-1)?.id ?? null;
  const hasVisible = parts.some((part) =>
    ["text", "reasoning", "tool_call", "error", "summary"].includes(part.kind),
  );

  return (
    <div ref={copyRoot} className="group animate-in py-2">
      {!hasVisible && streaming && !hideThinking ? <Thinking /> : null}
      {parts.map((part) => {
        if (part.kind === "text") {
          const text = partText(part);
          if (!text) return null;
          return (
            <div key={part.id} className="relative" data-markdown-copy>
              <Markdown text={text} />
              {streaming && part.id === lastTextId ? (
                <span className="stream-cursor" aria-hidden />
              ) : null}
            </div>
          );
        }
        if (part.kind === "reasoning") {
          return <Reasoning key={part.id} part={part} />;
        }
        if (part.kind === "tool_call") {
          return <ToolCard key={part.id} part={part} />;
        }
        if (part.kind === "error") {
          const payload = parsePayload(part);
          const text = String(payload.message ?? message.error ?? "Something went wrong.");
          return (
            <div
              key={part.id}
              className="my-2.5 rounded-xl border px-3.5 py-3 text-[13px]"
              style={{
                borderColor: "color-mix(in srgb, var(--danger) 40%, var(--border))",
                background: "var(--danger-soft)",
                color: "var(--fg)",
              }}
            >
              <div className="flex items-start gap-2">
                <AlertTriangle
                  size={15}
                  className="mt-0.5 shrink-0"
                  style={{ color: "var(--danger)" }}
                />
                <div className="min-w-0 flex-1">
                  <div className="font-medium">The turn failed</div>
                  <div className="mt-0.5 whitespace-pre-wrap" style={{ color: "var(--muted)" }}>
                    {text}
                  </div>
                  {canRetry ? (
                    <div className="mt-2">
                      <Button
                        size="sm"
                        variant="secondary"
                        icon={<RotateCcw size={12} />}
                        onClick={() => void regenerate()}
                      >
                        Retry
                      </Button>
                    </div>
                  ) : null}
                </div>
              </div>
            </div>
          );
        }
        if (part.kind === "summary") {
          const payload = parsePayload(part);
          return (
            <details
              key={part.id}
              className="my-2 rounded-lg border px-3 py-2 text-[12.5px]"
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <summary className="cursor-pointer select-none" style={{ color: "var(--muted)" }}>
                Earlier conversation summary
              </summary>
              <div className="mt-2 whitespace-pre-wrap" style={{ color: "var(--muted)" }}>
                {String(payload.text ?? "")}
              </div>
            </details>
          );
        }
        return null;
      })}

      {message.status === "interrupted" ? (
        <div className="mt-1 text-[11.5px]" style={{ color: "var(--faint)" }}>
          Interrupted — the partial answer was kept.
        </div>
      ) : null}
      {message.status === "error" && !parts.some((part) => part.kind === "error") ? (
        <div className="mt-1 text-[12px]" style={{ color: "var(--danger)" }}>
          {message.error ?? "The turn failed."}
        </div>
      ) : null}
      {copyText ? (
        <div className="mt-1 flex h-6 items-center opacity-0 transition group-hover:opacity-100 focus-within:opacity-100">
          <CopyButton payload={copyPayload} />
        </div>
      ) : null}
    </div>
  );
}

function Thinking() {
  return (
    <div
      className="flex items-center gap-2 py-1 text-[13px]"
      style={{ color: "var(--muted)" }}
    >
      <span className="pulse-dot inline-block h-1.5 w-1.5 rounded-full bg-[var(--accent)]" />
      <span className="pulse-dot inline-block h-1.5 w-1.5 rounded-full bg-[var(--accent)] [animation-delay:150ms]" />
      <span className="pulse-dot inline-block h-1.5 w-1.5 rounded-full bg-[var(--accent)] [animation-delay:300ms]" />
      <span className="ml-1">Thinking…</span>
    </div>
  );
}

function Reasoning({ part }: { part: Part }) {
  const payload = parsePayload(part);
  const text = String(payload.text ?? "");
  if (!text) return null;
  return (
    <details
      className={cn("my-2 rounded-lg border px-3 py-2 text-[12.5px]")}
      style={{ borderColor: "var(--border)", background: "var(--panel)" }}
    >
      <summary className="cursor-pointer select-none" style={{ color: "var(--muted)" }}>
        Reasoning
      </summary>
      <div
        className="mt-2 whitespace-pre-wrap italic"
        style={{ color: "var(--muted)" }}
      >
        {text}
      </div>
    </details>
  );
}
