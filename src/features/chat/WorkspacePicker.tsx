import { useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, Folder, Loader2 } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useApp } from "@/app/store";
import type { Conversation } from "@/lib/types";

function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

export function WorkspacePicker({
  conversation,
  disabled = false,
}: {
  conversation: Conversation;
  disabled?: boolean;
}) {
  const appInfo = useApp((state) => state.appInfo);
  const settings = useApp((state) => state.settings);
  const setWorkspace = useApp((state) => state.setConversationWorkspace);
  const toast = useApp((state) => state.toast);
  const [openPicker, setOpenPicker] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const current = conversation.workspaceRoot ?? appInfo?.workspaceDir ?? "";

  const recent = useMemo(() => {
    try {
      const parsed = JSON.parse(settings["workspace.recent"] ?? "[]") as unknown;
      return Array.isArray(parsed)
        ? parsed.filter((item): item is string => typeof item === "string")
        : [];
    } catch {
      return [];
    }
  }, [settings]);

  useEffect(() => {
    if (openPicker) setError(null);
  }, [openPicker]);

  useEffect(() => {
    if (!openPicker) return;
    const onClick = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) {
        setOpenPicker(false);
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [openPicker]);

  const apply = async (path: string) => {
    if (!path.trim()) return;
    setSaving(true);
    setError(null);
    try {
      await setWorkspace(conversation.id, path);
      setOpenPicker(false);
      toast("Workspace updated for this chat.", "info");
    } catch (error) {
      const message = String(error);
      setError(message);
      toast(message, "error");
    } finally {
      setSaving(false);
    }
  };

  const choose = async () => {
    if (window.__MCP_CHAT_MOCK__) {
      await apply(appInfo?.workspaceDir ?? "/home/you/project");
      return;
    }
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose workspace folder",
      });
      if (typeof selected === "string") await apply(selected);
    } catch (error) {
      toast(String(error), "error");
    }
  };

  const choices = recent.filter((path) => path !== current);

  return (
    <div ref={ref} className="relative min-w-0">
      <button
        type="button"
        onClick={() => setOpenPicker((value) => !value)}
        disabled={disabled}
        className="focus-ring flex max-w-[220px] items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)] disabled:cursor-not-allowed disabled:opacity-50"
        style={{ borderColor: "var(--border)", color: "var(--muted)" }}
        title={current || "Choose workspace"}
      >
        <Folder size={12} className="shrink-0" />
        <span className="truncate" style={{ color: "var(--fg)" }}>
          {current ? baseName(current) : "Workspace"}
        </span>
        <ChevronDown size={11} className="shrink-0" />
      </button>

      {openPicker ? (
        <div
          className="animate-in absolute bottom-full left-0 z-30 mb-1.5 w-[360px] rounded-xl border p-3 shadow-xl"
          style={{ background: "var(--panel)", borderColor: "var(--border)" }}
        >
          <div className="text-[12px] font-medium">Workspace for this chat</div>
          <div
            className="mt-1 truncate font-mono text-[10.5px]"
            style={{ color: "var(--faint)" }}
            title={current}
          >
            {current || "No workspace selected"}
          </div>
          <div className="mt-2.5 grid gap-1">
            {current ? (
              <div
                className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-[12px]"
                style={{ background: "var(--panel-2)" }}
              >
                <Check size={12} style={{ color: "var(--accent)" }} />
                <span className="min-w-0 flex-1 truncate">{current}</span>
              </div>
            ) : null}
            {choices.map((path) => (
              <button
                key={path}
                type="button"
                onClick={() => void apply(path)}
                className="focus-ring flex items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[12px] transition hover:bg-[var(--panel-2)]"
              >
                <Folder size={12} style={{ color: "var(--faint)" }} />
                <span className="min-w-0 flex-1 truncate">{path}</span>
              </button>
            ))}
          </div>
          {error ? (
            <div
              className="mt-2 rounded-lg px-2 py-1.5 text-[10.5px] leading-relaxed"
              style={{ background: "var(--danger-soft)", color: "var(--danger)" }}
              role="alert"
            >
              {error}
            </div>
          ) : null}
          <div className="mt-3 flex flex-wrap items-center gap-1">
            <button
              type="button"
              onClick={() => void choose()}
              disabled={saving}
              className="focus-ring flex items-center gap-1.5 rounded-lg px-2 py-1.5 text-[11.5px] font-medium transition hover:bg-[var(--panel-2)] disabled:opacity-50"
              style={{ color: "var(--accent)" }}
            >
              {saving ? <Loader2 size={12} className="animate-spin" /> : <Folder size={12} />}
              Choose folder…
            </button>
            {appInfo?.workspaceDir && appInfo.workspaceDir !== current ? (
              <button
                type="button"
                onClick={() => void apply(appInfo.workspaceDir)}
                disabled={saving}
                className="focus-ring rounded-lg px-2 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)] disabled:opacity-50"
                style={{ color: "var(--muted)" }}
              >
                Use app directory
              </button>
            ) : null}
            <button
              type="button"
              onClick={() => setOpenPicker(false)}
              className="focus-ring ml-auto rounded-lg px-2 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)]"
              style={{ color: "var(--muted)" }}
            >
              Cancel
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
