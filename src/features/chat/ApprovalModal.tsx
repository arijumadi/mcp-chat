import { useEffect, useState } from "react";
import { ShieldAlert } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { formatJson } from "@/lib/format";
import { inputClass } from "@/components/ui";
import { capabilityOf, toolForRole } from "@/lib/capabilities";
import { commandById } from "@/features/commands/registry";
import { ariaKeyShortcuts, formatShortcut } from "@/features/commands/shortcuts";
import { DiffView } from "./DiffView";

/** Tooltip and aria attributes for a button that has a keyboard shortcut. */
function shortcutProps(id: string, label: string) {
  const shortcut = commandById(id)?.shortcut;
  if (!shortcut) return { title: label };
  return {
    title: `${label} (${formatShortcut(shortcut)})`,
    "aria-keyshortcuts": ariaKeyShortcuts(shortcut),
  };
}

function overrideParam(server: string, tool: string): string {
  return capabilityOf(server, tool)?.overrideParam ?? "";
}

/** A shell call that sets the flag lifting its file-write guard. */
function guardOverridden(server: string, tool: string, args: Record<string, unknown>): boolean {
  const capability = capabilityOf(server, tool);
  return capability?.risk === "shell" && capability.overrideParam !== null && args[capability.overrideParam] === true;
}

export function ApprovalModal() {
  const approvals = useApp((state) => state.approvals);
  const activeId = useApp((state) => state.activeId);
  const respond = useApp((state) => state.respondApproval);
  const [feedback, setFeedback] = useState("");
  const [rejecting, setRejecting] = useState(false);
  const pending = approvals.filter((item) => item.conversationId === activeId);
  const approval = pending[0];

  useEffect(() => {
    setFeedback("");
    setRejecting(false);
  }, [approval?.callId]);

  if (!approval) return null;
  return (
    <Modal
      open
      onClose={() => undefined}
      width="md"
      title={
        <span className="flex items-center gap-2">
          <ShieldAlert size={16} style={{ color: "var(--warn)" }} />
          Tool permission
        </span>
      }
      subtitle={
        <>
          {approval.server} wants to run <code className="font-mono">{approval.tool}</code>
          {pending.length > 1 ? ` · 1 of ${pending.length} pending` : ""}
        </>
      }
      footer={
        rejecting ? (
          <>
            <Button size="sm" variant="ghost" onClick={() => setRejecting(false)}>
              Back
            </Button>
            <Button
              size="sm"
              variant="danger"
              onClick={() => void respond(approval.callId, "reject", feedback.trim() || undefined)}
            >
              Reject call
            </Button>
          </>
        ) : (
          <>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => setRejecting(true)}
              {...shortcutProps("approval.reject", "Reject, with optional feedback. The shortcut rejects at once")}
            >
              Reject
            </Button>
            {approval.allowAlways ? (
              <Button
                size="sm"
                variant="secondary"
                onClick={() => void respond(approval.callId, "always")}
                {...shortcutProps("approval.always", "Adds a saved rule for this tool")}
              >
                Always allow
              </Button>
            ) : null}
            <Button
              size="sm"
              variant="primary"
              onClick={() => void respond(approval.callId, "once")}
              {...shortcutProps("approval.once", "Allow this call once")}
            >
              Allow once
            </Button>
          </>
        )
      }
    >
      <p className="mb-3 text-[12.5px]" style={{ color: "var(--muted)" }}>
        The model asked to execute this tool. Review the changes and arguments before allowing it.
      </p>
      {guardOverridden(approval.server, approval.tool, approval.args) ? (
        <div
          role="note"
          className="mb-3 rounded-lg px-3 py-2 text-[12px]"
          style={{ background: "var(--warn-soft)", color: "var(--warn)" }}
        >
          <span className="font-medium">{overrideParam(approval.server, approval.tool)} is on.</span>{" "}
          This command may write file contents directly. Normally that goes through{" "}
          {toolForRole("create_file")} or {toolForRole("modify_file")}.
        </div>
      ) : null}
      {approval.diff ? (
        <div className="mb-3">
          <DiffView
            key={approval.callId}
            diff={approval.diff}
            defaultOpen
            className="diff-view-modal"
          />
        </div>
      ) : null}
      <div className="mb-1 text-[11.5px] font-medium" style={{ color: "var(--muted)" }}>
        Arguments
      </div>
      <pre
        className="max-h-[38vh] overflow-auto rounded-lg border p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap"
        style={{ borderColor: "var(--border)", background: "var(--code)" }}
      >
        {formatJson(approval.args)}
      </pre>
      {rejecting ? (
        <div className="mt-3">
          <label className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
            Optional feedback for the model
          </label>
          <textarea
            value={feedback}
            onChange={(event) => setFeedback(event.target.value)}
            placeholder="e.g. use the read-only search tool instead"
            className={inputClass}
            style={{ borderColor: "var(--border)", minHeight: 72 }}
          />
        </div>
      ) : null}
    </Modal>
  );
}
