import { useId, useLayoutEffect, useRef, useState } from "react";
import { Brain } from "lucide-react";
import { ActivityRow } from "./ActivityRow";

/** How close to the bottom still counts as "at the bottom". */
const FOLLOW_THRESHOLD_PX = 24;

/**
 * A collapsed-by-default thought. The collapsed row shows only "Thought" and,
 * when known, its duration. The reasoning text appears only after expanding.
 *
 * While the thought is streaming and expanded, its body follows the newest
 * text. Scrolling up inside the body stops that, and scrolling back near the
 * bottom resumes it. Only the thought body scrolls, never the transcript.
 *
 * Following is decided from geometry on every update, not from scroll events.
 * `scrollTop` is always current, but a scroll event is delivered a frame later,
 * so a text update landing in between would otherwise drag the user back down.
 * The question asked on each update is: before this text arrived, was the view
 * at the bottom?
 */
export function ThoughtRow({
  text,
  duration,
  streaming = false,
}: {
  text: string;
  duration?: string;
  streaming?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  const bodyRef = useRef<HTMLDivElement>(null);
  const wasOpen = useRef(false);
  // Content height as of the previous update, before the new text was added.
  const previousHeight = useRef(0);
  const following = useRef(false);

  useLayoutEffect(() => {
    const justOpened = open && !wasOpen.current;
    wasOpen.current = open;
    const body = bodyRef.current;
    if (!open || !body) return;
    if (justOpened) {
      // Every open starts over: a live thought opens at its tail and follows
      // it, a finished one opens at the top so it reads from the start.
      following.current = streaming;
    } else if (streaming) {
      following.current =
        previousHeight.current - body.scrollTop - body.clientHeight <= FOLLOW_THRESHOLD_PX;
    }
    if (streaming && following.current) body.scrollTop = body.scrollHeight;
    previousHeight.current = body.scrollHeight;
  }, [open, streaming, text]);

  return (
    <div>
      <ActivityRow
        icon={<Brain size={13} style={{ color: "var(--faint)" }} aria-hidden />}
        expanded={open}
        onToggle={() => setOpen((value) => !value)}
        controlsId={bodyId}
      >
        <span className="shrink-0 italic" style={{ color: "var(--muted)" }}>
          Thought
        </span>
        {duration ? (
          <span className="shrink-0 tabular-nums" style={{ color: "var(--faint)" }}>
            {duration}
          </span>
        ) : null}
      </ActivityRow>
      {open ? (
        <div
          id={bodyId}
          ref={bodyRef}
          role="region"
          aria-label="Thought text"
          tabIndex={0}
          className="mt-0.5 mb-1.5 ml-[15px] max-h-56 overflow-y-auto border-l pl-4 text-[12.5px] whitespace-pre-wrap italic"
          style={{ borderColor: "var(--border)", color: "var(--muted)" }}
        >
          {text}
        </div>
      ) : null}
    </div>
  );
}
