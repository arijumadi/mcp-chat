import type { ButtonHTMLAttributes, ReactNode } from "react";
import { Loader2 } from "lucide-react";
import { cn } from "@/lib/cn";

type Variant = "primary" | "secondary" | "ghost" | "danger" | "quiet";
type Size = "sm" | "md";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  loading?: boolean;
  icon?: ReactNode;
}

export function Button({
  variant = "secondary",
  size = "md",
  loading,
  icon,
  className,
  children,
  disabled,
  ...props
}: ButtonProps) {
  return (
    <button
      {...props}
      disabled={disabled || loading}
      className={cn(
        "focus-ring inline-flex items-center justify-center gap-1.5 rounded-lg border font-medium transition select-none disabled:cursor-not-allowed disabled:opacity-45",
        size === "sm" ? "h-8 px-2.5 text-[12.5px]" : "h-9 px-3.5 text-[13.5px]",
        variant === "primary" &&
          "border-transparent bg-[var(--accent)] text-[var(--accent-fg)] hover:brightness-95",
        variant === "secondary" &&
          "border-[var(--border)] bg-[var(--panel)] hover:bg-[var(--panel-2)]",
        variant === "ghost" &&
          "border-transparent bg-transparent hover:bg-[var(--panel-2)]",
        variant === "quiet" &&
          "border-transparent bg-[var(--panel-2)] text-[var(--muted)] hover:text-[var(--fg)]",
        variant === "danger" &&
          "border-transparent bg-transparent text-[var(--danger)] hover:bg-[var(--danger-soft)]",
        className,
      )}
    >
      {loading ? <Loader2 size={14} className="animate-spin" /> : icon}
      {children}
    </button>
  );
}
