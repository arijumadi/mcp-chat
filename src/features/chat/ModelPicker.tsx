import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { inputClass } from "@/components/ui";
import type { Conversation } from "@/lib/types";
import { shortModelName } from "@/lib/format";

export function ModelPicker({
  conversation,
  disabled = false,
}: {
  conversation: Conversation;
  disabled?: boolean;
}) {
  const providers = useApp((state) => state.providers);
  const applyModel = useApp((state) => state.setModel);
  const setView = useApp((state) => state.setView);
  const [open, setOpen] = useState(false);
  const [providerId, setProviderId] = useState(conversation.providerId ?? "");
  const [selectedModel, setSelectedModel] = useState(conversation.model ?? "");
  const [modelQuery, setModelQuery] = useState("");
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setProviderId(conversation.providerId ?? providers[0]?.id ?? "");
    setSelectedModel(conversation.model ?? providers[0]?.model ?? "");
  }, [conversation.providerId, conversation.model, providers]);

  useEffect(() => {
    if (open) setModelQuery("");
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onClick = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  const provider = providers.find((item) => item.id === providerId);
  const models = provider?.models ?? [];
  const usesCustom = models.length === 0 || !models.includes(selectedModel);
  const filteredModels = models.filter((model) => {
    const needle = modelQuery.trim().toLowerCase();
    return (
      !needle ||
      model.toLowerCase().includes(needle) ||
      shortModelName(model).toLowerCase().includes(needle)
    );
  });

  const save = async () => {
    if (!providerId || !selectedModel.trim()) return;
    await applyModel(providerId, selectedModel.trim());
    setOpen(false);
  };

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((value) => !value)}
        disabled={disabled}
        className="focus-ring flex max-w-[260px] items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)] disabled:cursor-not-allowed disabled:opacity-50"
        style={{ borderColor: "var(--border)", color: "var(--muted)" }}
        title="Change model"
      >
        <span className="truncate font-medium" style={{ color: "var(--fg)" }}>
          {provider
            ? shortModelName(conversation.model ?? provider.model ?? "model")
            : "No provider"}
        </span>
        {provider &&
        !provider.hasKey &&
        !/^(https?:\/\/)?(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\])/i.test(
          provider.baseUrl,
        ) ? (
          <span
            className="h-1.5 w-1.5 shrink-0 rounded-full"
            style={{ background: "var(--danger)" }}
            title="Provider API key is missing"
            aria-label="Provider key missing"
          />
        ) : null}
        <ChevronDown size={12} className="shrink-0" />
      </button>

      {open ? (
        <div
          className="animate-in absolute bottom-full left-0 z-30 mb-1.5 w-[320px] rounded-xl border p-3 shadow-xl"
          style={{ background: "var(--panel)", borderColor: "var(--border)" }}
        >
          {providers.length === 0 ? (
            <div className="text-[12.5px]" style={{ color: "var(--muted)" }}>
              No providers yet.{" "}
              <button
                className="font-medium"
                style={{ color: "var(--accent)" }}
                onClick={() => {
                  setOpen(false);
                  setView("settings");
                }}
              >
                Add one
              </button>
            </div>
          ) : (
            <>
              <label className="mb-1 block text-[11px]" style={{ color: "var(--muted)" }}>
                Provider
              </label>
              <select
                value={providerId}
                onChange={(event) => {
                  setProviderId(event.target.value);
                  setModelQuery("");
                  const next = providers.find((item) => item.id === event.target.value);
                  if (next) {
                    setSelectedModel(
                      next.model || next.models[0] || "",
                    );
                  }
                }}
                className={inputClass}
                style={{ borderColor: "var(--border)", background: "var(--panel)" }}
              >
                {providers.map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.name}
                  </option>
                ))}
              </select>

              <label className="mt-2.5 mb-1 block text-[11px]" style={{ color: "var(--muted)" }}>
                Model
              </label>
              {models.length > 0 ? (
                <>
                  <input
                    value={modelQuery}
                    onChange={(event) => setModelQuery(event.target.value)}
                    className={inputClass}
                    style={{ borderColor: "var(--border)", background: "var(--panel)" }}
                    placeholder="Search models…"
                    aria-label="Search models"
                  />
                  <div
                    className="mt-1.5 max-h-48 overflow-y-auto rounded-lg border p-1"
                    style={{ borderColor: "var(--border)" }}
                    data-testid="model-options"
                  >
                    {filteredModels.length === 0 ? (
                      <div className="px-2 py-2 text-[11px]" style={{ color: "var(--faint)" }}>
                        No matching models.
                      </div>
                    ) : (
                      filteredModels.map((item) => {
                        const active = selectedModel === item;
                        return (
                          <button
                            key={item}
                            type="button"
                            onClick={() => setSelectedModel(item)}
                            className="focus-ring flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12px] transition hover:bg-[var(--panel-2)]"
                            style={{ background: active ? "var(--panel-2)" : "transparent" }}
                            title={item}
                          >
                            <span className="min-w-0 flex-1 truncate">{shortModelName(item)}</span>
                            {active ? <Check size={12} style={{ color: "var(--accent)" }} /> : null}
                          </button>
                        );
                      })
                    )}
                  </div>
                </>
              ) : null}
              {usesCustom ? (
                <input
                  value={selectedModel}
                  onChange={(event) => setSelectedModel(event.target.value)}
                  className={inputClass}
                  style={{
                    borderColor: "var(--border)",
                    background: "var(--panel)",
                    marginTop: models.length > 0 ? 6 : 0,
                  }}
                  placeholder={provider?.model ?? "model id"}
                  aria-label="Custom model"
                />
              ) : (
                <button
                  type="button"
                  onClick={() => setSelectedModel("")}
                  className="focus-ring mt-1.5 rounded px-1 text-[10.5px] font-medium transition hover:bg-[var(--panel-2)]"
                  style={{ color: "var(--accent)" }}
                >
                  Use a custom model…
                </button>
              )}
              <div className="mt-1 text-[9.5px]" style={{ color: "var(--faint)" }}>
                {models.length > 0
                  ? "Model list saved with this provider."
                  : "No saved models. Add or fetch models in Settings."}
              </div>

              <div className="mt-3 flex justify-end gap-2">
                <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
                  Cancel
                </Button>
                <Button
                  size="sm"
                  variant="primary"
                  icon={<Check size={12} />}
                  onClick={() => void save()}
                >
                  Use model
                </Button>
              </div>
            </>
          )}
        </div>
      ) : null}
    </div>
  );
}
