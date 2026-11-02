import { useId, useState } from "react";
import { Brain } from "lucide-react";
import { ActivityRow } from "./ActivityRow";

/**
 * A collapsed-by-default thought. The collapsed row shows only "Thought" and,
 * when known, its duration. The reasoning text appears only after expanding.
 */
export function ThoughtRow({ text, duration }: { text: string; duration?: string }) {
  const [open, setOpen] = useState(false);
  const bodyId = useId();
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
          className="mt-0.5 mb-1.5 ml-[15px] max-h-56 overflow-y-auto border-l pl-4 text-[12.5px] whitespace-pre-wrap italic"
          style={{ borderColor: "var(--border)", color: "var(--muted)" }}
        >
          {text}
        </div>
      ) : null}
    </div>
  );
}
