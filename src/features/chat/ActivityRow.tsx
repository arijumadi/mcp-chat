import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "@/lib/cn";

/**
 * One compact, single-line activity row (a tool call or a thought). The whole
 * row is the toggle. Callers render the detail body themselves when expanded.
 */
export function ActivityRow({
  icon,
  expanded,
  onToggle,
  controlsId,
  title,
  trailing,
  children,
  expandable = true,
}: {
  icon: ReactNode;
  expanded: boolean;
  onToggle: () => void;
  controlsId?: string;
  title?: string;
  trailing?: ReactNode;
  children: ReactNode;
  expandable?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={() => {
        if (expandable) onToggle();
      }}
      aria-expanded={expandable ? expanded : undefined}
      aria-controls={expandable && expanded ? controlsId : undefined}
      title={title}
      // The global button reset beats size utilities; follow the chat size explicitly.
      style={{ fontSize: "var(--chat-fs)" }}
      className="focus-ring flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-(--row-py) text-left leading-5 transition-colors hover:bg-[var(--panel-2)]"
    >
      <span className="flex w-4 shrink-0 items-center justify-center">{icon}</span>
      {children}
      {trailing ? (
        <span
          className="ml-auto flex shrink-0 items-center gap-1.5 pl-1 text-[length:var(--fs-meta)] tabular-nums"
          style={{ color: "var(--faint)" }}
        >
          {trailing}
        </span>
      ) : (
        <span className="ml-auto" />
      )}
      {expandable ? (
        <ChevronRight
          size={12}
          aria-hidden
          className={cn("shrink-0 transition-transform", expanded && "rotate-90")}
          style={{ color: "var(--faint)" }}
        />
      ) : null}
    </button>
  );
}
