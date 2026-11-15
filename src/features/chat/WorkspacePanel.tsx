import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  ChevronRight,
  FileCode2,
  FileText,
  Folder,
  FolderOpen,
  Loader2,
  MessageSquarePlus,
  X,
} from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type {
  Conversation,
  MessageContext,
  WorkspaceEntry,
  WorkspaceFile,
} from "@/lib/types";
import { Button } from "@/components/Button";
import { cn } from "@/lib/cn";
import { inputClass } from "@/components/ui";

type Selection = { start: number; end: number };

function baseName(path: string): string {
  const index = path.lastIndexOf("/");
  return index < 0 ? path : path.slice(index + 1);
}

function rangeLabel(selection: Selection | null): string {
  if (!selection) return "";
  return selection.start === selection.end
    ? `line ${selection.start}`
    : `lines ${selection.start}–${selection.end}`;
}

function TreeRow({
  entry,
  depth,
  expanded,
  loading,
  filtered,
  selected,
  onToggle,
  onOpen,
}: {
  entry: WorkspaceEntry;
  depth: number;
  expanded: boolean;
  loading: boolean;
  filtered: boolean;
  selected: boolean;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
}) {
  const isDir = entry.kind === "dir";
  return (
    <button
      type="button"
      onClick={() => (isDir ? onToggle(entry.path) : onOpen(entry.path))}
      className={cn(
        "focus-ring flex w-full min-w-0 items-center gap-1 rounded px-2 py-1 text-left text-[11.5px] transition hover:bg-[var(--panel-2)]",
        filtered && "font-medium",
        selected && "bg-[var(--panel-2)]",
      )}
      style={{ paddingLeft: 8 + depth * 13 }}
      title={entry.path}
    >
      {isDir ? (
        <>
          <ChevronRight
            size={11}
            className={cn("shrink-0 transition-transform", expanded && "rotate-90")}
            style={{ color: "var(--faint)" }}
          />
          {expanded ? (
            <FolderOpen size={13} className="shrink-0" style={{ color: "var(--warn)" }} />
          ) : (
            <Folder size={13} className="shrink-0" style={{ color: "var(--warn)" }} />
          )}
        </>
      ) : (
        <>
          <span className="w-[11px] shrink-0" />
          {/\.(tsx?|jsx?|rs|py|go|java|kt|json|md|css|html|toml|yaml|yml|sh|sql)$/i.test(
            entry.name,
          ) ? (
            <FileCode2 size={13} className="shrink-0" style={{ color: "var(--muted)" }} />
          ) : (
            <FileText size={13} className="shrink-0" style={{ color: "var(--muted)" }} />
          )}
        </>
      )}
      <span className="truncate">{entry.name}</span>
      {loading ? <Loader2 size={10} className="ml-auto animate-spin" /> : null}
    </button>
  );
}

