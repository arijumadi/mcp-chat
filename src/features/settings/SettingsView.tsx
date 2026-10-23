import { useEffect, useState } from "react";
import {
  ArrowLeft,
  Database,
  ExternalLink,
  KeyRound,
  Monitor,
  Moon,
  Pencil,
  Plus,
  ShieldCheck,
  Sun,
  Trash2,
} from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { KeyTest, ProviderView } from "@/lib/types";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass, hintClass } from "@/components/ui";
import { cn } from "@/lib/cn";
import { ProviderForm } from "./ProviderForm";

export function SettingsView() {
  const appInfo = useApp((state) => state.appInfo);
  const providers = useApp((state) => state.providers);
  const settings = useApp((state) => state.settings);
  const refreshProviders = useApp((state) => state.refreshProviders);
  const refreshSettings = useApp((state) => state.refreshSettings);
  const refreshConversations = useApp((state) => state.refreshConversations);
  const setTheme = useApp((state) => state.setTheme);
  const setView = useApp((state) => state.setView);
  const toast = useApp((state) => state.toast);

  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<ProviderView | null>(null);
  const [deleting, setDeleting] = useState<ProviderView | null>(null);
  const [models, setModels] = useState<{ name: string; models: string[] } | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [deleteAllOpen, setDeleteAllOpen] = useState(false);
  const [deleteConfirm, setDeleteConfirm] = useState("");
  const [systemPrompt, setSystemPrompt] = useState(settings.default_system_prompt ?? "");
  const [savingPrompt, setSavingPrompt] = useState(false);

  useEffect(() => {
    setSystemPrompt(settings.default_system_prompt ?? "");
  }, [settings.default_system_prompt]);

  const theme = (settings.theme as "system" | "light" | "dark") ?? "system";

  const test = async (provider: ProviderView) => {
    setTesting(provider.id);
    try {
      const result = await call<KeyTest>("test_provider", { providerId: provider.id });
      toast(result.message, result.ok ? "info" : "error");
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setTesting(null);
    }
  };

  const fetchModels = async (provider: ProviderView) => {
    try {
      const list = await call<string[]>("fetch_models", { providerId: provider.id });
      setModels({ name: provider.name, models: list });
    } catch (error) {
      toast(String(error), "error");
    }
  };

  const deleteProvider = async () => {
    if (!deleting) return;
    try {
      await call("delete_provider", { providerId: deleting.id });
      setDeleting(null);
      await refreshProviders();
      toast("Provider removed.", "info");
    } catch (error) {
      toast(String(error), "error");
    }
  };

  const savePrompt = async () => {
    setSavingPrompt(true);
    try {
      await call("set_setting", { key: "default_system_prompt", value: systemPrompt });
      await refreshSettings();
      toast("Default system prompt saved.", "info");
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setSavingPrompt(false);
    }
  };

  const deleteAll = async () => {
    if (deleteConfirm !== "DELETE") return;
    try {
      await call("delete_all_data");
      setDeleteAllOpen(false);
      setDeleteConfirm("");
      await Promise.all([refreshProviders(), refreshSettings(), refreshConversations()]);
      toast("All local data and keys were deleted.", "info");
      setView("chat");
    } catch (error) {
      toast(String(error), "error");
    }
  };

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
          <div className="text-[14px] font-medium">Settings</div>
          <div className="text-[10.5px]" style={{ color: "var(--faint)" }}>
            providers, appearance and local data
          </div>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[820px] px-6 py-7">
          <Section
            title="Providers"
            description="Models are called from Rust. API keys stay in the OS keychain and are never exposed to the webview."
            action={
              <Button
                size="sm"
                variant="primary"
                icon={<Plus size={13} />}
                onClick={() => {
                  setEditing(null);
                  setFormOpen(true);
                }}
              >
                Add provider
              </Button>
            }
          >
            <div className="grid gap-2">
              {providers.length === 0 ? (
                <div
                  className="rounded-xl border px-4 py-6 text-center text-[12.5px]"
                  style={{ borderColor: "var(--border)", color: "var(--muted)" }}
                >
                  No providers yet. Add an OpenAI-compatible endpoint or Anthropic to start chatting.
                </div>
              ) : (
                providers.map((provider) => (
                  <div
                    key={provider.id}
                    className="rounded-xl border px-4 py-3"
                    style={{ borderColor: "var(--border)", background: "var(--panel)" }}
                  >
                    <div className="flex flex-wrap items-start gap-x-3 gap-y-1">
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className="truncate text-[13.5px] font-medium">{provider.name}</span>
                          <span
                            className="rounded border px-1.5 py-0.5 text-[10px]"
                            style={{ borderColor: "var(--border)", color: "var(--muted)" }}
                          >
                            {provider.kind === "anthropic" ? "Anthropic" : "OpenAI-compatible"}
                          </span>
                          {provider.hasKey ? (
                            <span
                              className="flex items-center gap-1 text-[10px]"
                              style={{ color: "var(--accent)" }}
                              title={`Keychain: ${provider.keyHint ?? "saved"}`}
                            >
                              <KeyRound size={10} /> key
                            </span>
                          ) : (
                            <span className="text-[10px]" style={{ color: "var(--faint)" }}>
                              no key
                            </span>
                          )}
                        </div>
                        <div className="mt-0.5 truncate font-mono text-[10.5px]" style={{ color: "var(--faint)" }}>
                          {provider.baseUrl}
                        </div>
                        <div className="mt-1 text-[11.5px]" style={{ color: "var(--muted)" }}>
                          {provider.model} · {(provider.contextWindow / 1000).toFixed(0)}k context ·
                          max {provider.maxOutputTokens} out
                        </div>
                      </div>
                      <div className="flex items-center gap-1">
                        <Button
                          size="sm"
                          variant="ghost"
                          loading={testing === provider.id}
                          onClick={() => void test(provider)}
                        >
                          Test key
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => void fetchModels(provider)}>
                          Models
                        </Button>
                        <button
                          onClick={() => {
                            setEditing(provider);
                            setFormOpen(true);
                          }}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
                          title="Edit provider"
                        >
                          <Pencil size={13} />
                        </button>
                        <button
                          onClick={() => setDeleting(provider)}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--danger-soft)] hover:text-[var(--danger)]"
                          title="Delete provider"
                        >
                          <Trash2 size={13} />
                        </button>
                      </div>
                    </div>
                  </div>
                ))
              )}
            </div>
          </Section>

          <Section
            title="Appearance"
            description="The interface follows the system theme by default."
          >
            <div className="flex flex-wrap gap-2">
              {(
                [
                  ["system", Monitor, "System"],
                  ["light", Sun, "Light"],
                  ["dark", Moon, "Dark"],
                ] as const
              ).map(([value, Icon, label]) => (
                <button
                  key={value}
                  onClick={() => void setTheme(value)}
                  className={cn(
                    "focus-ring flex items-center gap-2 rounded-lg border px-3 py-2 text-[12.5px] transition",
                    theme === value ? "bg-[var(--panel-2)]" : "hover:bg-[var(--panel-2)]",
                  )}
                  style={{
                    borderColor: theme === value ? "var(--accent)" : "var(--border)",
                  }}
                >
                  <Icon size={14} /> {label}
                </button>
              ))}
            </div>
          </Section>

          <Section
            title="Chat defaults"
            description="Applied to new conversations that do not set their own system prompt."
          >
            <textarea
              value={systemPrompt}
              onChange={(event) => setSystemPrompt(event.target.value)}
              placeholder="You are a concise assistant…"
              className={inputClass}
              style={{ borderColor: "var(--border)", minHeight: 96 }}
            />
            <div className="mt-2 flex justify-end">
              <Button size="sm" loading={savingPrompt} onClick={() => void savePrompt()}>
                Save prompt
              </Button>
            </div>
          </Section>

          <Section title="Data" description="Everything is stored locally in this app's data directory.">
            <div
              className="flex flex-wrap items-center gap-2 rounded-lg border px-3 py-2.5"
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <Database size={14} style={{ color: "var(--muted)" }} />
              <code className="min-w-0 flex-1 truncate font-mono text-[11px]" style={{ color: "var(--muted)" }}>
                {appInfo?.dataDir ?? "loading…"}
              </code>
              <Button
                size="sm"
                variant="ghost"
                icon={<ExternalLink size={12} />}
                onClick={() => void call("open_data_dir").catch((error) => toast(String(error), "error"))}
              >
                Open
              </Button>
            </div>
            <div className="mt-4 flex flex-wrap items-center justify-between gap-3">
              <div>
                <div className="text-[12.5px] font-medium">Delete all data</div>
                <div className="text-[11.5px]" style={{ color: "var(--muted)" }}>
                  Conversations, tool output, providers and keychain entries.
                </div>
              </div>
              <Button
                size="sm"
                variant="danger"
                icon={<Trash2 size={12} />}
                onClick={() => setDeleteAllOpen(true)}
              >
                Delete everything
              </Button>
            </div>
          </Section>

          <Section title="About" description="">
            <div className="flex items-start gap-2 text-[12px]" style={{ color: "var(--muted)" }}>
              <ShieldCheck size={15} className="mt-0.5 shrink-0" style={{ color: "var(--accent)" }} />
              <div>
                MCP Chat {appInfo?.version ?? "0.1.0"} · no telemetry, no accounts, no cloud sync.
                Message content is not logged; API keys live in the OS keychain.
              </div>
            </div>
          </Section>
        </div>
      </div>

      <ProviderForm
        provider={editing}
        open={formOpen}
        onClose={() => {
          setFormOpen(false);
          setEditing(null);
        }}
      />

      <Modal
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        title="Delete provider?"
        width="sm"
        footer={
          <>
            <Button size="sm" variant="ghost" onClick={() => setDeleting(null)}>
              Cancel
            </Button>
            <Button size="sm" variant="danger" onClick={() => void deleteProvider()}>
              Delete
            </Button>
          </>
        }
      >
        <p className="text-[13px]" style={{ color: "var(--muted)" }}>
          The API key will be removed from the OS keychain. Conversations that used this provider
          will need a new model selected.
        </p>
      </Modal>

      <Modal
        open={models !== null}
        onClose={() => setModels(null)}
        title={`Models · ${models?.name ?? ""}`}
        width="md"
      >
        <div className="grid gap-1">
          {models?.models.length ? (
            models.models.map((model) => (
              <div
                key={model}
                className="rounded-lg border px-3 py-1.5 font-mono text-[11.5px]"
                style={{ borderColor: "var(--border)", background: "var(--panel-2)" }}
              >
                {model}
              </div>
            ))
          ) : (
            <div className="text-[12.5px]" style={{ color: "var(--muted)" }}>
              The endpoint returned no models. Enter one manually in the provider form.
            </div>
          )}
        </div>
      </Modal>

      <Modal
        open={deleteAllOpen}
        onClose={() => setDeleteAllOpen(false)}
        title="Delete all local data?"
        subtitle="This cannot be undone. Type DELETE to confirm."
        width="sm"
        footer={
          <>
            <Button size="sm" variant="ghost" onClick={() => setDeleteAllOpen(false)}>
              Cancel
            </Button>
            <Button
              size="sm"
              variant="danger"
              disabled={deleteConfirm !== "DELETE"}
              onClick={() => void deleteAll()}
            >
              Delete everything
            </Button>
          </>
        }
      >
        <p className="mb-3 text-[13px]" style={{ color: "var(--muted)" }}>
          Conversations, messages, tool output, providers, MCP server configs, permission rules
          and keychain entries will be removed.
        </p>
        <input
          value={deleteConfirm}
          onChange={(event) => setDeleteConfirm(event.target.value)}
          placeholder="DELETE"
          className={inputClass}
          style={{ borderColor: "var(--border)" }}
        />
        <span className={hintClass} style={{ color: "var(--faint)" }}>
          Keys are removed from the OS keychain. Operating-system backups are out of scope.
        </span>
      </Modal>
    </div>
  );
}

function Section({
  title,
  description,
  action,
  children,
}: {
  title: string;
  description: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="mb-8">
      <div className="mb-2.5 flex items-start justify-between gap-3">
        <div>
          <h2 className="text-[14px] font-semibold tracking-[-0.01em]">{title}</h2>
          {description ? (
            <p className="mt-0.5 max-w-[560px] text-[11.5px]" style={{ color: "var(--muted)" }}>
              {description}
            </p>
          ) : null}
        </div>
        {action}
      </div>
      {children}
    </section>
  );
}
