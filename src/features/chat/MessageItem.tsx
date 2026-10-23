import { AlertTriangle, RotateCcw } from "lucide-react";
import type { MessageWithParts, Part } from "@/lib/types";
import { parsePayload, partText, useApp } from "@/app/store";
import { Markdown } from "@/components/Markdown";
import { Button } from "@/components/Button";
import { ToolCard } from "./ToolCard";
import { cn } from "@/lib/cn";

export function MessageItem({
  message,
  canRetry,
}: {
  message: MessageWithParts;
  canRetry: boolean;
}) {
  const regenerate = useApp((state) => state.regenerate);
  if (message.role === "tool") return null;

  if (message.role === "user") {
    const text = message.parts.map(partText).join("");
    return (
      <div className="animate-in flex justify-end py-2">
        <div
          className="max-w-[85%] rounded-2xl rounded-br-md px-4 py-2.5 text-[14.5px] whitespace-pre-wrap"
          style={{ background: "var(--user)" }}
        >
          {text}
        </div>
      </div>
    );
  }

  const streaming = message.status === "streaming";
  const textParts = message.parts.filter((part) => part.kind === "text");
  const lastTextId = textParts.at(-1)?.id ?? null;
  const hasVisible = message.parts.some((part) =>
    ["text", "reasoning", "tool_call", "error", "summary"].includes(part.kind),
  );

  return (
    <div className="animate-in py-2">
      {!hasVisible && streaming ? <Thinking /> : null}
      {message.parts.map((part) => {
        if (part.kind === "text") {
          const text = partText(part);
          if (!text) return null;
          return (
            <div key={part.id} className="relative">
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
      {message.status === "error" && !message.parts.some((part) => part.kind === "error") ? (
        <div className="mt-1 text-[12px]" style={{ color: "var(--danger)" }}>
          {message.error ?? "The turn failed."}
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