export function WorkspacePanel({
  conversation,
  onClose,
}: {
  conversation: Conversation;
  onClose: () => void;
}) {
  const addContext = useApp((state) => state.addContext);
  const toast = useApp((state) => state.toast);
  const [entries, setEntries] = useState<Record<string, WorkspaceEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set([""]));
  const [loading, setLoading] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [file, setFile] = useState<WorkspaceFile | null>(null);
  const [fileLoading, setFileLoading] = useState(false);
  const [highlightedLines, setHighlightedLines] = useState<string[]>([]);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [comment, setComment] = useState("");
  const [commentFocus, setCommentFocus] = useState(false);
  const treeRequest = useRef(0);
  const fileRequest = useRef(0);

  const rootName = conversation.workspaceRoot
    ? baseName(conversation.workspaceRoot.replace(/[\\/]+$/, "")) || "workspace"
    : "workspace";

  const loadDir = async (path: string, force = false) => {
    if (!force && entries[path]) return;
    const request = treeRequest.current;
    setLoading((current) => new Set(current).add(path));
    try {
      const list = await call<WorkspaceEntry[]>("list_workspace_dir", {
        conversationId: conversation.id,
        path: path || null,
      });
      if (request !== treeRequest.current) return;
      setEntries((current) => ({ ...current, [path]: list }));
    } catch (error) {
      if (request !== treeRequest.current) return;
      toast(String(error), "error");
    } finally {
      if (request === treeRequest.current) {
        setLoading((current) => {
          const next = new Set(current);
          next.delete(path);
          return next;
        });
      }
    }
  };

  useEffect(() => {
    treeRequest.current += 1;
    fileRequest.current += 1;
    setEntries({});
    setExpanded(new Set([""]));
    setSelected(null);
    setFile(null);
    setFileLoading(false);
    setFilter("");
    setSelection(null);
    setComment("");
    setCommentFocus(false);
    void loadDir("", true);
    // The conversation id and workspace root identify this panel's tree.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversation.id, conversation.workspaceRoot]);

  const toggleDir = (path: string) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
        if (!entries[path]) void loadDir(path);
      }
      return next;
    });
  };

  const openFile = async (path: string) => {
    const request = ++fileRequest.current;
    setSelected(path);
    setFile(null);
    setSelection(null);
    setComment("");
    setFileLoading(true);
    try {
      const content = await call<WorkspaceFile>("read_workspace_file", {
        conversationId: conversation.id,
        path,
      });
      if (request !== fileRequest.current) return;
      setFile(content);
    } catch (error) {
      if (request !== fileRequest.current) return;
      toast(String(error), "error");
    } finally {
      if (request === fileRequest.current) setFileLoading(false);
    }
  };

  const lines = useMemo(() => (file?.content ?? "").split("\n"), [file?.content]);
  const selectedLines = useMemo(() => {
    if (!selection) return [];
    return lines.slice(selection.start - 1, selection.end);
  }, [lines, selection]);

  useEffect(() => {
    if (!file || file.binary) {
      setHighlightedLines([]);
      return;
    }
    let cancelled = false;
    setHighlightedLines([]);
    void import("@/lib/highlight")
      .then(({ highlightLines }) => {
        if (!cancelled) setHighlightedLines(highlightLines(file.content, file.path));
      })
      .catch(() => {
        if (!cancelled) setHighlightedLines([]);
      });
    return () => {
      cancelled = true;
    };
  }, [file]);

  const selectLine = (line: number, shift: boolean) => {
    setSelection((current) => {
      if (!current || !shift) return { start: line, end: line };
      return { start: Math.min(current.start, line), end: Math.max(current.start, line) };
    });
  };

  const addSelectionToChat = () => {
    if (!file || !selection) return;
    const text = comment.trim();
    if (!text) {
      setCommentFocus(true);
      return;
    }
    const context: MessageContext = {
      path: file.path,
      startLine: selection.start,
      endLine: selection.end,
      comment: text,
      preview: selectedLines.join("\n").slice(0, 12_000),
    };
    addContext(context);
    setSelection(null);
    setComment("");
    setCommentFocus(false);
    toast("File context added to the composer.", "info");
  };

  const renderTree = (path: string, depth = 0): ReactNode => {
    const list = entries[path] ?? [];
    return list.map((entry) => {
      const isExpanded = expanded.has(entry.path);
      return (
        <div key={entry.path}>
          <TreeRow
            entry={entry}
            depth={depth}
            expanded={isExpanded}
            loading={loading.has(entry.path)}
            filtered={Boolean(filter.trim())}
            selected={selected === entry.path}
            onToggle={toggleDir}
            onOpen={(next) => void openFile(next)}
          />
          {entry.kind === "dir" && isExpanded ? renderTree(entry.path, depth + 1) : null}
        </div>
      );
    });
  };

  const filteredTree = (path: string, depth = 0): ReactNode => {
    const list = entries[path] ?? [];
    const query = filter.trim().toLowerCase();
    if (!query) return renderTree(path, depth);
    const visible = list.filter((entry) => {
      if (entry.kind === "file") {
        return entry.path.toLowerCase().includes(query) || entry.name.toLowerCase().includes(query);
      }
      const children = entries[entry.path] ?? [];
      return (
        entry.name.toLowerCase().includes(query) ||
        entry.path.toLowerCase().includes(query) ||
        children.some((child) => child.path.toLowerCase().includes(query))
      );
    });
    return visible.map((entry) => {
      const isExpanded = expanded.has(entry.path) || Boolean(query);
      return (
        <div key={entry.path}>
          <TreeRow
            entry={entry}
            depth={depth}
            expanded={isExpanded}
            loading={loading.has(entry.path)}
            filtered={Boolean(query)}
            selected={selected === entry.path}
            onToggle={toggleDir}
            onOpen={(next) => void openFile(next)}
          />
          {entry.kind === "dir" && isExpanded
            ? filteredTree(entry.path, depth + 1)
            : null}
        </div>
      );
    });
  };

  return (
    <aside
      className="workspace-panel flex h-full min-w-0 shrink-0 flex-col border-l"
      style={{
        borderColor: "var(--border)",
        background: "var(--bg)",
      }}
      aria-label="Workspace files"
    >
      <header
        className="flex h-14 shrink-0 items-center gap-2 border-b px-3"
        style={{ borderColor: "var(--border)" }}
      >
        <Folder size={15} style={{ color: "var(--warn)" }} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12.5px] font-medium">Workspace files</div>
          <div className="truncate text-[10.5px]" style={{ color: "var(--faint)" }}>
            {conversation.workspaceRoot ?? "workspace root"}
          </div>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
          title="Close workspace"
          aria-label="Close workspace"
        >
          <X size={15} />
        </button>
      </header>

      <div className="flex min-h-0 flex-1">
        <aside
          className="flex w-[230px] shrink-0 flex-col border-r"
          style={{ borderColor: "var(--border)" }}
        >
          <div className="border-b p-2" style={{ borderColor: "var(--border)" }}>
            <input
              value={filter}
              onChange={(event) => setFilter(event.target.value)}
              placeholder="Filter loaded files"
              aria-label="Filter files"
              className={cn(inputClass, "h-8 text-[11px]")}
              style={{ borderColor: "var(--border)" }}
            />
          </div>
          <div className="min-h-0 flex-1 overflow-auto py-1">
            {entries[""] === undefined && loading.has("") ? (
              <div className="px-3 py-2 text-[11px]" style={{ color: "var(--faint)" }}>
                Loading files…
              </div>
            ) : (
              filteredTree("")
            )}
            {entries[""]?.length === 0 ? (
              <div className="px-3 py-2 text-[11px]" style={{ color: "var(--faint)" }}>
                Workspace is empty.
              </div>
            ) : null}
          </div>
        </aside>

        <section className="flex min-w-0 flex-1 flex-col">
          {file ? (
            <>
              <div
                className="flex shrink-0 flex-wrap items-center gap-2 border-b px-3 py-2"
                style={{ borderColor: "var(--border)" }}
              >
                <span className="min-w-0 flex-1 truncate font-mono text-[11.5px]">
                  {file.path}
                </span>
                {file.binary ? (
                  <span className="text-[10.5px]" style={{ color: "var(--danger)" }}>
                    binary preview unavailable
                  </span>
                ) : null}
                {file.truncated ? (
                  <span className="text-[10.5px]" style={{ color: "var(--warn)" }}>
                    truncated
                  </span>
                ) : null}
                <span className="text-[10.5px]" style={{ color: "var(--faint)" }}>
                  {file.size.toLocaleString()} B
                </span>
              </div>

              {selection ? (
                <div
                  className="shrink-0 border-b px-3 py-2"
                  style={{ borderColor: "var(--border)", background: "var(--panel)" }}
                >
                  <div className="flex items-center gap-2 text-[11.5px]">
                    <MessageSquarePlus size={13} style={{ color: "var(--accent)" }} />
                    <span className="font-medium">Add context for this change</span>
                    <span style={{ color: "var(--faint)" }}>{rangeLabel(selection)}</span>
                  </div>
                  <textarea
                    autoFocus={commentFocus}
                    value={comment}
                    onChange={(event) => setComment(event.target.value)}
                    placeholder="Add a comment for the model..."
                    aria-label="File context comment"
                    className={cn(inputClass, "mt-2 min-h-[64px] resize-y text-[11.5px]")}
                    style={{ borderColor: "var(--border)" }}
                  />
                  <div className="mt-2 flex justify-end gap-1.5">
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => {
                        setSelection(null);
                        setComment("");
                        setCommentFocus(false);
                      }}
                    >
                      Cancel
                    </Button>
                    <Button
                      size="sm"
                      variant="primary"
                      disabled={!comment.trim()}
                      onClick={addSelectionToChat}
                    >
                      Comment
                    </Button>
                  </div>
                </div>
              ) : null}

              <div className="min-h-0 flex-1 overflow-auto" style={{ background: "var(--code)" }}>
                {fileLoading ? (
                  <div className="flex items-center gap-2 px-4 py-3 text-[11.5px]" style={{ color: "var(--muted)" }}>
                    <Loader2 size={13} className="animate-spin" /> Loading file…
                  </div>
                ) : file.binary ? (
                  <div className="px-4 py-3 text-[11.5px]" style={{ color: "var(--muted)" }}>
                    This file is binary. Jarvis does not render binary files here.
                  </div>
                ) : (
                  <div className="min-w-max py-2 font-mono text-[11.5px] leading-[18px]">
                    {lines.map((_, index) => {
                      const number = index + 1;
                      const active =
                        selection !== null &&
                        number >= selection.start &&
                        number <= selection.end;
                      return (
                        <div
                          key={number}
                          role="button"
                          tabIndex={0}
                          onClick={(event) => selectLine(number, event.shiftKey)}
                          onKeyDown={(event) => {
                            if (event.key === "Enter" || event.key === " ") {
                              event.preventDefault();
                              selectLine(number, event.shiftKey);
                            }
                          }}
                          className="flex cursor-text whitespace-pre"
                          style={{
                            background: active ? "var(--panel-2)" : undefined,
                            boxShadow: active ? "inset 2px 0 var(--accent)" : undefined,
                          }}
                        >
                          <span
                            className="w-12 shrink-0 select-none pr-3 text-right"
                            style={{ color: active ? "var(--accent)" : "var(--faint)" }}
                          >
                            {number}
                          </span>
                          {highlightedLines[index] !== undefined ? (
                            <span
                              className="pr-4"
                              dangerouslySetInnerHTML={{
                                __html: highlightedLines[index] ?? "",
                              }}
                            />
                          ) : (
                            <span className="pr-4">{lines[index] || " "}</span>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            </>
          ) : (
            <div className="flex min-h-0 flex-1 flex-col items-center justify-center px-8 text-center">
              <FolderOpen size={24} style={{ color: "var(--faint)" }} />
              <div className="mt-2 text-[12.5px] font-medium">Select a file</div>
              <div className="mt-1 max-w-[280px] text-[11.5px]" style={{ color: "var(--muted)" }}>
                Browse {rootName}, open a file, and click a line to add file context to the
                composer.
              </div>
              {fileLoading ? <Loader2 size={14} className="mt-3 animate-spin" /> : null}
            </div>
          )}
        </section>
      </div>
    </aside>
  );
}
