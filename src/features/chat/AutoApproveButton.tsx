import { useState } from "react";
import { ShieldCheck } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";

export function AutoApproveButton() {
  const enabled = useApp((state) => state.autoApprove);
  const setAutoApprove = useApp((state) => state.setAutoApprove);
  const [confirmOpen, setConfirmOpen] = useState(false);

  const toggle = async () => {
    if (enabled) {
      await setAutoApprove(false);
      return;
    }
    setConfirmOpen(true);
  };

  const enable = async () => {
    await setAutoApprove(true);
    if (useApp.getState().autoApprove) setConfirmOpen(false);
  };

  return (
    <>
      <button
        type="button"
        onClick={() => void toggle()}
        className="focus-ring flex items-center gap-1.5 rounded-lg border border-transparent px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)]"
        style={{
          borderColor: enabled ? "var(--accent)" : "transparent",
          background: enabled ? "var(--accent-soft)" : "transparent",
          color: enabled ? "var(--accent)" : "var(--muted)",
        }}
        title={
          enabled
            ? "Auto-approve is on for this chat"
            : "Enable auto-approve for this chat"
        }
      >
        <ShieldCheck size={12} />
        <span>Auto</span>
      </button>

      <Modal
        open={confirmOpen}
        onClose={() => setConfirmOpen(false)}
        width="sm"
        title="Enable auto-approve?"
        subtitle="This applies only to the current chat."
        footer={
          <>
            <Button size="sm" variant="ghost" onClick={() => setConfirmOpen(false)}>
              Cancel
            </Button>
            <Button size="sm" variant="primary" onClick={() => void enable()}>
              Enable for this session
            </Button>
          </>
        }
      >
        <p className="text-[13.5px] leading-relaxed" style={{ color: "var(--muted)" }}>
          Built-in tools in this chat will run without asking for approval,
          except dangerous shell commands, patch deletions, and
          outside-workspace writes. Read-only tools may access explicit paths
          outside the workspace. MCP server policies still apply. This setting
          lasts until Jarvis is closed.
        </p>
      </Modal>
    </>
  );
}
