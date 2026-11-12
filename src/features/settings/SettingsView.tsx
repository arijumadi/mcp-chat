import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  Check,
  ChevronDown,
  Database,
  ExternalLink,
  FileJson,
  FileText,
  KeyRound,
  List,
  Loader2,
  Monitor,
  Moon,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  ShieldCheck,
  Sun,
  Trash2,
} from "lucide-react";
import { useApp } from "@/app/store";
import { call, downloadText } from "@/lib/ipc";
import type { Conversation, ExportResult, KeyTest, ProviderView } from "@/lib/types";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass, hintClass } from "@/components/ui";
import { cn } from "@/lib/cn";
import { ProviderForm } from "./ProviderForm";
import { AppearanceSettings, ShortcutList } from "./AppearanceSettings";
import { NotificationSettings } from "./NotificationSettings";
import { DEFAULT_SYSTEM_PROMPT } from "@/lib/defaults";

export function SettingsView() {
  const appInfo = useApp((state) => state.appInfo);
  const updateInfo = useApp((state) => state.updateInfo);
  const updateChecking = useApp((state) => state.updateChecking);
  const updateError = useApp((state) => state.updateError);
  const checkForUpdate = useApp((state) => state.checkForUpdate);
  const providers = useApp((state) => state.providers);
  const settings = useApp((state) => state.settings);
  const refreshProviders = useApp((state) => state.refreshProviders);
  const refreshSettings = useApp((state) => state.refreshSettings);
  const refreshConversations = useApp((state) => state.refreshConversations);
  const conversations = useApp((state) => state.conversations);
  const conversationHasMore = useApp((state) => state.conversationHasMore);
  const loadMoreConversations = useApp((state) => state.loadMoreConversations);
  const activeId = useApp((state) => state.activeId);
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
  const [systemPrompt, setSystemPrompt] = useState(
    settings.default_system_prompt ?? DEFAULT_SYSTEM_PROMPT,
  );
  const [savingPrompt, setSavingPrompt] = useState(false);
  const [keepTokens, setKeepTokens] = useState(
    Number(settings["compaction.keep_tokens"] ?? 15000),
  );
  const [bufferTokens, setBufferTokens] = useState(
    Number(settings["compaction.buffer"] ?? 0),
  );
  const [savingContext, setSavingContext] = useState(false);
  const [savingUpdateSetting, setSavingUpdateSetting] = useState(false);
  const [exportId, setExportId] = useState(activeId ?? conversations[0]?.id ?? "");
  const compactionKeepSetting = settings["compaction.keep_tokens"];
  const compactionBufferSetting = settings["compaction.buffer"];

  useEffect(() => {
    setSystemPrompt(settings.default_system_prompt ?? DEFAULT_SYSTEM_PROMPT);
  }, [settings.default_system_prompt]);

  useEffect(() => {
    setExportId((current) => {
      if (conversations.some((conversation) => conversation.id === current)) {
        return current;
      }
      return activeId ?? conversations[0]?.id ?? "";
    });
  }, [activeId, conversations]);

  useEffect(() => {
    setKeepTokens(Number(compactionKeepSetting ?? 15000));
    setBufferTokens(Number(compactionBufferSetting ?? 0));
  }, [compactionKeepSetting, compactionBufferSetting]);

  const theme = (settings.theme as "system" | "light" | "dark") ?? "system";
  const checkOnStartup = settings["update.check_on_startup"] !== "0";

  const setUpdateCheckOnStartup = async (enabled: boolean) => {
    setSavingUpdateSetting(true);
    try {
      await call("set_setting", {
        key: "update.check_on_startup",
        value: enabled ? "1" : "0",
      });
      await refreshSettings();
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setSavingUpdateSetting(false);
    }
  };

  const openUpdateRelease = () => {
    if (!updateInfo?.url) return;
    void call("open_external", { url: updateInfo.url }).catch((error) =>
      toast(String(error), "error"),
    );
  };

  const test = async (provider: ProviderView) => {
    setTesting(provider.id);
    try {
      const result = await call<KeyTest>("test_provider", { providerId: provider.id });
      await refreshProviders();
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
      await refreshProviders();
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

  const saveContext = async () => {
    setSavingContext(true);
    try {
      await call("set_setting", {
        key: "compaction.keep_tokens",
        value: String(Math.max(1000, keepTokens)),
      });
      await call("set_setting", {
        key: "compaction.buffer",
        value: String(Math.max(0, bufferTokens)),
      });
      await refreshSettings();
      toast("Context settings saved.", "info");
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setSavingContext(false);
    }
  };

  const doExport = async (format: "markdown" | "json") => {
    if (!exportId) return;
    try {
      const result = await call<ExportResult>("export_conversation", {
        conversationId: exportId,
        format,
      });
      downloadText(result.filename, result.content, result.mime);
      toast(`Exported ${result.filename}`, "info");
    } catch (error) {
      toast(String(error), "error");
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
          <SettingsNav />
          <Section
            id="providers"
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
                            className="rounded border px-1.5 py-0.5 text-[10.5px]"
                            style={{ borderColor: "var(--border)", color: "var(--muted)" }}
                          >
                            {provider.kind === "anthropic" ? "Anthropic" : "OpenAI-compatible"}
                          </span>
                          {provider.hasKey ? (
                            <span
                              className="flex items-center gap-1 text-[10.5px]"
                              style={{ color: "var(--accent)" }}
                              title={`Keychain: ${provider.keyHint ?? "saved"}`}
                            >
                              <KeyRound size={10} /> key
                            </span>
                          ) : (
                            <span className="text-[10.5px]" style={{ color: "var(--faint)" }}>
                              no key
                            </span>
                          )}
                        </div>
                        <div className="mt-0.5 truncate font-mono text-[10.5px]" style={{ color: "var(--faint)" }}>
                          {provider.baseUrl}
                        </div>
                        <div className="mt-1 text-[11.5px]" style={{ color: "var(--muted)" }}>
                          {provider.model || "no model"} ·{" "}
                          {(provider.contextWindow / 1000).toFixed(0)}k context · max{" "}
                          {provider.maxOutputTokens} out · {provider.models.length} saved model
                          {provider.models.length === 1 ? "" : "s"}
                        </div>
                      </div>
                      <div className="flex shrink-0 items-center gap-0.5">
                        <button
                          type="button"
                          onClick={() => void test(provider)}
                          disabled={testing === provider.id}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)] disabled:opacity-45"
                          title="Test API key"
                          aria-label="Test API key"
                        >
                          {testing === provider.id ? (
                            <Loader2 size={14} className="animate-spin" />
                          ) : (
                            <KeyRound size={14} />
                          )}
                        </button>
                        <button
                          type="button"
                          onClick={() => void fetchModels(provider)}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
                          title="Fetch models"
                          aria-label="Fetch models"
                        >
                          <List size={14} />
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setEditing(provider);
                            setFormOpen(true);
                          }}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
                          title="Edit provider"
                          aria-label="Edit provider"
                        >
                          <Pencil size={13} />
                        </button>
                        <button
                          type="button"
                          onClick={() => setDeleting(provider)}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--danger-soft)] hover:text-[var(--danger)]"
                          title="Delete provider"
                          aria-label="Delete provider"
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
            id="appearance"
            title="Appearance"
            description="Theme, density, text size and chat width. The interface follows the system theme by default."
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
            <AppearanceSettings />
          </Section>

          <Section
            id="notifications"
            title="Notifications"
            description="Know when a long task needs you, without watching the window."
          >
            <NotificationSettings />
          </Section>

          <Section
            id="shortcuts"
            title="Keyboard shortcuts"
            description="Press Ctrl+Shift+P (Cmd+Shift+P on macOS) to search every command."
          >
            <ShortcutList />
          </Section>

          <Section
            id="chat-defaults"
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
            <span className={hintClass} style={{ color: "var(--faint)" }}>
              Each user message is stamped with its local send time in minute
              precision. Time lives in the user message, not this prompt, so the
              system/tool prefix stays byte-identical for prompt caching.
            </span>
            <div className="mt-2 flex justify-end gap-2">
              <Button
                size="sm"
                variant="ghost"
                onClick={() => setSystemPrompt(DEFAULT_SYSTEM_PROMPT)}
              >
                Reset to default
              </Button>
              <Button size="sm" loading={savingPrompt} onClick={() => void savePrompt()}>
                Save prompt
              </Button>
            </div>
          </Section>

          <Section
            id="context"
            title="Context management"
            description="Compaction summarizes older turns and keeps the most recent tokens verbatim."
          >
            <div className="grid grid-cols-2 gap-3">
              <label className="block">
                <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                  Keep recent tokens
                </span>
                <input
                  type="number"
                  min={1000}
                  step={1000}
                  value={keepTokens}
                  onChange={(event) => setKeepTokens(Number(event.target.value))}
                  className={inputClass}
                  style={{ borderColor: "var(--border)" }}
                />
              </label>
              <label className="block">
                <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                  Compact buffer
                </span>
                <input
                  type="number"
                  min={0}
                  step={1000}
                  value={bufferTokens}
                  onChange={(event) => setBufferTokens(Number(event.target.value))}
                  className={inputClass}
                  style={{ borderColor: "var(--border)" }}
                />
              </label>
            </div>
            <span className={hintClass} style={{ color: "var(--faint)" }}>
              Default keep is 15,000 tokens. Buffer 0 means automatic: 15% of the
              context window, at least 16,000 tokens.
            </span>
            <div className="mt-2 flex justify-end">
              <Button size="sm" loading={savingContext} onClick={() => void saveContext()}>
                Save context settings
              </Button>
            </div>
          </Section>

          <Section
            id="export"
            title="Export conversation"
            description="Save a transcript as Markdown or JSON. Conversations can be selected independently of the open chat."
          >
            {conversations.length === 0 ? (
              <div
                className="rounded-lg border px-3 py-2 text-[12.5px]"
                style={{ borderColor: "var(--border)", color: "var(--muted)" }}
              >
                No conversations to export.
              </div>
            ) : (
              <>
                <label className="block">
                  <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                    Conversation
                  </span>
                  <ConversationSelect
                    conversations={conversations}
                    value={exportId}
                    onChange={setExportId}
                    hasMore={conversationHasMore}
                    onLoadMore={() => void loadMoreConversations()}
                  />
                </label>
                <div className="mt-2 flex justify-end gap-2">
                  <Button
                    size="sm"
                    icon={<FileText size={12} />}
                    onClick={() => void doExport("markdown")}
                  >
                    Markdown
                  </Button>
                  <Button
                    size="sm"
                    icon={<FileJson size={12} />}
                    onClick={() => void doExport("json")}
                  >
                    JSON
                  </Button>
                </div>
              </>
            )}
          </Section>

          <Section id="data" title="Data" description="Everything is stored locally in this app's data directory.">
            <div
              className="flex flex-wrap items-center gap-2 rounded-lg border px-3 py-2.5"
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <Database size={14} style={{ color: "var(--muted)" }} />
              <code className="min-w-0 flex-1 truncate font-mono text-[11.5px]" style={{ color: "var(--muted)" }}>
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

          <Section
            id="about"
            title="About"
            description="Version information and manual update checks. Jarvis does not download or install an update without you."
          >
            <div className="flex items-start gap-2 text-[12.5px]" style={{ color: "var(--muted)" }}>
              <ShieldCheck size={15} className="mt-0.5 shrink-0" style={{ color: "var(--accent)" }} />
              <div>
                Jarvis {appInfo?.version ?? "0.1.0"} · no telemetry, no accounts, no cloud sync.
                Message content is not logged; API keys live in the OS keychain.
              </div>
            </div>

            <div
              className="mt-4 rounded-xl border px-3.5 py-3"
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <div className="flex flex-wrap items-center gap-3">
                <div className="min-w-0 flex-1">
                  <div className="text-[12.5px] font-medium">
                    Jarvis {appInfo?.version ?? "0.1.0"}
                  </div>
                  <div className="text-[11.5px]" style={{ color: "var(--muted)" }}>
                    Update checks read the public GitHub release page. Nothing is downloaded
                    automatically.
                  </div>
                </div>
                <Button
                  size="sm"
                  variant="secondary"
                  icon={<RefreshCw size={12} />}
                  loading={updateChecking}
                  onClick={() => void checkForUpdate(true)}
                >
                  Check for updates
                </Button>
              </div>

              <label className="mt-3 flex items-center gap-2 text-[11.5px]" style={{ color: "var(--muted)" }}>
                <input
                  type="checkbox"
                  checked={checkOnStartup}
                  disabled={savingUpdateSetting}
                  onChange={(event) => void setUpdateCheckOnStartup(event.target.checked)}
                  style={{ accentColor: "var(--accent)" }}
                />
                Check for updates on startup
              </label>

              {updateInfo?.available ? (
                <div
                  className="mt-3 rounded-lg border px-3 py-3"
                  style={{ borderColor: "var(--accent)", background: "var(--panel-2)" }}
                >
                  <div className="flex flex-wrap items-start gap-2">
                    <div className="min-w-0 flex-1">
                      <div className="text-[12.5px] font-medium">
                        Jarvis {updateInfo.latestVersion} is available
                      </div>
                      <div className="mt-0.5 text-[11.5px]" style={{ color: "var(--muted)" }}>
                        You are on {updateInfo.currentVersion}. Review the release, then download
                        and install it manually.
                      </div>
                    </div>
                    <Button
                      size="sm"
                      variant="primary"
                      icon={<ExternalLink size={12} />}
                      onClick={openUpdateRelease}
                    >
                      View release
                    </Button>
                  </div>
                  {updateInfo.notes ? (
                    <details className="mt-2">
                      <summary
                        className="cursor-pointer text-[11px]"
                        style={{ color: "var(--faint)" }}
                      >
                        Release notes
                      </summary>
                      <pre
                        className="mt-1 max-h-56 overflow-auto rounded-md p-2 font-mono text-[10.5px] leading-relaxed whitespace-pre-wrap"
                        style={{ background: "var(--code)", color: "var(--muted)" }}
                      >
                        {updateInfo.notes}
                      </pre>
                    </details>
                  ) : null}
                </div>
              ) : updateInfo ? (
                <div
                  className="mt-2 text-[11.5px]"
                  role="status"
                  style={{ color: "var(--accent)" }}
                >
                  You're up to date.
                </div>
              ) : null}

              {updateError ? (
                <div
                  className="mt-2 rounded-lg px-2.5 py-2 text-[11.5px]"
                  role="alert"
                  style={{ background: "var(--danger-soft)", color: "var(--danger)" }}
                >
                  {updateError}
                </div>
              ) : null}
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
        <p className="text-[13.5px]" style={{ color: "var(--muted)" }}>
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
        <p className="mb-3 text-[13.5px]" style={{ color: "var(--muted)" }}>
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

