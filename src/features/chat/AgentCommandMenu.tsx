import { useEffect, useMemo, useState } from "react";
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

  // Only offer suggestions while the user is still typing the name.
  const query = useMemo(() => {
    const trimmed = draft.trimStart();
    if (!trimmed.startsWith("/")) return null;
    const body = trimmed.slice(1);
    if (body.includes(" ") || body.includes("\n")) return null;
    return body.toLowerCase();
  }, [draft]);

  const matches = useMemo(() => {
    if (query === null) return [];
    return commands.filter((command) => command.name.toLowerCase().startsWith(query));
  }, [commands, query]);

  useEffect(() => {
    setActive(0);
  }, [query, commands.length]);

  if (matches.length === 0) return null;

  return (
    <div
      role="listbox"
      aria-label="Agent commands"
      className="mb-1 overflow-hidden rounded-xl border"
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
          <span className="min-w-0">
            <span className="font-mono text-[11.5px]">/{command.name}</span>
            {command.inputHint ? (
              <span className="ml-1.5 font-mono text-[10.5px]" style={{ color: "var(--faint)" }}>
                {command.inputHint}
              </span>
            ) : null}
            {command.description ? (
              <span className="ml-2 text-[10.5px]" style={{ color: "var(--muted)" }}>
                {command.description}
              </span>
            ) : null}
          </span>
        </button>
      ))}
      <div
        className="border-t px-2.5 py-1 text-[10px]"
        style={{ borderColor: "var(--border)", color: "var(--faint)" }}
      >
        Sent to the external agent, which runs the command itself.
      </div>
    </div>
  );
}
