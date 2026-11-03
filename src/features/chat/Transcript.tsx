import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, Loader2 } from "lucide-react";
import { useApp } from "@/app/store";
import { MessageItem } from "./MessageItem";
import { AssistantTurn } from "./AssistantTurn";

export function Transcript() {
  const messages = useApp((state) => state.messages);
  const messageHasMore = useApp((state) => state.messageHasMore);
  const loadingOlder = useApp((state) => state.loadingOlder);
  const loadOlderMessages = useApp((state) => state.loadOlderMessages);
  const status = useApp((state) => state.status);
  const activeId = useApp((state) => state.activeId);
  const container = useRef<HTMLDivElement>(null);
  const followBottom = useRef(true);
  const loadingEarlier = useRef(false);
  const lastScrollTop = useRef(-1);
  const scrollFrame = useRef<number | null>(null);
  const [showJump, setShowJump] = useState(false);
  const previousConversation = useRef<string | null>(null);

  const visible = messages.filter(
    (message) =>
      message.role !== "tool" &&
      message.status !== "synthetic" &&
      (message.parts.length > 0 || message.status === "streaming"),
  );

  const lastAssistant = [...visible]
    .reverse()
    .find((message) => message.role === "assistant");

  const rows = useMemo(() => {
    type Row =
      | { kind: "user"; message: (typeof visible)[number] }
      | { kind: "system"; message: (typeof visible)[number] }
      | { kind: "assistant"; messages: (typeof visible)[number][] };
    const result: Row[] = [];
    let buffer: (typeof visible)[number][] = [];
    const flush = () => {
      if (buffer.length > 0) {
        result.push({ kind: "assistant", messages: buffer });
        buffer = [];
      }
    };
    for (const message of visible) {
      if (message.role === "user") {
        flush();
        result.push({ kind: "user", message });
      } else if (message.role === "assistant") {
        buffer.push(message);
      } else {
        flush();
        result.push({ kind: "system", message });
      }
    }
    flush();
    return result;
  }, [visible]);

  const cancelScheduledScroll = () => {
    if (scrollFrame.current !== null) {
      cancelAnimationFrame(scrollFrame.current);
      scrollFrame.current = null;
    }
  };

  const scrollToBottom = (behavior: ScrollBehavior = "smooth") => {
    const element = container.current;
    if (!element) return;
    cancelScheduledScroll();
    element.scrollTo({ top: element.scrollHeight, behavior });
    followBottom.current = true;
    setShowJump(false);
  };

  const scheduleScrollToBottom = () => {
    if (!followBottom.current) return;
    cancelScheduledScroll();
    scrollFrame.current = requestAnimationFrame(() => {
      scrollFrame.current = null;
      if (followBottom.current) scrollToBottom("auto");
    });
  };

  useEffect(() => {
    if (previousConversation.current !== activeId) {
      previousConversation.current = activeId;
      lastScrollTop.current = -1;
      loadingEarlier.current = false;
      followBottom.current = true;
      requestAnimationFrame(() => scrollToBottom("auto"));
    }
    // The scroll helpers intentionally read live refs from this render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeId]);

  useEffect(() => {
    scheduleScrollToBottom();
    // Deltas arrive through messages; status changes are used for the loader.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [messages, status]);

  const loadEarlier = () => {
    if (!messageHasMore || loadingOlder || loadingEarlier.current) return;
    const element = container.current;
    if (!element) return;
    const before = element.scrollHeight;
    loadingEarlier.current = true;
    void loadOlderMessages().finally(() => {
      requestAnimationFrame(() => {
        if (element) {
          element.scrollTop += element.scrollHeight - before;
        }
        loadingEarlier.current = false;
      });
    });
  };

  const onScroll = () => {
    const element = container.current;
    if (!element) return;
    const distance =
      element.scrollHeight - element.scrollTop - element.clientHeight;
    const previousTop = lastScrollTop.current;
    const scrolledUp = previousTop >= 0 && element.scrollTop < previousTop - 2;
    lastScrollTop.current = element.scrollTop;
    if (scrolledUp) {
      cancelScheduledScroll();
      followBottom.current = false;
    } else if (distance < 12) {
      followBottom.current = true;
    }
    setShowJump(!followBottom.current);
    if (element.scrollTop < 120 && scrolledUp) {
      loadEarlier();
    }
  };

  const onWheel = (event: React.WheelEvent<HTMLDivElement>) => {
    if (event.deltaY < 0) {
      cancelScheduledScroll();
      followBottom.current = false;
      setShowJump(true);
    }
  };

  const hasStreamingAssistant = visible.some(
    (message) => message.role === "assistant" && message.status === "streaming",
  );
  const showCompacting = status === "compacting";
  const showThinking =
    (status === "thinking" || status === "streaming") && !hasStreamingAssistant;

  return (
    <div className="relative min-h-0 flex-1">
      <div
        ref={container}
        onScroll={onScroll}
        onWheel={onWheel}
        className="h-full overflow-y-auto"
      >
        <div className="mx-auto w-full max-w-(--chat-width) px-6 pt-8 pb-10">
          {messageHasMore || loadingOlder ? (
            <div className="mb-4 flex justify-center">
              {loadingOlder ? (
                <span
                  className="flex items-center gap-1.5 text-[length:var(--fs-mono)]"
                  style={{ color: "var(--muted)" }}
                >
                  <Loader2 size={12} className="animate-spin" />
                  Loading earlier messages…
                </span>
              ) : (
                <button
                  type="button"
                  onClick={loadEarlier}
                  className="focus-ring rounded-full border px-3 py-1.5 text-[length:var(--fs-mono)] transition hover:bg-[var(--panel-2)]"
                  style={{ borderColor: "var(--border)", color: "var(--muted)" }}
                >
                  Load earlier messages
                </button>
              )}
            </div>
          ) : null}
          {rows.map((row) => {
            if (row.kind === "user" || row.kind === "system") {
              return (
                <MessageItem
                  key={row.message.id}
                  message={row.message}
                  canRetry={false}
                />
              );
            }
            const canRetry =
              row.messages.at(-1)?.id === lastAssistant?.id &&
              row.messages.at(-1)?.status === "error" &&
              status !== "streaming" &&
              status !== "thinking";
            const hasWork = row.messages.some((message) =>
              message.parts.some(
                (part) => part.kind === "reasoning" || part.kind === "tool_call",
              ),
            );
            if (hasWork) {
              return (
                <AssistantTurn
                  key={row.messages[0]?.id ?? "turn"}
                  messages={row.messages}
                  canRetry={canRetry}
                />
              );
            }
            return row.messages.map((message) => (
              <MessageItem
                key={message.id}
                message={message}
                canRetry={message.id === lastAssistant?.id && canRetry}
              />
            ));
          })}
          {showCompacting ? (
            <div
              className="flex items-center gap-2 py-(--status-py) text-[length:var(--fs-status)]"
              style={{ color: "var(--muted)" }}
            >
              <Loader2 size={14} className="animate-spin" style={{ color: "var(--warn)" }} />
              <span>Compacting context…</span>
            </div>
          ) : showThinking ? (
            <div className="flex items-center gap-2 py-(--status-py) text-[length:var(--fs-status)]" style={{ color: "var(--muted)" }}>
              <span className="pulse-dot h-1.5 w-1.5 rounded-full bg-[var(--accent)]" />
              <span className="pulse-dot h-1.5 w-1.5 rounded-full bg-[var(--accent)] [animation-delay:150ms]" />
              <span className="pulse-dot h-1.5 w-1.5 rounded-full bg-[var(--accent)] [animation-delay:300ms]" />
              <span className="ml-1">
                {status === "thinking" ? "Thinking…" : "Waiting for the first token…"}
              </span>
            </div>
          ) : null}
          {status === "running_tool" ? (
            <div className="py-1 text-[length:var(--fs-mono)]" style={{ color: "var(--faint)" }}>
              A tool is running. You can stop the turn with Esc.
            </div>
          ) : null}
          <div className="h-1" />
        </div>
      </div>
      {showJump ? (
        <button
          onClick={() => scrollToBottom()}
          className="focus-ring absolute right-6 bottom-4 flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-[length:var(--fs-mono)] shadow-lg transition hover:bg-[var(--panel-2)]"
          style={{ background: "var(--panel)", borderColor: "var(--border)" }}
        >
          <ArrowDown size={12} /> Latest
        </button>
      ) : null}
    </div>
  );
}