function ConversationSelect({
  conversations,
  value,
  onChange,
  hasMore,
  onLoadMore,
}: {
  conversations: Conversation[];
  value: string;
  onChange: (id: string) => void;
  hasMore: boolean;
  onLoadMore: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const ref = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const selected = conversations.find((conversation) => conversation.id === value);
  const filtered = conversations.filter((conversation) =>
    conversation.title.toLowerCase().includes(query.trim().toLowerCase()),
  );

  useEffect(() => {
    if (!open) return;
    setQuery("");
    const onPointer = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative">
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen((current) => !current)}
        className={cn(inputClass, "flex w-full items-center justify-between gap-2 text-left")}
        style={{ borderColor: "var(--border)", background: "var(--panel)" }}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label="Export conversation"
      >
        <span className="min-w-0 truncate">{selected?.title ?? "Choose a conversation"}</span>
        <ChevronDown size={14} className="shrink-0" style={{ color: "var(--faint)" }} />
      </button>
      {open ? (
        <div
          className="absolute z-30 mt-1 w-full rounded-xl border p-1.5 shadow-xl"
          style={{ borderColor: "var(--border)", background: "var(--panel)" }}
        >
          <div className="relative mb-1.5">
            <Search
              size={13}
              className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2"
              style={{ color: "var(--faint)" }}
            />
            <input
              autoFocus
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className={cn(inputClass, "h-8 py-1 pl-8 text-[12.5px]")}
              style={{ borderColor: "var(--border)" }}
              placeholder="Search conversations…"
              aria-label="Search export conversations"
            />
          </div>
          <div className="max-h-60 overflow-y-auto" role="listbox" aria-label="Conversations">
            {filtered.length === 0 ? (
              <div className="px-2 py-2 text-[11.5px]" style={{ color: "var(--faint)" }}>
                No matching conversations.
              </div>
            ) : (
              filtered.map((conversation) => {
                const active = conversation.id === value;
                return (
                  <button
                    key={conversation.id}
                    type="button"
                    role="option"
                    aria-selected={active}
                    onClick={() => {
                      onChange(conversation.id);
                      setOpen(false);
                      triggerRef.current?.focus();
                    }}
                    className="focus-ring flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[12.5px] transition hover:bg-[var(--panel-2)]"
                    style={{ background: active ? "var(--panel-2)" : "transparent" }}
                  >
                    <span className="min-w-0 flex-1 truncate">{conversation.title}</span>
                    {active ? <Check size={12} style={{ color: "var(--accent)" }} /> : null}
                  </button>
                );
              })
            )}
          </div>
          {hasMore ? (
            <button
              type="button"
              onClick={onLoadMore}
              className="focus-ring mt-1 w-full rounded-lg px-2 py-1.5 text-left text-[11px] font-medium transition hover:bg-[var(--panel-2)]"
              style={{ color: "var(--accent)" }}
            >
              Load older conversations…
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function SettingsNav() {
  const items = [
    ["providers", "Providers"],
    ["appearance", "Appearance"],
    ["notifications", "Notifications"],
    ["shortcuts", "Shortcuts"],
    ["chat-defaults", "Chat defaults"],
    ["context", "Context"],
    ["export", "Export"],
    ["data", "Data"],
    ["about", "About"],
  ] as const;
  return (
    <nav
      className="sticky top-0 z-10 -mx-6 mb-5 flex gap-1 overflow-x-auto border-b px-6 py-2"
      style={{ background: "var(--bg)", borderColor: "var(--border)" }}
      aria-label="Settings sections"
    >
      {items.map(([id, label]) => (
        <button
          key={id}
          type="button"
          onClick={() =>
            document.getElementById(id)?.scrollIntoView({
              behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches
                ? "auto"
                : "smooth",
              block: "start",
            })
          }
          className="focus-ring shrink-0 rounded-md px-2 py-1 text-[11.5px] transition hover:bg-[var(--panel-2)]"
          style={{ color: "var(--muted)" }}
        >
          {label}
        </button>
      ))}
    </nav>
  );
}

function Section({
  id,
  title,
  description,
  action,
  children,
}: {
  id?: string;
  title: string;
  description: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section id={id} className="mb-8 scroll-mt-14">
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
