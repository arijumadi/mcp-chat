import { useState } from "react";
import {
  AlertCircle,
  ChevronDown,
  Loader2,
  Sparkles,
} from "lucide-react";
import type { MessageWithParts, Part } from "@/lib/types";
import { parsePayload, partText } from "@/app/store";
import { Markdown } from "@/components/Markdown";
import { MessageItem } from "./MessageItem";
import { ToolCard } from "./ToolCard";

interface Flattened {
  part: Part;
  message: MessageWithParts;
  index: number;
}

export function AssistantTurn({
  messages,
  canRetry,
}: {
  messages: MessageWithParts[];
  canRetry: boolean;
}) {
  const [open, setOpen] = useState(false);
  const last = messages[messages.length - 1];
  if (!last) return null;

  const flattened: Flattened[] = messages.flatMap((message) =>
    message.parts.map((part) => ({ part, message, index: 0 })),
  );
  flattened.forEach((item, index) => {
    item.index = index;
  });

  const lastTool = flattened.reduce(
    (found, item, index) => (item.part.kind === "tool_call" ? index : found),
    -1,
  );

  const finalParts = flattened
    .filter((item) => {
      if (item.part.kind === "error") return true;
      if (item.part.kind !== "text") return false;
      return lastTool === -1 || item.index > lastTool;
    })
    .map((item) => item.part);

  const work = flattened.filter((item) => {
    if (item.part.kind === "reasoning" || item.part.kind === "tool_call") {
      return true;
    }
    return (
      item.part.kind === "text" && lastTool !== -1 && item.index < lastTool
    );
  });

  const toolCount = flattened.filter(
    (item) => item.part.kind === "tool_call",
  ).length;
  const thoughtCount = flattened.filter(
    (item) => item.part.kind === "reasoning",
  ).length;

  const running =
    messages.some((message) => message.status === "streaming") ||
    flattened.some((item) => {
      if (item.part.kind !== "tool_call") return false;
      const payload = parsePayload(item.part);
      return (
        payload.approval === "pending" ||
        ["streaming", "queued", "running"].includes(String(payload.state ?? ""))
      );
    });

  const hasError =
    messages.some((message) => message.status === "error") ||
    flattened.some((item) => {
      if (item.part.kind !== "tool_call") return false;
      return ["error", "aborted"].includes(
        String(parsePayload(item.part).state ?? ""),
      );
    });

  const summaryParts: string[] = [];
  if (toolCount > 0) {
    summaryParts.push(`Used ${toolCount} tool${toolCount === 1 ? "" : "s"}`);
  }
  if (thoughtCount > 0) {
    summaryParts.push(`${thoughtCount} thought${thoughtCount === 1 ? "" : "s"}`);
  }
  const summary = summaryParts.join(" · ") || "Activity";

  return (
    <div className="animate-in py-1">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        className="focus-ring my-1.5 flex w-full items-center gap-2 rounded-xl border px-3 py-2 text-left text-[12.5px] transition hover:bg-[var(--panel-2)]"
        style={{ borderColor: "var(--border)", background: "var(--panel)" }}
      >
        <Sparkles size={13} style={{ color: "var(--accent)" }} />
        <span className="font-medium">{summary}</span>
        {running ? (
          <Loader2
            size={12}
            className="animate-spin"
            style={{ color: "var(--warn)" }}
          />
        ) : null}
        {hasError ? (
          <AlertCircle size={12} style={{ color: "var(--danger)" }} />
        ) : null}
        <span
          className="ml-auto flex items-center gap-1 text-[10.5px]"
          style={{ color: "var(--faint)" }}
        >
          {open ? "Hide details" : "Show details"}
          <ChevronDown
            size={12}
            className={open ? "rotate-180 transition" : "transition"}
          />
        </span>
      </button>

      {open ? (
        <div
          className="mb-2 ml-2 border-l pl-3"
          style={{ borderColor: "var(--border)" }}
        >
          {work.map((item, index) => {
            if (item.part.kind === "reasoning") {
              const thought =
                work
                  .slice(0, index + 1)
                  .filter((entry) => entry.part.kind === "reasoning").length;
              return (
                <div
                  key={item.part.id}
                  className="my-1.5 rounded-lg border px-3 py-2"
                  style={{
                    borderColor: "var(--border)",
                    background: "var(--panel)",
                  }}
                >
                  <div
                    className="mb-1 text-[10px] tracking-[0.06em] uppercase"
                    style={{ color: "var(--faint)" }}
                  >
                    Thought {thought}
                  </div>
                  <div
                    className="max-h-56 overflow-y-auto text-[12.5px] whitespace-pre-wrap italic"
                    style={{ color: "var(--muted)" }}
                  >
                    {partText(item.part)}
                  </div>
                </div>
              );
            }
            if (item.part.kind === "text") {
              return (
                <div
                  key={item.part.id}
                  className="my-1.5 rounded-lg border px-3 py-2"
                  style={{
                    borderColor: "var(--border)",
                    background: "var(--panel)",
                  }}
                >
                  <div
                    className="mb-1 text-[10px] tracking-[0.06em] uppercase"
                    style={{ color: "var(--faint)" }}
                  >
                    Interim note
                  </div>
                  <div className="text-[13px]" style={{ color: "var(--muted)" }}>
                    <Markdown text={partText(item.part)} />
                  </div>
                </div>
              );
            }
            return <ToolCard key={item.part.id} part={item.part} />;
          })}
        </div>
      ) : null}

      {finalParts.length > 0 || last.status === "error" ? (
        <MessageItem
          message={last}
          partsOverride={finalParts}
          canRetry={canRetry}
          hideThinking
        />
      ) : null}
    </div>
  );
}
