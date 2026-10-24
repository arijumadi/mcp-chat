import { AlertCircle, CheckCircle2, Copy, X } from "lucide-react";
import { useApp } from "@/app/store";
import { copyText } from "@/lib/clipboard";

export function Toasts() {
  const toasts = useApp((state) => state.toasts);
  const dismiss = useApp((state) => state.dismissToast);
  return (
    <div className="pointer-events-none fixed right-4 bottom-4 z-[60] flex w-[min(360px,calc(100vw-2rem))] flex-col gap-2">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className="animate-in pointer-events-auto flex items-start gap-2.5 rounded-xl border px-3.5 py-3 text-[13px] shadow-xl"
          style={{
            background: "var(--panel)",
            borderColor: toast.kind === "error" ? "var(--danger)" : "var(--border)",
            color: "var(--fg)",
          }}
          role={toast.kind === "error" ? "alert" : "status"}
        >
          {toast.kind === "error" ? (
            <AlertCircle size={16} className="mt-0.5 shrink-0" style={{ color: "var(--danger)" }} />
          ) : (
            <CheckCircle2 size={16} className="mt-0.5 shrink-0" style={{ color: "var(--accent)" }} />
          )}
          <div className="min-w-0 flex-1">
            <span className="block whitespace-pre-wrap">{toast.text}</span>
            {toast.actions?.length ? (
              <div className="mt-2 flex flex-wrap items-center gap-1.5">
                {toast.actions.map((action) => (
                  <button
                    key={action.label}
                    type="button"
                    onClick={() => {
                      dismiss(toast.id);
                      action.onClick();
                    }}
                    className="focus-ring rounded-md border px-2 py-1 text-[10.5px] font-medium transition hover:bg-[var(--panel-2)]"
                    style={{ borderColor: "var(--border)", color: "var(--accent)" }}
                  >
                    {action.label}
                  </button>
                ))}
              </div>
            ) : null}
          </div>
          <button
            type="button"
            onClick={() => void copyText(toast.text).catch(() => undefined)}
            className="focus-ring -mr-0.5 rounded p-0.5"
            style={{ color: "var(--faint)" }}
            title="Copy message"
            aria-label="Copy notification"
          >
            <Copy size={13} />
          </button>
          <button
            onClick={() => dismiss(toast.id)}
            className="focus-ring -mr-1 rounded p-0.5"
            aria-label="Dismiss"
          >
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
