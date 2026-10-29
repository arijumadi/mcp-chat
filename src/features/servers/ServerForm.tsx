import { useEffect, useState } from "react";
import { PlugZap } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { ConnectionReport, ServerInput, ServerView, TestServerInput } from "@/lib/types";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass, hintClass } from "@/components/ui";

export function ServerForm({
  server,
  open,
  onClose,
}: {
  server: ServerView | null;
  open: boolean;
  onClose: () => void;
}) {
  const refreshServers = useApp((state) => state.refreshServers);
  const toast = useApp((state) => state.toast);
  const [form, setForm] = useState<ServerInput>({
    name: "",
    url: "",
    authKind: "none",
    headersJson: "{}",
    policy: "ask",
    bearer: "",
    apiKey: "",
    transport: "streamable_http",
  });
  const [report, setReport] = useState<ConnectionReport | null>(null);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setReport(null);
    setError(null);
    if (server) {
      setForm({
        name: server.name,
        url: server.url,
        authKind: server.authKind,
        headersJson: server.headersJson,
        policy: server.policy,
        bearer: "",
        apiKey: "",
        transport: server.transport === "sse" ? "sse" : "streamable_http",
      });
    } else {
      setForm({
        name: "",
        url: "",
        authKind: "none",
        headersJson: "{}",
        policy: "ask",
        bearer: "",
        apiKey: "",
        transport: "streamable_http",
      });
    }
  }, [open, server]);

  const set = <K extends keyof ServerInput>(key: K, value: ServerInput[K]) =>
    setForm((current) => ({ ...current, [key]: value }));

  const test = async () => {
    setTesting(true);
    setError(null);
    setReport(null);
    try {
      const input: TestServerInput = {
        serverId: server?.id,
        name: form.name || "Test server",
        url: form.url,
        authKind: form.authKind,
        headersJson: form.headersJson,
        bearer: form.bearer,
        apiKey: form.apiKey,
        transport: form.transport,
      };
      const result = await call<ConnectionReport>("test_server", { input });
      setReport(result);
      toast(
        `Connected with ${result.protocolVersion}. ${result.tools.length} tool(s).`,
        "info",
      );
    } catch (error) {
      setError(String(error));
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const input = {
        ...form,
        headersJson: form.headersJson.trim() || "{}",
        bearer: form.bearer?.trim() || undefined,
        apiKey: form.apiKey?.trim() || undefined,
      };
      if (server) {
        await call("update_server", { serverId: server.id, input });
      } else {
        await call("create_server", { input });
      }
      await refreshServers();
      onClose();
    } catch (error) {
      setError(String(error));
    } finally {
      setSaving(false);
    }
  };

  const capabilities = report?.capabilities
    ? Object.keys(report.capabilities as Record<string, unknown>)
    : [];

  return (
    <Modal
      open={open}
      onClose={onClose}
      width="lg"
      title={server ? "Edit MCP server" : "Add MCP server"}
      subtitle="Remote Streamable HTTP is preferred; legacy handshake and HTTP+SSE are detected automatically."
      footer={
        <>
          <Button size="sm" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            variant="secondary"
            icon={<PlugZap size={12} />}
            loading={testing}
            onClick={() => void test()}
            disabled={!form.url.trim()}
          >
            Test connection
          </Button>
          <Button
            size="sm"
            variant="primary"
            loading={saving}
            onClick={() => void save()}
            disabled={!form.name.trim() || !form.url.trim()}
          >
            {server ? "Save changes" : "Add server"}
          </Button>
        </>
      }
    >
      <div className="grid gap-3.5">
        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              Name
            </span>
            <input
              autoFocus
              value={form.name}
              onChange={(event) => set("name", event.target.value)}
              placeholder="Mock MCP"
              className={inputClass}
              style={{ borderColor: "var(--border)" }}
            />
          </label>
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              Policy
            </span>
            <select
              value={form.policy}
              onChange={(event) => set("policy", event.target.value)}
              className={inputClass}
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <option value="ask">Ask before tools (default)</option>
              <option value="auto">Auto-allow tools</option>
              <option value="deny">Deny all tools</option>
            </select>
          </label>
        </div>

        <label className="block">
          <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
            Server URL
          </span>
          <input
            value={form.url}
            onChange={(event) => set("url", event.target.value)}
            placeholder="https://example.com/mcp or http://127.0.0.1:8787/mcp"
            className={inputClass}
            style={{ borderColor: "var(--border)" }}
          />
          <span className={hintClass} style={{ color: "var(--faint)" }}>
            HTTPS only, except localhost for development.
          </span>
        </label>

        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              Auth
            </span>
            <select
              value={form.authKind}
              onChange={(event) => set("authKind", event.target.value)}
              className={inputClass}
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <option value="none">None</option>
              <option value="api_key">API key header (X-API-Key)</option>
              <option value="bearer">Static bearer token</option>
              <option value="oauth" disabled>
                OAuth (next milestone)
              </option>
            </select>
          </label>
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              Transport
            </span>
            <select
              value={form.transport}
              onChange={(event) => set("transport", event.target.value)}
              className={inputClass}
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <option value="streamable_http">Auto-detect (HTTP POST)</option>
              <option value="sse">HTTP+SSE (legacy /sse URL)</option>
            </select>
          </label>
        </div>

        {form.authKind === "bearer" || form.authKind === "api_key" ? (
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              {form.authKind === "api_key" ? "X-API-Key value" : "Bearer token"}
            </span>
            <input
              type="password"
              value={form.authKind === "api_key" ? (form.apiKey ?? "") : (form.bearer ?? "")}
              onChange={(event) =>
                set(form.authKind === "api_key" ? "apiKey" : "bearer", event.target.value)
              }
              placeholder={
                form.authKind === "api_key"
                  ? server?.hasApiKey
                    ? "Saved — leave blank to keep"
                    : "key from stock.arjum.com"
                  : server?.hasBearer
                    ? "Saved — leave blank to keep"
                    : "token"
              }
              className={inputClass}
              style={{ borderColor: "var(--border)" }}
              autoComplete="off"
            />
            <span className={hintClass} style={{ color: "var(--faint)" }}>
              Stored in the OS keychain, never in SQLite.
              {form.authKind === "api_key"
                ? " Sent as the X-API-Key header on every MCP request."
                : ""}
            </span>
          </label>
        ) : null}

        <details className="rounded-lg border px-3 py-2" style={{ borderColor: "var(--border)" }}>
          <summary className="cursor-pointer text-[11.5px]" style={{ color: "var(--muted)" }}>
            Advanced: custom headers (JSON)
          </summary>
          <textarea
            value={form.headersJson}
            onChange={(event) => set("headersJson", event.target.value)}
            className={inputClass}
            style={{ borderColor: "var(--border)", minHeight: 80, marginTop: 8 }}
            placeholder='{"X-Client": "mcp-chat"}'
          />
          <span className={hintClass} style={{ color: "var(--faint)" }}>
            Authorization, Content-Type, Accept and Mcp-* headers are managed by the app.
          </span>
        </details>

        {error ? (
          <div
            className="rounded-lg border px-3 py-2 text-[12.5px]"
            style={{ borderColor: "var(--danger)", color: "var(--danger)", background: "var(--danger-soft)" }}
          >
            {error}
          </div>
        ) : null}

        {report ? (
          <div
            className="rounded-xl border px-3.5 py-3"
            style={{ borderColor: "var(--accent)", background: "var(--panel)" }}
          >
            <div className="flex flex-wrap items-center gap-1.5">
              <span
                className="rounded border px-1.5 py-0.5 text-[10.5px]"
                style={{ borderColor: "var(--accent)", color: "var(--accent)" }}
              >
                {report.protocolVersion}
              </span>
              <span className="text-[11.5px]" style={{ color: "var(--muted)" }}>
                {report.serverInfo?.name ?? "unknown server"} {report.serverInfo?.version ?? ""} ·{" "}
                {report.transport} · {report.tools.length} tools
              </span>
            </div>
            <div className="mt-2 flex flex-wrap gap-1">
              {capabilities.length ? (
                capabilities.map((capability) => (
                  <span
                    key={capability}
                    className="rounded border px-1.5 py-0.5 text-[10.5px]"
                    style={{ borderColor: "var(--border)", color: "var(--muted)" }}
                  >
                    {capability}
                  </span>
                ))
              ) : (
                <span className="text-[10.5px]" style={{ color: "var(--faint)" }}>
                  no capabilities advertised
                </span>
              )}
            </div>
            <div className="mt-2 grid gap-1">
              {report.tools.map((tool) => (
                <div key={tool.name} className="text-[11.5px]" style={{ color: "var(--muted)" }}>
                  <span className="font-mono" style={{ color: "var(--fg)" }}>
                    {tool.name}
                  </span>{" "}
                  — {tool.description || "no description"}
                </div>
              ))}
            </div>
          </div>
        ) : null}
      </div>
    </Modal>
  );
}
