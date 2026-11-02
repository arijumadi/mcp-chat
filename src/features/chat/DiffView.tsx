import { useId, useMemo, useState } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "@/lib/cn";
import { diffStats, parseDiff, type DiffLineKind } from "./diff";

const lineClass: Record<DiffLineKind, string> = {
  file: "diff-line diff-file-header",
  hunk: "diff-line diff-hunk",
  added: "diff-line diff-line-added",
  removed: "diff-line diff-line-removed",
  context: "diff-line diff-line-context",
};

/**
 * Collapsible unified diff. Lines are colored by a plain prefix parser; the
 * +, - and @@ prefixes stay in the text so color is never the only cue.
 */
export function DiffView({
  diff,
  defaultOpen = false,
  className,
}: {
  diff: string;
  defaultOpen?: boolean;
  className?: string;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const bodyId = useId();
  const lines = useMemo(() => parseDiff(diff), [diff]);
  const stats = useMemo(() => diffStats(lines), [lines]);
  if (lines.length === 0) return null;

  return (
    <div className={cn("diff-view", className)}>
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-controls={open ? bodyId : undefined}
        className="diff-summary focus-ring"
      >
        <ChevronRight
          size={12}
          aria-hidden
          className={cn("shrink-0 transition-transform", open && "rotate-90")}
        />
        <span className="font-medium">Changes</span>{" "}
        <span className="diff-stats">
          · <span className="diff-stat-added">+{stats.added}</span>{" "}
          <span className="diff-stat-removed">{"\u2212"}{stats.removed}</span>
          {stats.files > 0 ? (
            <>
              {" "}
              · {stats.files} {stats.files === 1 ? "file" : "files"}
            </>
          ) : null}
          {stats.truncated ? (
            <>
              {" "}
              · truncated
            </>
          ) : null}
        </span>
      </button>
      {open ? (
        <div id={bodyId} className="diff-body" role="region" aria-label="Changes" tabIndex={0}>
          {lines.map((line, index) => (
            <div key={index} className={lineClass[line.kind]}>
              {line.text === "" ? "\u00a0" : line.text}
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
