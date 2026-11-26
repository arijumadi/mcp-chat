import { useEffect, useRef, useState } from "react";
import { Terminal } from "lucide-react";
import type { AgentCommand } from "@/lib/types";

/**
 * Slash command suggestions for an external ACP agent.
 *
 * The agent executes these itself; the menu only spares the user from
 * memorising which commands a given agent supports. Everything typed after the
 * command name is forwarded untouched, in the same message.
 */
export function AgentCommandMenu({
  draft,
  commands,
  onPick,
}: {
  draft: string;
  commands: AgentCommand[];
  onPick: (command: AgentCommand) => void;
}) {
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  // Only offer suggestions while the user is still typing the name.
  const query = (() => {
    const trimmed = draft.trimStart();
    if (!trimmed.startsWith("/")) return null;
    const body = trimmed.slice(1);
    if (body.includes(" ") || body.includes("\n")) return null;
    return body.toLowerCase();
  })();

  const matches =
    query === null
      ? []
      : commands.filter((command) => command.name.toLowerCase().startsWith(query));

  useEffect(() => {
    setActive(0);
  }, [query, commands.length]);

  // Keep the highlighted row in view: agents can advertise 20+ commands and
  // the list is taller than the menu.
  useEffect(() => {
    const selected = listRef.current?.querySelector('[aria-selected="true"]');
    // jsdom (tests) has no layout, so the method is absent there.
    if (selected instanceof HTMLElement && typeof selected.scrollIntoView === "function") {
      selected.scrollIntoView({ block: "nearest" });
    }
  }, [active]);

  if (matches.length === 0) return null;

  return (
    // Anchored above the composer instead of flowing in the scroll area: a
    // static list used to push the textarea down and fill the screen.
    <div
      ref={listRef}
      role="listbox"
      aria-label="Agent commands"
      onKeyDown={(event) => {
        if (event.key === "ArrowDown") {
          event.preventDefault();
          setActive((index) => (index + 1) % matches.length);
        } else if (event.key === "ArrowUp") {
          event.preventDefault();
          setActive((index) => (index - 1 + matches.length) % matches.length);
        } else if (event.key === "Enter" || event.key === "Tab") {
          // `active` is always in range, but the compiler cannot know that.
          const chosen = matches[active];
          if (!chosen) return;
          event.preventDefault();
          onPick(chosen);
        }
      }}
      tabIndex={-1}
      className="absolute bottom-full left-0 z-30 mb-1 max-h-[min(50vh,320px)] w-full overflow-y-auto overscroll-contain rounded-xl border shadow-xl"
      style={{ borderColor: "var(--border)", background: "var(--panel-2)" }}
    >
      {matches.map((command, index) => (
        <button
          key={command.name}
          type="button"
          role="option"
          aria-selected={index === active}
          onMouseEnter={() => setActive(index)}
          // `onMouseDown` beats the textarea losing focus, so the click is not
          // swallowed by a blur handler.
          onMouseDown={(event) => {
            event.preventDefault();
            onPick(command);
          }}
          className="flex w-full items-start gap-2 px-2.5 py-1.5 text-left transition"
          style={{
            background: index === active ? "var(--panel)" : "transparent",
          }}
        >
          <Terminal size={11} className="mt-0.5 shrink-0" style={{ color: "var(--muted)" }} />
          <span className="min-w-0 flex-1">
            <span className="font-mono text-[11.5px]">/{command.name}</span>
            {command.inputHint ? (
              <span className="ml-1.5 font-mono text-[10.5px]" style={{ color: "var(--faint)" }}>
                {command.inputHint}
              </span>
            ) : null}
            {command.description ? (
              <span className="mt-0.5 block text-[10.5px] leading-snug" style={{ color: "var(--muted)" }}>
                {command.description}
              </span>
            ) : null}
          </span>
        </button>
      ))}
      <div
        className="sticky bottom-0 border-t px-2.5 py-1 text-[10px]"
        style={{ borderColor: "var(--border)", background: "var(--panel-2)", color: "var(--faint)" }}
      >
        {matches.length} command{matches.length === 1 ? "" : "s"} · run by the agent itself
      </div>
    </div>
  );
}
