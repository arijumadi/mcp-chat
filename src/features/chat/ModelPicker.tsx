import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, RefreshCw } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import { Button } from "@/components/Button";
import { inputClass } from "@/components/ui";
import type { Conversation } from "@/lib/types";

export function ModelPicker({ conversation }: { conversation: Conversation }) {
  const providers = useApp((state) => state.providers);
  const applyModel = useApp((state) => state.setModel);
  const setView = useApp((state) => state.setView);
  const toast = useApp((state) => state.toast);
  const [open, setOpen] = useState(false);
  const [providerId, setProviderId] = useState(conversation.providerId ?? "");
  const [selectedModel, setSelectedModel] = useState(conversation.model ?? "");
  const [models, setModels] = useState<string[]>([]);
  const [fetching, setFetching] = useState(false);
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

  const fetchModels = async () => {
    if (!providerId) return;
    setFetching(true);
    try {
      const list = await call<string[]>("fetch_models", { providerId });
      setModels(list);
      if (!list.includes(selectedModel) && list.length > 0) setSelectedModel(list[0] ?? "");
      toast(`Fetched ${list.length} models.`, "info");
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setFetching(false);
    }
  };

  const save = async () => {
    if (!providerId || !selectedModel.trim()) return;
    await applyModel(providerId, selectedModel.trim());
    setOpen(false);
  };

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((value) => !value)}
        className="focus-ring flex max-w-[260px] items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-[11.5px] transition hover:bg-[var(--panel-2)]"
        style={{ borderColor: "var(--border)", color: "var(--muted)" }}
        title="Change model"
      >
        <span className="truncate">{provider?.name ?? "No provider"}</span>
        <span style={{ color: "var(--faint)" }}>/</span>
        <span className="truncate font-medium" style={{ color: "var(--fg)" }}>
          {conversation.model ?? provider?.model ?? "model"}
        </span>
        <ChevronDown size={12} className="shrink-0" />
      </button>

      {open ? (
        <div
          className="animate-in absolute top-full right-0 z-30 mt-1.5 w-[320px] rounded-xl border p-3 shadow-xl"
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
                  if (next) setSelectedModel(next.model);
                }}
                className={inputClass}
                style={{ borderColor: "var(--border)", background: "var(--panel)" }}
              >
                {providers.map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.name} · {item.kind}
                  </option>
                ))}
              </select>

              <div className="mt-2.5 mb-1 flex items-center justify-between">
                <label className="text-[11px]" style={{ color: "var(--muted)" }}>
                  Model
                </label>
                <button
                  onClick={() => void fetchModels()}
                  className="focus-ring flex items-center gap-1 rounded px-1 text-[10.5px] transition hover:bg-[var(--panel-2)]"
                  style={{ color: "var(--muted)" }}
                >
                  <RefreshCw size={10} className={fetching ? "animate-spin" : ""} />
                  Fetch models
                </button>
              </div>
              <input
                value={selectedModel}
                onChange={(event) => setSelectedModel(event.target.value)}
                list="model-options"
                className={inputClass}
                style={{ borderColor: "var(--border)", background: "var(--panel)" }}
                placeholder={provider?.model ?? "model id"}
              />
              <datalist id="model-options">
                {models.map((item) => (
                  <option key={item} value={item} />
                ))}
              </datalist>

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
