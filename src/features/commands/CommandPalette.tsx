import { useEffect, useMemo, useRef, useState } from "react";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import { ChevronLeft, Search } from "lucide-react";
import { useApp } from "@/app/store";
import { useUi, type PalettePage } from "@/app/ui";
import { inputClass } from "@/components/ui";
import { cn } from "@/lib/cn";
import { shortModelName } from "@/lib/format";
import { filterCommands } from "./registry";
import { formatShortcut } from "./shortcuts";

interface Row {
  key: string;
  title: string;
  hint?: string;
  /** Why the row cannot be chosen, shown instead of the hint. */
  disabled?: string | null;
  current?: boolean;
  run: () => void | Promise<void>;
}

const PLACEHOLDER: Record<PalettePage, string> = {
  root: "Type a command…",
  models: "Choose a model…",
  workspaces: "Choose a workspace…",
};

/**
 * The command palette. Every row is a registry command, or on the model and
 * workspace pages, a choice that uses the same store actions as the pickers
 * in the composer.
 */
export function CommandPalette() {
  const palette = useUi((state) => state.palette);
  const openPalette = useUi((state) => state.openPalette);
  const close = useUi((state) => state.closePalette);
  // Only what the rows read, so streaming deltas do not re-render the palette.
  const status = useApp((state) => state.status);
  const activeId = useApp((state) => state.activeId);
  const view = useApp((state) => state.view);
  const providers = useApp((state) => state.providers);
  const approvals = useApp((state) => state.approvals);
  const settings = useApp((state) => state.settings);
  const conversations = useApp((state) => state.conversations);
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const page = palette?.page ?? "root";
  const query = palette?.query ?? "";
  const isOpen = palette !== null;

  useEffect(() => {
    if (!isOpen) return;
    setIndex(0);
    const timer = window.setTimeout(() => inputRef.current?.focus(), 20);
    return () => window.clearTimeout(timer);
  }, [isOpen, page]);

  const rows = useMemo<Row[]>(() => {
    if (!isOpen) return [];
    const state = useApp.getState();
    const needle = query.trim().toLowerCase();
    const matches = (text: string) => !needle || text.toLowerCase().includes(needle);

    if (page === "models") {
      const conversation = state.conversations.find((item) => item.id === state.activeId);
      return state.providers.flatMap((provider) => {
        const models = provider.models.length > 0 ? provider.models : [provider.model];
        return models
          .filter((model) => matches(`${provider.name} ${model} ${shortModelName(model)}`))
          .map<Row>((model) => ({
            key: `${provider.id}:${model}`,
            title: shortModelName(model),
            hint: provider.name,
            current:
              conversation?.providerId === provider.id &&
              (conversation?.model ?? provider.model) === model,
            run: async () => {
              await state.setModel(provider.id, model);
              state.toast(`Model set to ${shortModelName(model)}.`, "info");
            },
          }));
      });
    }

    if (page === "workspaces") {
      const conversation = state.conversations.find((item) => item.id === state.activeId);
      const current = conversation?.workspaceRoot ?? state.appInfo?.workspaceDir ?? "";
      let recent: string[] = [];
      try {
        const parsed = JSON.parse(state.settings["workspace.recent"] ?? "[]") as unknown;
        if (Array.isArray(parsed)) {
          recent = parsed.filter((item): item is string => typeof item === "string");
        }
      } catch {
        recent = [];
      }
      const apply = async (path: string) => {
        if (!state.activeId) return;
        try {
          await state.setConversationWorkspace(state.activeId, path);
          state.toast("Workspace updated for this chat.", "info");
        } catch (error) {
          state.toast(String(error), "error");
        }
      };
      const choose: Row = {
        key: "browse",
        title: "Browse for a folder…",
        run: async () => {
          if (window.__MCP_CHAT_MOCK__) {
            await apply(state.appInfo?.workspaceDir ?? "/home/you/project");
            return;
          }
          try {
            const selected = await openFolderDialog({
              directory: true,
              multiple: false,
              title: "Choose workspace folder",
            });
            if (typeof selected === "string") await apply(selected);
          } catch (error) {
            state.toast(String(error), "error");
          }
        },
      };
      const recents = recent
        .filter((path) => matches(path))
        .map<Row>((path) => ({
          key: path,
          title: path,
          current: path === current,
          run: () => apply(path),
        }));
      return [...recents, ...(matches("browse folder") ? [choose] : [])];
    }

    return filterCommands(query).map<Row>((command) => {
      const reason = command.disabled(state);
      return {
        key: command.id,
        title: command.title,
        hint: command.shortcut ? formatShortcut(command.shortcut) : undefined,
        disabled: reason,
        run: () => command.run(useApp.getState()),
      };
    });
    // `status`, `view` and `approvals` are not read directly but decide whether
    // a command is disabled, so the rows must be rebuilt when they change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, page, query, status, activeId, view, providers, approvals, settings, conversations]);

  // Keep the highlight on a row that exists.
  const active = Math.min(index, Math.max(rows.length - 1, 0));

  if (!palette) return null;

  const choose = (row: Row | undefined) => {
    if (!row) return;
    if (row.disabled) {
      useApp.getState().toast(row.disabled, "info");
      return;
    }
    // The command may open another overlay, so close first, then run.
    close();
    void row.run();
  };

  const setQuery = (value: string) => openPalette(page, value);

  return (
    <div className="fixed inset-0 z-[75] flex items-start justify-center px-4 pt-[12vh]">
      <button className="absolute inset-0 bg-black/40" aria-label="Close command palette" onClick={close} />
      <div
        className="animate-in relative w-full max-w-[560px] overflow-hidden rounded-2xl border shadow-2xl"
        style={{ background: "var(--panel)", borderColor: "var(--border)", boxShadow: "var(--shadow)" }}
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
      >
        <div className="flex items-center gap-2 border-b px-3.5" style={{ borderColor: "var(--border)" }}>
          {page === "root" ? (
            <Search size={15} style={{ color: "var(--faint)" }} aria-hidden />
          ) : (
            <button
              type="button"
              onClick={() => openPalette("root")}
              className="focus-ring -ml-1 rounded p-1"
              style={{ color: "var(--faint)" }}
              aria-label="Back to all commands"
            >
              <ChevronLeft size={16} />
            </button>
          )}
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setIndex(0);
            }}
            onKeyDown={(event) => {
              event.stopPropagation();
              if (event.key === "Escape") {
                event.preventDefault();
                if (page === "root") close();
                else openPalette("root");
              } else if (event.key === "ArrowDown") {
                event.preventDefault();
                setIndex(Math.min(active + 1, Math.max(rows.length - 1, 0)));
              } else if (event.key === "ArrowUp") {
                event.preventDefault();
                setIndex(Math.max(active - 1, 0));
              } else if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                event.preventDefault();
                choose(rows[active]);
              }
            }}
            role="combobox"
            aria-expanded
            aria-controls="palette-list"
            aria-activedescendant={rows[active] ? `palette-row-${active}` : undefined}
            aria-label={PLACEHOLDER[page]}
            placeholder={PLACEHOLDER[page]}
            className={cn(inputClass, "h-12 flex-1 border-0 px-0 shadow-none outline-none")}
          />
          <span className="rounded border px-1.5 py-0.5 text-[10.5px]" style={{ borderColor: "var(--border)", color: "var(--faint)" }}>
            Esc
          </span>
        </div>
        <div id="palette-list" role="listbox" className="max-h-[52vh] overflow-y-auto p-1.5">
          {rows.length === 0 ? (
            <div className="px-3 py-6 text-center text-[12.5px]" style={{ color: "var(--faint)" }}>
              {page === "root" ? "No matching commands." : "Nothing matches."}
            </div>
          ) : (
            rows.map((row, position) => (
              <button
                key={row.key}
                id={`palette-row-${position}`}
                type="button"
                role="option"
                aria-selected={position === active}
                aria-disabled={row.disabled ? true : undefined}
                onMouseMove={() => setIndex(position)}
                onClick={() => choose(row)}
                className={cn(
                  "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-[13px] transition",
                  position === active && "bg-[var(--panel-2)]",
                )}
                style={{ opacity: row.disabled ? 0.55 : 1 }}
              >
                <span className="min-w-0 flex-1 truncate">
                  {row.title}
                  {row.current ? (
                    <span className="ml-2 text-[11px]" style={{ color: "var(--accent)" }}>
                      current
                    </span>
                  ) : null}
                </span>
                {row.disabled ? (
                  <span className="shrink-0 text-[11px]" style={{ color: "var(--faint)" }}>
                    {row.disabled}
                  </span>
                ) : row.hint ? (
                  <kbd
                    className="shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10.5px]"
                    style={{ borderColor: "var(--border)", color: "var(--faint)" }}
                  >
                    {row.hint}
                  </kbd>
                ) : null}
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
