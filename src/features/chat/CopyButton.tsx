import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { useApp } from "@/app/store";
import { copyRich, copyText } from "@/lib/clipboard";

export interface CopyPayload {
  text: string;
  html?: string;
}

export function CopyButton({ payload }: { payload: () => CopyPayload }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, []);

  const copy = async () => {
    const value = payload();
    if (!value.text && !value.html) return;
    try {
      if (value.html) await copyRich(value.text, value.html);
      else await copyText(value.text);
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
