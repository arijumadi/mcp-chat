import { AlertCircle, CheckCircle2, X } from "lucide-react";
import { useApp } from "@/app/store";

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
          role="status"
        >
          {toast.kind === "error" ? (
            <AlertCircle size={16} className="mt-0.5 shrink-0" style={{ color: "var(--danger)" }} />
          ) : (
            <CheckCircle2 size={16} className="mt-0.5 shrink-0" style={{ color: "var(--accent)" }} />
          )}
          <span className="min-w-0 flex-1 whitespace-pre-wrap">{toast.text}</span>
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
