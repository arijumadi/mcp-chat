import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, Pencil, Plug, Plus, Trash2 } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { AcpAgentView, AcpPresetView, AcpProbe } from "@/lib/types";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass } from "@/components/ui";

function envToText(keys: string[]): string {
  // Values are never returned by the backend, so only the keys can be shown.
  return keys.map((key) => `${key}=`).join("\n");
}

function textToEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const index = trimmed.indexOf("=");
    if (index <= 0) continue;
    env[trimmed.slice(0, index).trim()] = trimmed.slice(index + 1).trim();
  }
  return env;
}

interface Draft {
  name: string;
  slug: string;
  command: string;
  args: string;
  env: string;
  permissionPolicy: string;
  enabled: boolean;
  /** True while editing an existing row, so env can be left untouched. */
  editing: boolean;
}

const EMPTY: Draft = {
  name: "",
  slug: "",
  command: "",
  args: "",
  env: "",
  permissionPolicy: "ask",
  enabled: true,
  editing: false,
};

export function AcpAgentsSettings() {
  const toast = useApp((state) => state.toast);
  const [agents, setAgents] = useState<AcpAgentView[]>([]);
  const [presets, setPresets] = useState<AcpPresetView[]>([]);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [probes, setProbes] = useState<Record<string, AcpProbe>>({});
  const [deleting, setDeleting] = useState<AcpAgentView | null>(null);

  const load = useCallback(async () => {
    try {
      const [list, available] = await Promise.all([
        call<AcpAgentView[]>("list_acp_agents"),
        call<AcpPresetView[]>("list_acp_presets"),
      ]);
      setAgents(list);
      setPresets(available);
    } catch (error) {
      toast(String(error), "error");
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const openPreset = (preset: AcpPresetView) => {
    setDraft({
      name: preset.name,
      slug: preset.slug,
      command: preset.command,
      args: preset.args.join(" "),
      env: "",
      permissionPolicy: "ask",
      enabled: true,
      editing: false,
    });
  };

  const openEdit = (agent: AcpAgentView) => {
    setDraft({
      name: agent.name,
      slug: agent.slug,
      command: agent.command,
      args: agent.args.join(" "),
      env: envToText(agent.envKeys),
      permissionPolicy: agent.permissionPolicy,
      enabled: agent.enabled,
      editing: true,
    });
  };

  const save = async () => {
    if (!draft) return;
    if (!draft.name.trim() || !draft.slug.trim() || !draft.command.trim()) {
      toast("Name, slug, and command are required.", "error");
      return;
    }
    setBusy("save");
    try {
      const args = draft.args.split(/\s+/).filter(Boolean);
      // While editing, an env box left as bare keys means "keep what is
      // stored": the backend only replaces env when a value is provided.
      const envText = draft.env.trim();
      const env =
        draft.editing && !envText.includes("=")
          ? undefined
          : textToEnv(draft.env);
      const input = {
        name: draft.name.trim(),
        slug: draft.slug.trim(),
        command: draft.command.trim(),
        args,
        env,
        permissionPolicy: draft.permissionPolicy,
        enabled: draft.enabled,
      };
      if (draft.editing) {
        await call("update_acp_agent", { id: agents.find((a) => a.slug === draft.slug)?.id, input });
      } else {
        await call("create_acp_agent", { input });
      }
      setDraft(null);
      await load();
      toast("Saved the ACP agent.", "info");
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setBusy(null);
    }
  };

  const toggle = async (agent: AcpAgentView) => {
    setBusy(agent.id);
    try {
      await call("set_acp_agent_enabled", { id: agent.id, enabled: !agent.enabled });
      await load();
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setBusy(null);
    }
  };

  const probe = async (agent: AcpAgentView) => {
    setBusy(agent.id);
    try {
      const result = await call<AcpProbe>("test_acp_agent", { id: agent.id });
      setProbes((current) => ({ ...current, [agent.id]: result }));
      await load();
    } catch (error) {
      setProbes((current) => ({
        ...current,
        [agent.id]: {
          ok: false,
          message: String(error),
          sessionId: null,
          agentName: null,
          agentVersion: null,
        },
      }));
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    if (!deleting) return;
    setBusy(deleting.id);
    try {
      await call("delete_acp_agent", { id: deleting.id });
      setDeleting(null);
      await load();
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button onClick={() => setDraft({ ...EMPTY })}>
          <Plus size={13} /> Add agent
        </Button>
        {presets.map((preset) => (
          <button
            key={preset.slug}
            type="button"
            onClick={() => openPreset(preset)}
            title={preset.notes}
            className="focus-ring rounded-lg border px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)]"
            style={{ borderColor: "var(--border)", color: "var(--muted)" }}
          >
            + {preset.name}
          </button>
        ))}
      </div>

      {agents.length === 0 ? (
        <p className="text-[11.5px]" style={{ color: "var(--muted)" }}>
          No ACP agents yet. Start from a preset above, or add one manually. An
          agent is a command that speaks ACP over stdio, such as{" "}
          <code>antigravity acp</code>.
        </p>
      ) : (
        <div className="space-y-2">
          {agents.map((agent) => {
            const result = probes[agent.id];
            return (
              <div
                key={agent.id}
                className="rounded-lg border p-3"
                style={{ borderColor: "var(--border)" }}
              >
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-[12.5px] font-medium">{agent.name}</span>
                      <code
                        className="rounded px-1.5 py-0.5 text-[10.5px]"
                        style={{ background: "var(--panel-2)", color: "var(--muted)" }}
                      >
                        {agent.slug}
                      </code>
                      {!agent.enabled ? (
                        <span className="text-[10.5px]" style={{ color: "var(--muted)" }}>
                          disabled
                        </span>
                      ) : null}
                    </div>
                    <div
                      className="mt-1 truncate font-mono text-[11px]"
                      style={{ color: "var(--muted)" }}
                    >
                      {[agent.command, ...agent.args].join(" ")}
                    </div>
                    <div className="mt-1 text-[10.5px]" style={{ color: "var(--muted)" }}>
                      Permissions:{" "}
                      {agent.permissionPolicy === "auto"
                        ? "auto-approve, except deletes"
                        : agent.permissionPolicy === "auto_reads"
                          ? "auto-approve reads"
                          : "always ask"}
                      {agent.envKeys.length > 0
                        ? ` · env: ${agent.envKeys.join(", ")}`
                        : ""}
                    </div>
                    {result ? (
                      <div
                        className="mt-1.5 flex items-start gap-1.5 text-[11px]"
                        style={{ color: result.ok ? "var(--accent)" : "#d9534f" }}
                      >
                        {result.ok ? <CheckCircle2 size={12} /> : <AlertTriangle size={12} />}
                        <span className="whitespace-pre-wrap">{result.message}</span>
                      </div>
                    ) : agent.lastError && agent.lastCheckedAt ? (
                      <div
                        className="mt-1.5 flex items-start gap-1.5 text-[11px]"
                        style={{ color: "#d9534f" }}
                      >
                        <AlertTriangle size={12} />
                        <span className="whitespace-pre-wrap">{agent.lastError}</span>
                      </div>
                    ) : null}
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    <button
                      type="button"
                      onClick={() => void probe(agent)}
                      disabled={busy === agent.id}
                      title="Start the agent and open a test session"
                      className="focus-ring rounded-md p-1.5 transition hover:bg-[var(--panel-2)]"
                      style={{ color: "var(--muted)" }}
                    >
                      <Plug size={13} />
                    </button>
                    <button
                      type="button"
                      onClick={() => void toggle(agent)}
                      disabled={busy === agent.id}
                      className="focus-ring rounded-md px-1.5 py-1 text-[10.5px] transition hover:bg-[var(--panel-2)]"
                      style={{ color: "var(--muted)" }}
                    >
                      {agent.enabled ? "Disable" : "Enable"}
                    </button>
                    <button
                      type="button"
                      onClick={() => openEdit(agent)}
                      className="focus-ring rounded-md p-1.5 transition hover:bg-[var(--panel-2)]"
                      style={{ color: "var(--muted)" }}
                    >
                      <Pencil size={13} />
                    </button>
                    <button
                      type="button"
                      onClick={() => setDeleting(agent)}
                      className="focus-ring rounded-md p-1.5 transition hover:bg-[var(--panel-2)]"
                      style={{ color: "var(--muted)" }}
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {draft ? (
        <Modal
          open
          title={draft.editing ? "Edit ACP agent" : "Add ACP agent"}
          onClose={() => setDraft(null)}
        >
          <div className="space-y-3">
            <label className="block text-[11.5px]">
              Name
              <input
                className={inputClass}
                value={draft.name}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
                placeholder="Antigravity"
              />
            </label>
            <label className="block text-[11.5px]">
              Slug
              <input
                className={inputClass}
                value={draft.slug}
                onChange={(event) => setDraft({ ...draft, slug: event.target.value })}
                placeholder="antigravity"
                disabled={draft.editing}
              />
              <span className="mt-1 block text-[10.5px]" style={{ color: "var(--muted)" }}>
                Used by the model in the <code>subagent</code> tool.
              </span>
            </label>
            <label className="block text-[11.5px]">
              Command
              <input
                className={inputClass}
                value={draft.command}
                onChange={(event) => setDraft({ ...draft, command: event.target.value })}
                placeholder="antigravity"
              />
            </label>
            <label className="block text-[11.5px]">
              Arguments
              <input
                className={inputClass}
                value={draft.args}
                onChange={(event) => setDraft({ ...draft, args: event.target.value })}
                placeholder="acp"
              />
            </label>
            <label className="block text-[11.5px]">
              Environment
              <textarea
                className={`${inputClass} h-20 font-mono text-[11px]`}
                value={draft.env}
                onChange={(event) => setDraft({ ...draft, env: event.target.value })}
                placeholder={"KEY=value"}
              />
              <span className="mt-1 block text-[10.5px]" style={{ color: "var(--muted)" }}>
                {draft.editing
                  ? "Stored values are never shown. Leave this untouched to keep them, or set a value to replace it."
                  : "Stored in the local database. Prefer the agent's own login when it has one."}
              </span>
            </label>
            <label className="block text-[11.5px]">
              Permissions
              <select
                className={inputClass}
                value={draft.permissionPolicy}
                onChange={(event) =>
                  setDraft({ ...draft, permissionPolicy: event.target.value })
                }
              >
                <option value="ask">Ask for every tool call</option>
                <option value="auto_reads">Auto-approve reads and searches</option>
                <option value="auto">
                  Auto-approve everything except deletes
                </option>
              </select>
            </label>
            <label className="flex items-center gap-2 text-[11.5px]">
              <input
                type="checkbox"
                checked={draft.enabled}
                onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })}
              />
              Enabled
            </label>
            <div className="flex justify-end gap-2 pt-1">
              <Button variant="ghost" onClick={() => setDraft(null)}>
                Cancel
              </Button>
              <Button onClick={() => void save()} disabled={busy === "save"}>
                Save
              </Button>
            </div>
          </div>
        </Modal>
      ) : null}

      {deleting ? (
        <Modal open title="Delete ACP agent" onClose={() => setDeleting(null)}>
          <p className="text-[12px]">
            Delete <strong>{deleting.name}</strong>? Chats currently bound to it
            will report that the agent is missing until you pick another one.
          </p>
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setDeleting(null)}>
              Cancel
            </Button>
            <Button variant="danger" onClick={() => void remove()}>
              Delete
            </Button>
          </div>
        </Modal>
      ) : null}
    </div>
  );
}
