import { useId, useState } from "react";
import { AlertCircle, ChevronDown, Loader2 } from "lucide-react";
import type { MessageWithParts, Part } from "@/lib/types";
import { parsePayload, partText } from "@/app/store";
import { formatDuration } from "@/lib/format";
import { MessageItem } from "./MessageItem";
import { ToolCard } from "./ToolCard";
import { ThoughtRow } from "./ThoughtRow";
import { thoughtDuration } from "./activity";

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
  // Activity rows are visible by default. The header only hides a long list.
  const [open, setOpen] = useState(true);
  const listId = useId();
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

  const work = flattened.filter(
    (item) =>
      item.part.kind === "reasoning" || item.part.kind === "tool_call",
  );

  const toolCount = flattened.reduce((count, item) => {
    if (item.part.kind !== "tool_call") return count;
    const payload = parsePayload(item.part);
    const name = payload.name;
    if (name === "mcp_search_tools" || name === "mcp_get_tool_schema") {
      return count;
    }
    if (name === "mcp_execute") {
      const args = payload.args as Record<string, unknown> | undefined;
      const calls = args?.calls;
      return count + (Array.isArray(calls) ? calls.length : 1);
    }
    return count + 1;
  }, 0);
  const thoughtCount = flattened.filter(
    (item) => item.part.kind === "reasoning",
  ).length;
  const toolCalls = flattened
    .filter((item) => item.part.kind === "tool_call")
    .map((item) => parsePayload(item.part));
  const failedCount = toolCalls.filter((payload) =>
    ["error", "aborted"].includes(String(payload.state ?? "")),
  ).length;
  const runningTool = toolCalls.find((payload) =>
    ["streaming", "queued", "running"].includes(String(payload.state ?? "")),
  );
  const started = toolCalls
    .map((payload) => Number(payload.startedAt ?? 0))
    .filter((value) => value > 0);
  const ended = toolCalls
    .map((payload) => Number(payload.endedAt ?? 0))
    .filter((value) => Number.isFinite(value) && value > 0);
  const duration =
    started.length > 0 && ended.length > 0
      ? Math.max(0, Math.max(...ended) - Math.min(...started))
      : 0;

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

  const summary =
    toolCount > 0
      ? `Used ${toolCount} tool${toolCount === 1 ? "" : "s"}${
          failedCount > 0 ? ` · ${failedCount} failed` : ""
        }${duration >= 1_000 ? ` · ${formatDuration(duration)}` : ""}`
      : thoughtCount > 0
        ? `${thoughtCount} thought${thoughtCount === 1 ? "" : "s"}`
        : "Activity";
  const streamingMessage = [...messages]
    .reverse()
    .find((message) => message.status === "streaming");
  const activeKind = streamingMessage?.parts.at(-1)?.kind;
  const thinking =
    Boolean(streamingMessage) && (!activeKind || activeKind === "reasoning");

  return (
    <div className="animate-in py-(--turn-py)">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        className="focus-ring mb-0.5 flex w-full items-center gap-2 rounded-md px-2 py-(--row-py) text-left transition-colors hover:bg-[var(--panel-2)]"
        style={{ color: "var(--muted)", fontSize: "var(--chat-fs)" }}
      >
        <span className="font-medium">{summary}</span>
        {thinking ? (
          <span className="flex items-center gap-1 text-[length:var(--fs-meta)]">
            <Loader2
              size={12}
              className="animate-spin"
              style={{ color: "var(--warn)" }}
            />
            Thinking…
          </span>
        ) : runningTool ? (
          <span className="flex min-w-0 items-center gap-1 text-[length:var(--fs-meta)]">
            <Loader2
              size={12}
              className="animate-spin"
              style={{ color: "var(--warn)" }}
            />
            <span className="max-w-[180px] truncate">
              Running {String(runningTool.name ?? "tool")}
            </span>
          </span>
        ) : running ? (
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
          className="ml-auto flex items-center gap-1 text-[length:var(--fs-meta)]"
          style={{ color: "var(--faint)" }}
        >
          {open ? "Hide" : "Show"}
          <ChevronDown
            size={12}
            aria-hidden
            className={open ? "transition" : "-rotate-90 transition"}
          />
        </span>
      </button>

      {open ? (
        <div id={listId} className="mb-2">
          {work.map((item) =>
            item.part.kind === "reasoning" ? (
              <ThoughtRow
                key={item.part.id}
                text={partText(item.part)}
                duration={thoughtDuration(parsePayload(item.part))}
                streaming={item.message.status === "streaming"}
              />
            ) : (
              <ToolCard key={item.part.id} part={item.part} />
            ),
          )}
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
