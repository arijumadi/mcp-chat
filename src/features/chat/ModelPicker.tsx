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
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setProviderId(conversation.providerId ?? providers[0]?.id ?? "");
    setSelectedModel(conversation.model ?? providers[0]?.model ?? "");
  }, [conversation.providerId, conversation.model, providers]);

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
                <select
                  value={usesCustom ? "__custom__" : selectedModel}
                  onChange={(event) => {
                    if (event.target.value === "__custom__") {
                      setSelectedModel("");
                    } else {
                      setSelectedModel(event.target.value);
                    }
                  }}
                  className={inputClass}
                  style={{ borderColor: "var(--border)", background: "var(--panel)" }}
                >
                  {usesCustom ? <option value="__custom__">Custom model…</option> : null}
                  {models.map((item) => (
                    <option key={item} value={item}>
                      {shortModelName(item)}
                    </option>
                  ))}
                  {!usesCustom ? <option value="__custom__">Custom model…</option> : null}
                </select>
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
                />
              ) : null}
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
