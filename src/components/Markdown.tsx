import {
  isValidElement,
  memo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeHighlight from "rehype-highlight";
import { Check, Copy } from "lucide-react";
import { call } from "@/lib/ipc";
import { useApp } from "@/app/store";
import { useSmoothText } from "@/features/chat/useSmoothText";

function languageOf(children: ReactNode): string | null {
  const child = Array.isArray(children) ? children[0] : children;
  if (!isValidElement(child)) return null;
  const className = (child.props as { className?: string }).className ?? "";
  const match = /language-([\w-]+)/.exec(className);
  return match?.[1] ?? null;
}

function CodeBlock({ children }: { children?: ReactNode }) {
  const ref = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const language = languageOf(children);
  const copy = async () => {
    const text = ref.current?.innerText ?? "";
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard access is optional in the webview.
    }
  };
  return (
    <div
      className="my-3 overflow-hidden rounded-xl border"
      style={{ borderColor: "var(--border)", background: "var(--code)" }}
    >
      <div
        className="flex items-center justify-between border-b px-3 py-1.5"
        style={{ borderColor: "var(--border)" }}
      >
        <span
          className="font-mono text-[10.5px] tracking-wide uppercase"
          style={{ color: "var(--faint)" }}
        >
          {language ?? "code"}
        </span>
        <button
          onClick={() => void copy()}
          className="focus-ring flex items-center gap-1 rounded px-1.5 py-0.5 text-[10.5px] transition hover:bg-[var(--panel-2)]"
          style={{ color: "var(--muted)" }}
        >
          {copied ? <Check size={11} /> : <Copy size={11} />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre ref={ref} className="max-w-full">
        {children}
      </pre>
    </div>
  );
}

const components: Components = {
  pre({ children }) {
    return <CodeBlock>{children}</CodeBlock>;
  },
  a({ href, children }) {
    const toast = useApp.getState().toast;
    return (
      <a
        href={href}
        onClick={(event) => {
          event.preventDefault();
          if (!href) return;
          void call("open_external", { url: href }).catch((error) => {
            toast(String(error), "error");
          });
        }}
      >
        {children}
      </a>
    );
  },
  img({ alt }) {
    return <span className="text-[12.5px] italic">[{alt || "image"}]</span>;
  },
};

function safeUrl(url: string): string {
  if (/^(https?:|mailto:)/i.test(url)) return url;
  return "";
}

/**
 * Markdown is expensive: remark + rehype + highlight run over the whole block
 * on every render. Streaming re-renders the transcript on each token, so without
 * this memo every finished message in the history was re-parsed many times a
 * second. `text` is a string, so the default shallow comparison is enough.
 *
 * Text that grows while the component is mounted is paced out; text present on
 * the first render (a transcript loaded from disk) appears at once.
 */
export const Markdown = memo(function Markdown({
  text,
  streaming = false,
}: {
  text: string;
  streaming?: boolean;
}) {
  const shown = useSmoothText(text);
  return (
    <div className={streaming ? "md streaming" : "md"}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[[rehypeHighlight, { detect: false, ignoreMissing: true }]]}
        components={components}
        urlTransform={safeUrl}
        skipHtml
      >
        {shown}
      </ReactMarkdown>
    </div>
  );
});
