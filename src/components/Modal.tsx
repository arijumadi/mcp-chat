import { useEffect, useId, useRef, type ReactNode } from "react";
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
  const dialogRef = useRef<HTMLDivElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);
  const titleId = useId();

  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    if (!open) return;
    previousFocus.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const frame = requestAnimationFrame(() => dialogRef.current?.focus());

    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      const node = dialogRef.current;
      if (!node) return;
      const focusable = Array.from(
        node.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      ).filter((element) => element.getClientRects().length > 0);
      if (focusable.length === 0) {
        event.preventDefault();
        node.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (!first || !last) return;
      if (event.shiftKey && (active === first || active === node)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("keydown", onKey);
      document.body.style.overflow = previousOverflow;
      const target = previousFocus.current;
      if (target && target.isConnected) target.focus();
    };
  }, [open]);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <button
        aria-label="Close dialog"
        className="absolute inset-0 bg-black/45"
        onClick={onClose}
      />
      <div
        ref={dialogRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
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
            <div id={titleId} className="text-[15px] font-semibold tracking-[-0.01em]">{title}</div>
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
