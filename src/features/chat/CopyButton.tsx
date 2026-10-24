import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { useApp } from "@/app/store";
import { copyText } from "@/lib/clipboard";

export function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, []);

  const copy = async () => {
    try {
      await copyText(text);
      setCopied(true);
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), 1400);
    } catch {
      useApp.getState().toast("Could not copy to the clipboard.", "error");
    }
  };

  return (
    <button
      type="button"
      onClick={() => void copy()}
      className="focus-ring flex h-6 w-6 items-center justify-center rounded-md transition hover:bg-[var(--panel-2)] hover:text-[var(--fg)]"
      style={{ color: "var(--muted)" }}
      title={copied ? "Copied" : "Copy message"}
      aria-label={copied ? "Copied" : "Copy message"}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}
