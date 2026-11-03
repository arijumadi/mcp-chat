import { useEffect, useState } from "react";
import { Minimize2 } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass } from "@/components/ui";
import { call } from "@/lib/ipc";
import { formatTokens } from "@/lib/format";
import type { ContextStatus } from "@/lib/types";
import { parseCompactCommand } from "./slashCommands";

/**
 * Confirms a manual compaction. It is not reversible, so it always asks, from
 * the `/compact` command, the command palette and the keyboard shortcut alike.
 */
export function CompactDialog() {
  const dialog = useApp((state) => state.compactDialog);
  const activeId = useApp((state) => state.activeId);
  const close = useApp((state) => state.closeCompact);
  const compact = useApp((state) => state.compactConversation);
  const draft = useApp((state) => state.draft);
  const setDraft = useApp((state) => state.setDraft);
  const [focus, setFocus] = useState("");
  const [context, setContext] = useState<ContextStatus | null>(null);
  const open = dialog !== null;

  useEffect(() => {
    if (!dialog) return;
    setFocus(dialog.focus);
    setContext(null);
    if (!activeId) return;
    let cancelled = false;
    // The estimate is informative only: the dialog works without it.
    call<ContextStatus>("context_status", { conversationId: activeId })
      .then((status) => {
        if (!cancelled) setContext(status);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
    // Reset only when the dialog opens, not on every keystroke in `focus`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, activeId]);

  if (!dialog) return null;

  const confirm = () => {
    // A typed `/compact ...` is consumed by the command; other drafts stay.
    if (parseCompactCommand(draft)) setDraft("");
    void compact(focus);
  };

  return (
    <Modal
      open
      onClose={close}
      width="sm"
      title={
        <span className="flex items-center gap-2">
          <Minimize2 size={15} aria-hidden /> Compact this conversation?
        </span>
      }
      subtitle="Older messages are summarized to free up context."
      footer={
        <>
          <Button size="sm" variant="ghost" onClick={close}>
            Cancel
          </Button>
          <Button size="sm" variant="primary" onClick={confirm}>
            Compact
          </Button>
        </>
      }
    >
      <p className="text-[13px] leading-relaxed" style={{ color: "var(--muted)" }}>
        The model will see a summary instead of the older messages. Your transcript stays as it
        is, and the most recent messages are kept word for word. This cannot be undone.
      </p>
      {context ? (
        <p className="mt-2 text-[12px]" style={{ color: "var(--faint)" }}>
          Context now: about {formatTokens(context.tokens)} of {formatTokens(context.window)}{" "}
          tokens.
        </p>
      ) : null}
      <label
        htmlFor="compact-focus"
        className="mt-3 mb-1 block text-[11.5px]"
        style={{ color: "var(--muted)" }}
      >
        Focus (optional)
      </label>
      <textarea
        id="compact-focus"
        value={focus}
        onChange={(event) => setFocus(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            confirm();
          }
        }}
        maxLength={2000}
        placeholder="e.g. keep the database migration decisions"
        className={inputClass}
        style={{ borderColor: "var(--border)", minHeight: 64 }}
      />
    </Modal>
  );
}
