import { useEffect, useMemo, useState } from "react";
import { MessageSquareQuote } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass } from "@/components/ui";
import { call } from "@/lib/ipc";

export function InputRequestModal() {
  const requests = useApp((state) => state.inputRequests);
  const respond = useApp((state) => state.respondInput);
  const toast = useApp((state) => state.toast);
  const request = requests[0];
  const [values, setValues] = useState<Record<string, Record<string, unknown>>>({});
  const [error, setError] = useState<string | null>(null);

  const entries = useMemo(() => Object.entries(request?.request.inputRequests ?? {}), [request]);

  useEffect(() => {
    setValues({});
    setError(null);
  }, [request?.callId]);

  if (!request) return null;

  const unsupported = entries.filter(
    ([, item]) => item.method !== "elicitation/create",
  );
  const forms = entries.filter(
    ([, item]) => item.method === "elicitation/create" && item.params.mode !== "url",
  );
  const urls = entries.filter(
    ([, item]) => item.method === "elicitation/create" && item.params.mode === "url",
  );

  const accept = () => {
    const responses: Record<string, unknown> = {};
    for (const [key, item] of forms) {
      const schema = item.params.requestedSchema;
      const content = values[key] ?? {};
      for (const required of schema?.required ?? []) {
        const current = content[required];
        if (current === undefined || current === "") {
          setError(`"${required}" is required.`);
          return;
        }
      }
      responses[key] = { action: "accept", content };
    }
    for (const [key] of urls) {
      responses[key] = { action: "accept" };
    }
    if (unsupported.length > 0) {
      setError("This app cannot answer sampling or roots requests. Decline to continue.");
      return;
    }
    void respond(request.callId, responses, false);
  };

  return (
    <Modal
      open
      onClose={() => undefined}
      width="md"
      title={
        <span className="flex items-center gap-2">
          <MessageSquareQuote size={16} style={{ color: "var(--accent)" }} />
          The server needs input
        </span>
      }
      subtitle="The tool call is paused until you answer."
      footer={
        <>
          <Button size="sm" variant="ghost" onClick={() => void respond(request.callId, {}, true)}>
            Decline
          </Button>
          <Button
            size="sm"
            variant="primary"
            disabled={unsupported.length > 0}
            onClick={accept}
          >
            Send response
          </Button>
        </>
      }
    >
      {request.request.requestState ? (
        <p className="mb-3 text-[11.5px]" style={{ color: "var(--faint)" }}>
          The server sent a continuation state. It will be echoed back verbatim.
        </p>
      ) : null}

      {entries.length === 0 ? (
        <p className="text-[13px]" style={{ color: "var(--muted)" }}>
          The server requested another round trip without a form. You can retry immediately.
        </p>
      ) : null}

      {forms.map(([key, item]) => (
        <div key={key} className="mb-4">
          {item.params.message ? (
            <p className="mb-2 text-[13px]">{item.params.message}</p>
          ) : null}
          <div className="grid gap-3">
            {Object.entries(item.params.requestedSchema?.properties ?? {}).map(
              ([field, property]) => {
                const required = item.params.requestedSchema?.required?.includes(field);
                if (property.enum) {
                  return (
                    <label key={field} className="block">
                      <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                        {property.title ?? field}
                        {required ? " *" : ""}
                      </span>
                      <select
                        value={String(values[key]?.[field] ?? "")}
                        onChange={(event) =>
                          setValues((current) => ({
                            ...current,
                            [key]: { ...current[key], [field]: event.target.value },
                          }))
                        }
                        className={inputClass}
                        style={{ borderColor: "var(--border)", background: "var(--panel)" }}
                      >
                        <option value="">Choose…</option>
                        {property.enum.map((option) => (
                          <option key={String(option)} value={String(option)}>
                            {String(option)}
                          </option>
                        ))}
                      </select>
                    </label>
                  );
                }
                if (property.type === "boolean") {
                  return (
                    <label key={field} className="flex items-center gap-2 text-[12.5px]">
                      <input
                        type="checkbox"
                        checked={Boolean(values[key]?.[field] ?? false)}
                        onChange={(event) =>
                          setValues((current) => ({
                            ...current,
                            [key]: { ...current[key], [field]: event.target.checked },
                          }))
                        }
                      />
                      {property.title ?? field}
                      {required ? " *" : ""}
                    </label>
                  );
                }
                const numeric = property.type === "number" || property.type === "integer";
                return (
                  <label key={field} className="block">
                    <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                      {property.title ?? field}
                      {required ? " *" : ""}
                    </span>
                    <input
                      type={numeric ? "number" : "text"}
                      value={String(values[key]?.[field] ?? "")}
                      onChange={(event) =>
                        setValues((current) => ({
                          ...current,
                          [key]: {
                            ...current[key],
                            [field]: numeric ? Number(event.target.value) : event.target.value,
                          },
                        }))
                      }
                      className={inputClass}
                      style={{ borderColor: "var(--border)" }}
                    />
                    {property.description ? (
                      <span className="mt-0.5 block text-[10.5px]" style={{ color: "var(--faint)" }}>
                        {property.description}
                      </span>
                    ) : null}
                  </label>
                );
              },
            )}
          </div>
        </div>
      ))}

      {urls.map(([key, item]) => (
        <div key={key} className="mb-4">
          <p className="mb-2 text-[13px]">{item.params.message}</p>
          {item.params.url ? (
            <Button
              size="sm"
              variant="secondary"
              onClick={() => void call("open_external", { url: item.params.url }).catch(() => toast("Could not open URL", "error"))}
            >
              Open link in browser
            </Button>
          ) : null}
        </div>
      ))}

      {unsupported.length > 0 ? (
        <div
          className="rounded-lg border px-3 py-2.5 text-[12.5px]"
          style={{
            borderColor: "var(--border)",
            background: "var(--panel-2)",
            color: "var(--muted)",
          }}
        >
          This server asked for{" "}
          {unsupported.map(([, item]) => item.method).join(", ")}. MCP Chat does not support
          sampling or roots requests in v1. Decline to let the tool settle with an error.
        </div>
      ) : null}

      {error ? (
        <p className="mt-3 text-[12px]" style={{ color: "var(--danger)" }}>
          {error}
        </p>
      ) : null}
    </Modal>
  );
}
