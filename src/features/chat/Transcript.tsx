import { useEffect, useRef, useState } from "react";
import { ArrowDown } from "lucide-react";
import { useApp } from "@/app/store";
import { MessageItem } from "./MessageItem";

export function Transcript() {
  const messages = useApp((state) => state.messages);
  const status = useApp((state) => state.status);
  const activeId = useApp((state) => state.activeId);
  const container = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
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

  const scrollToBottom = (behavior: ScrollBehavior = "smooth") => {
    const element = container.current;
    if (!element) return;
    element.scrollTo({ top: element.scrollHeight, behavior });
    atBottom.current = true;
    setShowJump(false);
  };

  useEffect(() => {
    if (previousConversation.current !== activeId) {
      previousConversation.current = activeId;
      atBottom.current = true;
      requestAnimationFrame(() => scrollToBottom("auto"));
    }
  }, [activeId]);

  useEffect(() => {
    if (atBottom.current) {
      requestAnimationFrame(() => scrollToBottom("auto"));
    }
  }, [messages, status]);

  const onScroll = () => {
    const element = container.current;
    if (!element) return;
    const distance =
      element.scrollHeight - element.scrollTop - element.clientHeight;
    atBottom.current = distance < 90;
    setShowJump(!atBottom.current);
  };

  const hasStreamingAssistant = visible.some(
    (message) => message.role === "assistant" && message.status === "streaming",
  );
  const showThinking =
    (status === "thinking" || status === "streaming") && !hasStreamingAssistant;

  return (
    <div className="relative min-h-0 flex-1">
      <div
        ref={container}
        onScroll={onScroll}
        className="h-full overflow-y-auto"
      >
        <div className="mx-auto w-full max-w-[760px] px-6 pt-8 pb-10">
          {visible.map((message) => (
            <MessageItem
              key={message.id}
              message={message}
              canRetry={
                message.id === lastAssistant?.id &&
                message.status === "error" &&
                status !== "streaming" &&
                status !== "thinking"
              }
            />
          ))}
          {showThinking ? (
            <div className="flex items-center gap-2 py-2 text-[13px]" style={{ color: "var(--muted)" }}>
              <span className="pulse-dot h-1.5 w-1.5 rounded-full bg-[var(--accent)]" />
              <span className="pulse-dot h-1.5 w-1.5 rounded-full bg-[var(--accent)] [animation-delay:150ms]" />
              <span className="pulse-dot h-1.5 w-1.5 rounded-full bg-[var(--accent)] [animation-delay:300ms]" />
              <span className="ml-1">
                {status === "thinking" ? "Thinking…" : "Waiting for the first token…"}
              </span>
            </div>
          ) : null}
          {status === "running_tool" ? (
            <div className="py-1 text-[11.5px]" style={{ color: "var(--faint)" }}>
              A tool is running. You can stop the turn with Esc.
            </div>
          ) : null}
          <div className="h-1" />
        </div>
      </div>
      {showJump ? (
        <button
          onClick={() => scrollToBottom()}
          className="focus-ring absolute right-6 bottom-4 flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-[11.5px] shadow-lg transition hover:bg-[var(--panel-2)]"
          style={{ background: "var(--panel)", borderColor: "var(--border)" }}
        >
          <ArrowDown size={12} /> Latest
        </button>
      ) : null}
    </div>
  );
}
