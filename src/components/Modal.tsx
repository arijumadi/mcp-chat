import { useEffect, type ReactNode } from "react";
import { X } from "lucide-react";
import { cn } from "@/lib/cn";

interface ModalProps {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  subtitle?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  width?: "sm" | "md" | "lg" | "xl";
}

const widths = {
  sm: "max-w-md",
  md: "max-w-xl",
  lg: "max-w-2xl",
  xl: "max-w-4xl",
};

export function Modal({
  open,
  onClose,
  title,
  subtitle,
  children,
  footer,
  width = "md",
}: ModalProps) {
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <button
        aria-label="Close dialog"
        className="absolute inset-0 bg-black/45"
        onClick={onClose}
      />
      <div
        role="dialog"
        aria-modal="true"
        className={cn(
          "animate-in relative flex max-h-[88vh] w-full flex-col overflow-hidden rounded-2xl border shadow-2xl",
          widths[width],
        )}
        style={{
          background: "var(--panel)",
          borderColor: "var(--border)",
          boxShadow: "var(--shadow)",
        }}
      >
        <div
          className="flex items-start justify-between gap-4 border-b px-5 py-4"
          style={{ borderColor: "var(--border)" }}
        >
          <div className="min-w-0">
            <div className="text-[15px] font-semibold tracking-[-0.01em]">{title}</div>
            {subtitle ? (
              <div className="mt-0.5 text-[12.5px]" style={{ color: "var(--muted)" }}>
                {subtitle}
              </div>
            ) : null}
          </div>
          <button
            onClick={onClose}
            className="focus-ring -mr-1 rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
            aria-label="Close"
          >
            <X size={16} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">{children}</div>
        {footer ? (
          <div
            className="flex items-center justify-end gap-2 border-t px-5 py-3.5"
            style={{ borderColor: "var(--border)" }}
          >
            {footer}
          </div>
        ) : null}
      </div>
    </div>
  );
}
