import { useEffect, useRef, useState } from "react";
import { CornerDownLeft, Search } from "lucide-react";
import { useApp } from "@/app/store";
import { cn } from "@/lib/cn";
import { inputClass } from "@/components/ui";

export function QuickSwitcher({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const conversations = useApp((state) => state.conversations);
  const searchHits = useApp((state) => state.searchHits);
  const search = useApp((state) => state.search);
  const clearSearch = useApp((state) => state.clearSearch);
  const openConversation = useApp((state) => state.openConversation);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setIndex(0);
    clearSearch();
    setTimeout(() => inputRef.current?.focus(), 20);
  }, [open, clearSearch]);

  useEffect(() => {
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => void search(query), 150);
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
    };
  }, [query, search]);

  useEffect(() => {
    setIndex(0);
  }, [query, searchHits.length]);

  if (!open) return null;

  const results = query.trim()
    ? searchHits.map((hit) => ({
        id: hit.conversationId,
        title: hit.title,
        subtitle: hit.snippet,
      }))
    : conversations.slice(0, 8).map((conversation) => ({
        id: conversation.id,
        title: conversation.title,
        subtitle: "",
      }));

  const choose = (id: string | undefined) => {
    if (!id) return;
    onClose();
    void openConversation(id);
  };

  return (
    <div className="fixed inset-0 z-[70] flex items-start justify-center px-4 pt-[12vh]">
      <button
        className="absolute inset-0 bg-black/40"
        aria-label="Close quick switcher"
        onClick={onClose}
      />
      <div
        className="animate-in relative w-full max-w-[540px] overflow-hidden rounded-2xl border shadow-2xl"
        style={{ background: "var(--panel)", borderColor: "var(--border)", boxShadow: "var(--shadow)" }}
        role="dialog"
        aria-modal="true"
      >
        <div className="flex items-center gap-2 border-b px-3.5" style={{ borderColor: "var(--border)" }}>
          <Search size={15} style={{ color: "var(--faint)" }} />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") onClose();
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setIndex((current) => Math.min(current + 1, Math.max(results.length - 1, 0)));
              }
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setIndex((current) => Math.max(current - 1, 0));
              }
              if (event.key === "Enter") {
                event.preventDefault();
                choose(results[index]?.id);
              }
            }}
            placeholder="Search conversations…"
            className={cn(inputClass, "h-12 flex-1 border-0 px-0 shadow-none outline-none")}
          />
          <span className="rounded border px-1.5 py-0.5 text-[10.5px]" style={{ borderColor: "var(--border)", color: "var(--faint)" }}>
            Esc
          </span>
        </div>
        <div className="max-h-[52vh] overflow-y-auto p-1.5">
          {results.length === 0 ? (
            <div className="px-3 py-6 text-center text-[12.5px]" style={{ color: "var(--faint)" }}>
              No conversations found.
            </div>
          ) : (
            results.map((result, resultIndex) => (
              <button
                key={result.id}
                onMouseEnter={() => setIndex(resultIndex)}
                onClick={() => choose(result.id)}
                className={cn(
                  "flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left transition",
                  resultIndex === index ? "bg-[var(--panel-2)]" : "",
                )}
              >
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13.5px]">{result.title}</div>
                  {result.subtitle ? (
                    <div className="truncate text-[11.5px]" style={{ color: "var(--muted)" }}>
                      {result.subtitle}
                    </div>
                  ) : null}
                </div>
                {resultIndex === index ? (
                  <CornerDownLeft size={12} style={{ color: "var(--faint)" }} />
                ) : null}
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
