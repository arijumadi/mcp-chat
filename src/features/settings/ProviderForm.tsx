import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { ProviderInput, ProviderView } from "@/lib/types";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass, hintClass } from "@/components/ui";

export function ProviderForm({
  provider,
  open,
  onClose,
}: {
  provider: ProviderView | null;
  open: boolean;
  onClose: () => void;
}) {
  const refreshProviders = useApp((state) => state.refreshProviders);
  const toast = useApp((state) => state.toast);
  const [form, setForm] = useState<ProviderInput>({
    name: "",
    kind: "openai_compat",
    baseUrl: "",
    model: "",
    contextWindow: 128000,
    maxOutputTokens: 8192,
    apiKey: "",
  });
  const [models, setModels] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);
  const [fetching, setFetching] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setError(null);
    setModels(provider?.models ?? []);
    if (provider) {
      setForm({
        name: provider.name,
        kind: provider.kind,
        baseUrl: provider.baseUrl,
        model: provider.model,
        contextWindow: provider.contextWindow,
        maxOutputTokens: provider.maxOutputTokens,
        apiKey: "",
      });
    } else {
      setForm({
        name: "",
        kind: "openai_compat",
        baseUrl: "https://api.openai.com/v1",
        model: "",
        contextWindow: 128000,
        maxOutputTokens: 8192,
        apiKey: "",
      });
    }
  }, [open, provider]);

  const set = <K extends keyof ProviderInput>(key: K, value: ProviderInput[K]) =>
    setForm((current) => ({ ...current, [key]: value }));

  const fetchModels = async () => {
    if (!form.baseUrl.trim()) {
      toast("Fill in the base URL first.", "error");
      return;
    }
    setFetching(true);
    try {
      const list = await call<string[]>("fetch_models_for_config", {
        kind: form.kind,
        baseUrl: form.baseUrl.trim(),
        apiKey: form.apiKey?.trim() || undefined,
        providerId: provider?.id,
      });
      setModels(list);
      if (!form.model.trim() && list.length > 0) set("model", list[0] ?? "");
      toast(`Fetched ${list.length} model(s).`, "info");
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setFetching(false);
    }
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const input = {
        ...form,
        apiKey: form.apiKey?.trim() || undefined,
        models,
      };
      if (provider) {
        await call("update_provider", { providerId: provider.id, input });
      } else {
        await call("create_provider", { input });
      }
      await refreshProviders();
      onClose();
    } catch (error) {
      setError(String(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      width="md"
      title={provider ? "Edit provider" : "Add provider"}
      subtitle="The API key is stored in the OS keychain and never returned to the webview."
      footer={
        <>
          <Button size="sm" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            variant="primary"
            loading={saving}
            onClick={() => void save()}
            disabled={!form.name.trim() || !form.baseUrl.trim()}
          >
            {provider ? "Save changes" : "Add provider"}
          </Button>
        </>
      }
    >
      <div className="grid gap-3.5">
        <label className="block">
          <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
            Name
          </span>
          <input
            autoFocus
            value={form.name}
            onChange={(event) => set("name", event.target.value)}
            placeholder="OpenAI, local Ollama, work gateway…"
            className={inputClass}
            style={{ borderColor: "var(--border)" }}
          />
        </label>

        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              Kind
            </span>
            <select
              value={form.kind}
              onChange={(event) => set("kind", event.target.value)}
              className={inputClass}
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <option value="openai_compat">OpenAI-compatible</option>
              <option value="anthropic">Anthropic</option>
            </select>
          </label>
          <label className="block">
            <span className="mb-1 flex items-center justify-between text-[11.5px]" style={{ color: "var(--muted)" }}>
              <span>Model (optional)</span>
              <button
                type="button"
                onClick={() => void fetchModels()}
                className="focus-ring flex items-center gap-1 rounded px-1 text-[10.5px] hover:bg-[var(--panel-2)]"
                style={{ color: "var(--muted)" }}
              >
                <RefreshCw size={10} className={fetching ? "animate-spin" : ""} /> Fetch models
              </button>
            </span>
            <input
              value={form.model}
              onChange={(event) => set("model", event.target.value)}
              list="provider-models"
              placeholder={form.kind === "anthropic" ? "claude-sonnet-4-5 (optional)" : "gpt-4.1 (optional)"}
              className={inputClass}
              style={{ borderColor: "var(--border)" }}
            />
            <datalist id="provider-models">
              {models.map((model) => (
                <option key={model} value={model} />
              ))}
            </datalist>
            <span className={hintClass} style={{ color: "var(--faint)" }}>
              Optional. Fetch once and the list is saved with this provider; choose the model per chat.
            </span>
          </label>
        </div>

        <label className="block">
          <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
            Base URL
          </span>
          <input
            value={form.baseUrl}
            onChange={(event) => set("baseUrl", event.target.value)}
            placeholder={
              form.kind === "anthropic"
                ? "https://api.anthropic.com"
                : "https://api.openai.com/v1"
            }
            className={inputClass}
            style={{ borderColor: "var(--border)" }}
          />
          <span className={hintClass} style={{ color: "var(--faint)" }}>
            OpenAI-compatible: include <code>/v1</code> if your endpoint expects it. Anthropic:
            use the root origin.
          </span>
        </label>

        <label className="block">
          <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
            API key
          </span>
          <input
            type="password"
            value={form.apiKey ?? ""}
            onChange={(event) => set("apiKey", event.target.value)}
            placeholder={
              provider?.hasKey ? `Saved (${provider.keyHint ?? "••••"}) — leave blank to keep` : "sk-…"
            }
            className={inputClass}
            style={{ borderColor: "var(--border)" }}
            autoComplete="off"
          />
          <span className={hintClass} style={{ color: "var(--faint)" }}>
            Local endpoints may not need a key. The value is sent only to this provider.
          </span>
        </label>

        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              Context window
            </span>
            <input
              type="number"
              min={4000}
              step={1000}
              value={form.contextWindow}
              onChange={(event) => set("contextWindow", Number(event.target.value))}
              className={inputClass}
              style={{ borderColor: "var(--border)" }}
            />
          </label>
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              Max output tokens
            </span>
            <input
              type="number"
              min={256}
              step={256}
              value={form.maxOutputTokens}
              onChange={(event) => set("maxOutputTokens", Number(event.target.value))}
              className={inputClass}
              style={{ borderColor: "var(--border)" }}
            />
          </label>
        </div>

        {error ? (
          <div
            className="rounded-lg border px-3 py-2 text-[12.5px]"
            style={{ borderColor: "var(--danger)", color: "var(--danger)", background: "var(--danger-soft)" }}
          >
            {error}
          </div>
        ) : null}
      </div>
    </Modal>
  );
}
