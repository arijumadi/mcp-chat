import { useEffect, useRef, useState } from "react";
import { KeyRound, PlugZap } from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type {
  ConnectionReport,
  OAuthStart,
  OAuthStatus,
  ServerInput,
  ServerView,
  TestServerInput,
} from "@/lib/types";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass, hintClass } from "@/components/ui";

function argsFromJson(json: string | null | undefined): string[] {
  try {
    const value = JSON.parse(json ?? "[]") as unknown;
    return Array.isArray(value)
      ? value.filter((item): item is string => typeof item === "string")
      : [];
  } catch {
    return [];
  }
}

function argsJsonFromText(text: string): string {
  return JSON.stringify(
    text
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean),
  );
}

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
    command: "",
    argsJson: "[]",
    envJson: "{}",
    cwd: "",
  });
  const [argsText, setArgsText] = useState("");
  const [report, setReport] = useState<ConnectionReport | null>(null);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [oauthBusy, setOauthBusy] = useState(false);
  const [oauthFlow, setOauthFlow] = useState<string | null>(null);
  const [oauthConnected, setOauthConnected] = useState(false);
  const [activeServer, setActiveServer] = useState<ServerView | null>(null);
  const oauthGeneration = useRef(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    oauthGeneration.current += 1;
    setReport(null);
    setError(null);
    setOauthFlow(null);
    setOauthBusy(false);
    setActiveServer(server);
    setOauthConnected(Boolean(server?.hasOAuthToken));
    if (server) {
      const transport =
        server.transport === "sse" || server.transport === "stdio"
          ? server.transport
          : "streamable_http";
      setForm({
        name: server.name,
        url: server.url,
        authKind: transport === "stdio" ? "none" : server.authKind,
        headersJson: server.headersJson,
        policy: server.policy,
        bearer: "",
        apiKey: "",
        transport,
        command: server.command,
        argsJson: server.argsJson,
        envJson: server.envJson,
        cwd: server.cwd,
      });
      setArgsText(argsFromJson(server.argsJson).join("\n"));
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
        command: "",
        argsJson: "[]",
        envJson: "{}",
        cwd: "",
      });
      setArgsText("");
    }
  }, [open, server]);

  const set = <K extends keyof ServerInput>(key: K, value: ServerInput[K]) =>
    setForm((current) => ({ ...current, [key]: value }));

  const isStdio = form.transport === "stdio";
  const endpointReady = isStdio ? Boolean(form.command?.trim()) : Boolean(form.url.trim());

  const changeTransport = (transport: string) => {
    if (transport === "stdio" && oauthFlow) {
      void call("cancel_server_oauth", { flowId: oauthFlow }).catch(() => undefined);
      oauthGeneration.current += 1;
      setOauthFlow(null);
      setOauthBusy(false);
    }
    setForm((current) => ({
      ...current,
      transport,
      authKind: transport === "stdio" ? "none" : current.authKind,
      command:
        transport === "stdio" && !current.command?.trim() ? "npx" : current.command,
    }));
  };

  const serverInput = (): ServerInput => ({
    ...form,
    url: isStdio ? "" : form.url.trim(),
    authKind: isStdio ? "none" : form.authKind,
    headersJson: isStdio ? "{}" : form.headersJson.trim() || "{}",
    command: isStdio ? form.command?.trim() : "",
    argsJson: isStdio ? argsJsonFromText(argsText) : "[]",
    envJson: isStdio ? form.envJson?.trim() || "{}" : "{}",
    cwd: isStdio ? form.cwd?.trim() : "",
    bearer: form.bearer?.trim() || undefined,
    apiKey: form.apiKey?.trim() || undefined,
  });

  const test = async () => {
    setTesting(true);
    setError(null);
    setReport(null);
    try {
      const input: TestServerInput = {
        ...serverInput(),
        serverId: activeServer?.id ?? server?.id,
        name: form.name || "Test server",
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

  const persist = async (): Promise<ServerView> => {
    const input = serverInput();
    const target = activeServer ?? server;
    const saved = target
      ? await call<ServerView>("update_server", { serverId: target.id, input })
      : await call<ServerView>("create_server", { input });
    setActiveServer(saved);
    return saved;
  };

  const pollOAuth = async (flowId: string, serverId: string, generation: number) => {
    const deadline = Date.now() + 10 * 60 * 1000;
    while (Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      if (oauthGeneration.current !== generation) return;
      const status = await call<OAuthStatus>("oauth_status", { flowId });
      if (oauthGeneration.current !== generation) return;
      if (status.status === "success") {
        setOauthConnected(true);
        setOauthFlow(null);
        setOauthBusy(false);
        await refreshServers();
        try {
          const refreshed = await call<ConnectionReport>("refresh_server", { serverId });
          setReport(refreshed);
        } catch {
          // The token is saved; a later refresh can retry the server.
        }
        toast("OAuth connected. Tokens are stored in the OS keychain.", "info");
        return;
      }
      if (["error", "cancelled", "not_found"].includes(status.status)) {
        setOauthFlow(null);
        setOauthBusy(false);
        setError(status.error ?? "OAuth was not completed.");
        return;
      }
    }
    if (oauthGeneration.current !== generation) return;
    setOauthFlow(null);
    setOauthBusy(false);
    setError("Timed out waiting for the browser sign-in. Try again.");
  };

  const startOAuth = async (target: ServerView) => {
    const generation = ++oauthGeneration.current;
    let flowId: string | null = null;
    setOauthBusy(true);
    setError(null);
    try {
      const start = await call<OAuthStart>("start_server_oauth", { serverId: target.id });
      flowId = start.flowId;
      setOauthFlow(start.flowId);
      await call("open_external", { url: start.authUrl });
      await pollOAuth(start.flowId, target.id, generation);
    } catch (error) {
      if (flowId) {
        void call("cancel_server_oauth", { flowId }).catch(() => undefined);
      }
      if (oauthGeneration.current === generation) {
        setOauthFlow(null);
        setOauthBusy(false);
        setError(String(error));
      }
    }
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const saved = await persist();
      setOauthConnected(saved.hasOAuthToken);
      await refreshServers();
      onClose();
    } catch (error) {
      setError(String(error));
    } finally {
      setSaving(false);
    }
  };

  const saveAndConnect = async () => {
    setSaving(true);
    setError(null);
    try {
      const saved = await persist();
      setOauthConnected(saved.hasOAuthToken);
      await refreshServers();
      await startOAuth(saved);
    } catch (error) {
      setError(String(error));
    } finally {
      setSaving(false);
    }
  };

  const disconnectOAuth = async () => {
    const target = activeServer ?? server;
    if (!target) return;
    setOauthBusy(true);
    setError(null);
    try {
      const updated = await call<ServerView>("disconnect_server_oauth", {
        serverId: target.id,
      });
      setActiveServer(updated);
      setOauthConnected(false);
      setOauthFlow(null);
      oauthGeneration.current += 1;
      await refreshServers();
      toast("OAuth session removed.", "info");
    } catch (error) {
      setError(String(error));
    } finally {
      setOauthBusy(false);
    }
  };

  const close = () => {
    oauthGeneration.current += 1;
    if (oauthFlow) {
      void call("cancel_server_oauth", { flowId: oauthFlow }).catch(() => undefined);
      setOauthFlow(null);
    }
    onClose();
  };

  const capabilities = report?.capabilities
    ? Object.keys(report.capabilities as Record<string, unknown>)
    : [];
  const oauthNeedsConnect = form.authKind === "oauth" && !oauthConnected;

  return (
    <Modal
      open={open}
      onClose={close}
      width="lg"
      title={server ? "Edit MCP server" : "Add MCP server"}
      subtitle={
        isStdio
          ? "Run a local MCP server over stdio, for example through npx."
          : "Remote Streamable HTTP is preferred; legacy handshake and HTTP+SSE are detected automatically."
      }
      footer={
        <>
          <Button size="sm" variant="ghost" onClick={close}>
            Cancel
          </Button>
          <Button
            size="sm"
            variant="secondary"
            icon={<PlugZap size={12} />}
            loading={testing}
            onClick={() => void test()}
            disabled={!endpointReady || oauthBusy || oauthNeedsConnect}
          >
            Test connection
          </Button>
          <Button
            size="sm"
            variant="primary"
            loading={saving || oauthBusy}
            onClick={() => void (oauthNeedsConnect ? saveAndConnect() : save())}
            disabled={!form.name.trim() || !endpointReady || oauthBusy}
          >
            {oauthNeedsConnect ? "Save and connect" : server ? "Save changes" : "Add server"}
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

        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
              Transport
            </span>
            <select
              value={form.transport}
              onChange={(event) => changeTransport(event.target.value)}
              disabled={oauthBusy}
              className={inputClass}
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <option value="streamable_http">Remote HTTP (auto-detect)</option>
              <option value="sse">Remote HTTP+SSE (legacy /sse URL)</option>
              <option value="stdio">Local command (stdio / npx)</option>
            </select>
          </label>
          {isStdio ? (
            <div
              className="rounded-lg border px-3 py-2 text-[11.5px]"
              style={{ borderColor: "var(--border)", color: "var(--muted)" }}
            >
              Runs as a child process with your user permissions.
            </div>
          ) : (
            <label className="block">
              <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                Auth
              </span>
              <select
                value={form.authKind}
                disabled={oauthBusy}
                onChange={(event) => {
                  const authKind = event.target.value;
                  set("authKind", authKind);
                  if (authKind !== "oauth") {
                    if (oauthFlow) {
                      void call("cancel_server_oauth", { flowId: oauthFlow }).catch(
                        () => undefined,
                      );
                    }
                    oauthGeneration.current += 1;
                    setOauthFlow(null);
                    setOauthBusy(false);
                    setOauthConnected(false);
                  }
                }}
                className={inputClass}
                style={{ borderColor: "var(--border)", background: "var(--panel)" }}
              >
                <option value="none">None</option>
                <option value="api_key">API key header (X-API-Key)</option>
                <option value="bearer">Static bearer token</option>
                <option value="oauth">OAuth 2.0 (authorization code + PKCE)</option>
              </select>
            </label>
          )}
        </div>

        {isStdio ? (
          <>
            <div className="grid grid-cols-2 gap-3">
              <label className="block">
                <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                  Command
                </span>
                <input
                  value={form.command ?? ""}
                  onChange={(event) => set("command", event.target.value)}
                  placeholder="npx"
                  className={inputClass}
                  style={{ borderColor: "var(--border)" }}
                  spellCheck={false}
                />
              </label>
              <label className="block">
                <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                  Working directory (optional)
                </span>
                <input
                  value={form.cwd ?? ""}
                  onChange={(event) => set("cwd", event.target.value)}
                  placeholder="/path/to/project"
                  className={inputClass}
                  style={{ borderColor: "var(--border)" }}
                  spellCheck={false}
                />
              </label>
            </div>

            <label className="block">
              <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                Arguments (one per line)
              </span>
              <textarea
                value={argsText}
                onChange={(event) => setArgsText(event.target.value)}
                className={inputClass}
                style={{ borderColor: "var(--border)", minHeight: 96 }}
                placeholder={"-y\n@modelcontextprotocol/server-filesystem\n/path/to/folder"}
                spellCheck={false}
              />
              <span className={hintClass} style={{ color: "var(--faint)" }}>
                Example: npx with <code>-y</code> and the MCP package name. Use one line per argument.
              </span>
            </label>

            <details className="rounded-lg border px-3 py-2" style={{ borderColor: "var(--border)" }}>
              <summary className="cursor-pointer text-[11.5px]" style={{ color: "var(--muted)" }}>
                Environment variables (JSON)
              </summary>
              <textarea
                value={form.envJson ?? "{}"}
                onChange={(event) => set("envJson", event.target.value)}
                className={inputClass}
                style={{ borderColor: "var(--border)", minHeight: 80, marginTop: 8 }}
                placeholder='{"GITHUB_TOKEN": "..."}'
                spellCheck={false}
              />
              <span className={hintClass} style={{ color: "var(--faint)" }}>
                Stored in the local SQLite database. The process inherits the app environment too.
              </span>
            </details>

            <div
              className="rounded-lg border px-3 py-2 text-[11.5px]"
              style={{ borderColor: "var(--warn)", color: "var(--warn)" }}
            >
              Only run packages and commands you trust. A local MCP server can read and write files as
              your user.
            </div>
          </>
        ) : (
          <>
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

            {form.authKind === "oauth" ? (
              <div
                className="rounded-xl border px-3.5 py-3"
                style={{ borderColor: oauthConnected ? "var(--accent)" : "var(--border)" }}
              >
                <div className="flex flex-wrap items-start gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2 text-[12.5px] font-medium">
                      <KeyRound size={14} style={{ color: oauthConnected ? "var(--accent)" : "var(--muted)" }} />
                      {oauthConnected ? "OAuth connected" : "OAuth not connected"}
                    </div>
                    <p className="mt-1 text-[11.5px]" style={{ color: "var(--muted)" }}>
                      {oauthFlow
                        ? "Waiting for the browser sign-in. Finish it there, then return to Jarvis."
                        : oauthConnected
                          ? `Tokens are stored in the OS keychain${activeServer?.oauthIssuer ? ` · ${activeServer.oauthIssuer}` : ""}. Reconnect if access expires or is revoked.`
                          : "Jarvis opens the provider's sign-in page with a local callback. The provider controls which account and scopes are available."}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-1.5">
                    {oauthConnected ? (
                      <Button size="sm" variant="ghost" onClick={() => void disconnectOAuth()} disabled={oauthBusy}>
                        Disconnect
                      </Button>
                    ) : null}
                    <Button
                      size="sm"
                      variant="secondary"
                      icon={<KeyRound size={12} />}
                      loading={oauthBusy || saving}
                      onClick={() => void saveAndConnect()}
                      disabled={!endpointReady || oauthBusy || saving}
                    >
                      {oauthConnected ? "Reconnect" : "Connect"}
                    </Button>
                  </div>
                </div>
              </div>
            ) : null}

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
          </>
        )}

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
