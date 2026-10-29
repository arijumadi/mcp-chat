import { useEffect, useState } from "react";
import { ShieldAlert } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { formatJson } from "@/lib/format";
import { inputClass } from "@/components/ui";

export function ApprovalModal() {
  const approvals = useApp((state) => state.approvals);
  const respond = useApp((state) => state.respondApproval);
  const [feedback, setFeedback] = useState("");
  const [rejecting, setRejecting] = useState(false);
  const approval = approvals[0];

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
          {approvals.length > 1 ? ` · 1 of ${approvals.length} pending` : ""}
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
            <Button size="sm" variant="ghost" onClick={() => setRejecting(true)}>
              Reject
            </Button>
            {approval.allowAlways ? (
              <Button
                size="sm"
                variant="secondary"
                onClick={() => void respond(approval.callId, "always")}
                title="Adds a saved rule for this tool"
              >
                Always allow
              </Button>
            ) : null}
            <Button
              size="sm"
              variant="primary"
              onClick={() => void respond(approval.callId, "once")}
            >
              Allow once
            </Button>
          </>
        )
      }
    >
      <p className="mb-3 text-[12.5px]" style={{ color: "var(--muted)" }}>
        The model asked to execute this tool. Review the arguments before allowing it.
      </p>
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
