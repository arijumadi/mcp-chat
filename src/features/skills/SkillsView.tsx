import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Loader2, RefreshCw, Sparkles } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { SkillView } from "@/lib/types";
import { Button } from "@/components/Button";
import { cn } from "@/lib/cn";

const SOURCE_LABELS: Record<string, string> = {
  workspace: "Workspace",
  app: "App data",
  opencode: "OpenCode",
  global: "Global",
};

function sourceLabel(source: string): string {
  return SOURCE_LABELS[source] ?? source;
}

export function SkillsView() {
  const appInfo = useApp((state) => state.appInfo);
  const activeId = useApp((state) => state.activeId);
  const conversations = useApp((state) => state.conversations);
  const setView = useApp((state) => state.setView);
  const toast = useApp((state) => state.toast);

  const [skills, setSkills] = useState<SkillView[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState<Set<string>>(new Set());

  const workspaceRoot =
    conversations.find((conversation) => conversation.id === activeId)?.workspaceRoot ??
    appInfo?.workspaceDir ??
    null;

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const next = await call<SkillView[]>("list_skills", { workspaceRoot });
      setSkills(next);
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setLoading(false);
    }
  }, [toast, workspaceRoot]);

  useEffect(() => {
    void load();
  }, [load]);

  const toggle = async (skill: SkillView) => {
    const enabled = !skill.enabled;
    setSkills((current) =>
      current.map((item) => (item.name === skill.name ? { ...item, enabled } : item)),
    );
    setSaving((current) => new Set(current).add(skill.name));
    try {
      await call("set_skill_enabled", { name: skill.name, enabled });
      toast(
        `Skill "${skill.name}" ${enabled ? "enabled" : "disabled"} for all chats.`,
        "info",
      );
    } catch (error) {
      setSkills((current) =>
        current.map((item) =>
          item.name === skill.name ? { ...item, enabled: skill.enabled } : item,
        ),
      );
      toast(String(error), "error");
    } finally {
      setSaving((current) => {
        const next = new Set(current);
        next.delete(skill.name);
        return next;
      });
    }
  };

  const enabledCount = skills.filter((skill) => skill.enabled).length;

  return (
    <div className="flex h-full min-w-0 flex-1 flex-col">
      <header
        className="flex h-14 shrink-0 items-center gap-2 border-b px-3"
        style={{ borderColor: "var(--border)" }}
      >
        <button
          onClick={() => setView("chat")}
          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
          aria-label="Back to chat"
        >
          <ArrowLeft size={16} />
        </button>
        <div className="ml-1">
          <div className="text-[14px] font-medium">Skills</div>
          <div className="text-[10.5px]" style={{ color: "var(--faint)" }}>
            reusable workflows the model can load on demand
          </div>
        </div>
        <div className="ml-auto flex items-center gap-2">
          <span className="text-[11px]" style={{ color: "var(--muted)" }}>
            {enabledCount} of {skills.length} enabled
          </span>
          <Button
            size="sm"
            variant="ghost"
            icon={<RefreshCw size={12} className={loading ? "animate-spin" : ""} />}
            onClick={() => void load()}
            disabled={loading}
          >
            Refresh
          </Button>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[860px] px-6 py-7">
          <div
            className="mb-4 rounded-xl border px-4 py-3"
            style={{ borderColor: "var(--border)", background: "var(--panel)" }}
          >
            <div className="flex items-start gap-2">
              <Sparkles size={15} className="mt-0.5 shrink-0" style={{ color: "var(--accent)" }} />
              <div>
                <div className="text-[12.5px] font-medium">Global skill settings</div>
                <p className="mt-0.5 text-[11.5px] leading-relaxed" style={{ color: "var(--muted)" }}>
                  A disabled skill is hidden from the model and cannot be loaded with the
                  <code className="mx-1 rounded bg-[var(--code)] px-1 py-0.5 font-mono text-[10.5px]">
                    skill
                  </code>
                  tool. Changes apply to every chat and every new chat.
                </p>
              </div>
            </div>
          </div>

          {loading ? (
            <div
              className="flex items-center justify-center gap-2 py-20 text-[12.5px]"
              style={{ color: "var(--muted)" }}
            >
              <Loader2 size={15} className="animate-spin" /> Loading skills…
            </div>
          ) : skills.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-20 text-center">
              <div
                className="mb-4 flex h-12 w-12 items-center justify-center rounded-2xl border"
                style={{ borderColor: "var(--border)", background: "var(--panel)" }}
              >
                <Sparkles size={22} style={{ color: "var(--muted)" }} />
              </div>
              <h2 className="text-[16px] font-medium">No skills found</h2>
              <p className="mt-1 max-w-md text-[12.5px]" style={{ color: "var(--muted)" }}>
                Add a skill as <code className="font-mono">&lt;name&gt;/SKILL.md</code> under the
                global skills directory, or create one with the built-in{" "}
                <code className="font-mono">manage-skill</code> skill.
              </p>
            </div>
          ) : (
            <div className="grid gap-2.5">
              {skills.map((skill) => {
                const isSaving = saving.has(skill.name);
                return (
                  <div
                    key={skill.name}
                    className="rounded-xl border px-4 py-3.5"
                    style={{
                      borderColor: "var(--border)",
                      background: "var(--panel)",
                      opacity: skill.enabled ? 1 : 0.78,
                    }}
                  >
                    <div className="flex items-start gap-3">
                      <div
                        className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border"
                        style={{
                          borderColor: "var(--border)",
                          background: "var(--panel-2)",
                          color: skill.enabled ? "var(--accent)" : "var(--muted)",
                        }}
                      >
                        <Sparkles size={15} />
                      </div>
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="text-[13.5px] font-medium">{skill.name}</span>
                          <span
                            className="rounded border px-1.5 py-0.5 text-[10px]"
                            style={{ borderColor: "var(--border)", color: "var(--muted)" }}
                          >
                            {sourceLabel(skill.source)}
                          </span>
                          {!skill.enabled ? (
                            <span
                              className="rounded border px-1.5 py-0.5 text-[10px]"
                              style={{ borderColor: "var(--border)", color: "var(--faint)" }}
                            >
                              disabled
                            </span>
                          ) : null}
                        </div>
                        <p className="mt-1 text-[11.5px] leading-relaxed" style={{ color: "var(--muted)" }}>
                          {skill.description || "No description."}
                        </p>
                        <div
                          className="mt-1.5 truncate font-mono text-[10.5px]"
                          style={{ color: "var(--faint)" }}
                          title={skill.path}
                        >
                          {skill.path}
                        </div>
                      </div>
                      <button
                        type="button"
                        role="switch"
                        aria-checked={skill.enabled}
                        aria-label={`Enable ${skill.name}`}
                        title={skill.enabled ? `Disable ${skill.name}` : `Enable ${skill.name}`}
                        disabled={isSaving}
                        onClick={() => void toggle(skill)}
                        className={cn(
                          "focus-ring relative mt-1 h-5 w-9 shrink-0 rounded-full transition disabled:cursor-wait disabled:opacity-60",
                          skill.enabled ? "bg-[var(--accent)]" : "bg-[var(--border)]",
                        )}
                      >
                        <span
                          className={cn(
                            "absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white shadow-sm transition-transform",
                            skill.enabled && "translate-x-4",
                          )}
                        />
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
