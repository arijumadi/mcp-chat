import { useState } from "react";
import {
  ArrowLeft,
  ChevronDown,
  Pencil,
  Play,
  Plug,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { useApp } from "@/app/store";
import { call } from "@/lib/ipc";
import type { CallToolResult, ConnectionReport, ServerView, ToolInfo } from "@/lib/types";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { cn } from "@/lib/cn";
import { formatJson } from "@/lib/format";
import { inputClass } from "@/components/ui";
import { ServerForm } from "./ServerForm";

function commandLine(server: ServerView): string {
  if (server.transport !== "stdio") return server.url;
  let args: string[] = [];
  try {
    const parsed = JSON.parse(server.argsJson) as unknown;
    if (Array.isArray(parsed)) {
      args = parsed.filter((item): item is string => typeof item === "string");
    }
  } catch {
    // The form reports invalid JSON; the card still shows the command.
  }
  return [server.command, ...args].filter(Boolean).join(" ");
}

export function ServersView() {
  const servers = useApp((state) => state.servers);
  const refreshServers = useApp((state) => state.refreshServers);
  const setView = useApp((state) => state.setView);
  const toast = useApp((state) => state.toast);

  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<ServerView | null>(null);
  const [deleting, setDeleting] = useState<ServerView | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [tools, setTools] = useState<Record<string, ToolInfo[]>>({});
  const [loadingTools, setLoadingTools] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState<string | null>(null);
  const [report, setReport] = useState<ConnectionReport | null>(null);
  const [running, setRunning] = useState<{ server: ServerView; tool: ToolInfo } | null>(null);
  const [runArgs, setRunArgs] = useState("{}");
  const [runResult, setRunResult] = useState<string | null>(null);
  const [runBusy, setRunBusy] = useState(false);

  const refreshAll = async () => {
    setRefreshing("*");
    try {
      await call("refresh_all_servers");
      await refreshServers();
      toast("Refreshed all MCP servers.", "info");
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setRefreshing(null);
    }
  };

  const refreshOne = async (server: ServerView) => {
    setRefreshing(server.id);
    try {
      const result = await call<ConnectionReport>("refresh_server", {
        serverId: server.id,
      });
      setReport(result);
      setTools((current) => ({ ...current, [server.id]: result.tools }));
      await refreshServers();
    } catch (error) {
      toast(String(error), "error");
      await refreshServers();
    } finally {
      setRefreshing(null);
    }
  };

  const toggleTools = async (server: ServerView) => {
    if (expanded === server.id) {
      setExpanded(null);
      return;
    }
    setExpanded(server.id);
    if (tools[server.id]) return;
    setLoadingTools(server.id);
    try {
      const list = await call<ToolInfo[]>("list_server_tools", { serverId: server.id });
      setTools((current) => ({ ...current, [server.id]: list }));
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setLoadingTools(null);
    }
  };

  const deleteServer = async () => {
    if (!deleting) return;
    try {
      await call("delete_server", { serverId: deleting.id });
      setDeleting(null);
      await refreshServers();
      toast("MCP server removed.", "info");
    } catch (error) {
      toast(String(error), "error");
    }
  };

  const openRun = (server: ServerView, tool: ToolInfo) => {
    setRunning({ server, tool });
    setRunArgs("{}");
    setRunResult(null);
  };

  const runTool = async () => {
    if (!running) return;
    let parsed: unknown;
    try {
      parsed = JSON.parse(runArgs);
    } catch {
      toast("Arguments must be valid JSON.", "error");
      return;
    }
    setRunBusy(true);
    try {
      const result = await call<CallToolResult>("run_server_tool", {
        serverId: running.server.id,
        toolName: running.tool.name,
        arguments: parsed,
      });
      setRunResult(formatJson(result));
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setRunBusy(false);
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
          <div className="text-[14px] font-medium">MCP servers</div>
          <div className="text-[10.5px]" style={{ color: "var(--faint)" }}>
            remote HTTP + local stdio (npx) · manual approval by default
          </div>
        </div>
        <div className="ml-auto flex items-center gap-1.5">
          <Button
            size="sm"
            variant="ghost"
            icon={<RefreshCw size={12} className={refreshing === "*" ? "animate-spin" : ""} />}
            onClick={() => void refreshAll()}
            disabled={servers.length === 0}
          >
            Refresh all
          </Button>
          <Button
            size="sm"
            variant="primary"
            icon={<Plus size={13} />}
            onClick={() => {
              setEditing(null);
              setFormOpen(true);
            }}
          >
            Add server
          </Button>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[860px] px-6 py-7">
          {servers.length === 0 ? (
            <div className="flex flex-col items-center justify-center py-20 text-center">
              <div
                className="mb-4 flex h-12 w-12 items-center justify-center rounded-2xl border"
                style={{ borderColor: "var(--border)", background: "var(--panel)" }}
              >
                <Plug size={22} style={{ color: "var(--muted)" }} />
              </div>
              <h2 className="text-[16px] font-medium">No MCP servers yet</h2>
              <p className="mt-1 max-w-sm text-[12.5px]" style={{ color: "var(--muted)" }}>
                Connect a remote MCP server, or run a local one through npx/stdio. Tools stay hidden
                until the server answers discovery.
              </p>
              <Button
                className="mt-4"
                variant="primary"
                icon={<Plus size={14} />}
                onClick={() => {
                  setEditing(null);
                  setFormOpen(true);
                }}
              >
                Add your first server
              </Button>
            </div>
          ) : (
            <div className="grid gap-2.5">
              {servers.map((server) => {
                const expandedTools = tools[server.id];
                return (
                  <div
                    key={server.id}
                    className="overflow-hidden rounded-xl border"
                    style={{ borderColor: "var(--border)", background: "var(--panel)" }}
                  >
                    <div className="flex flex-wrap items-start gap-3 px-4 py-3.5">
                      <StatusDot status={server.lastStatus} />
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="text-[13.5px] font-medium">{server.name}</span>
                          <Chip
                            label={`policy: ${server.policy}`}
                            accent={server.policy === "auto"}
                          />
                          {!server.enabled ? <Chip label="disabled" /> : null}
                        </div>
                        <div
                          className="mt-0.5 truncate font-mono text-[10.5px]"
                          style={{ color: "var(--faint)" }}
                        >
                          {commandLine(server)}
                        </div>
                        <div
                          className="mt-1 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-[11.5px]"
                          style={{ color: "var(--muted)" }}
                        >
                          <span>
                            {server.toolCount} tool{server.toolCount === 1 ? "" : "s"}
                          </span>
                          <span style={{ color: "var(--faint)" }}>·</span>
                          <span>
                            {server.transport === "sse"
                              ? "HTTP+SSE"
                              : server.transport === "stdio"
                                ? "local stdio"
                                : "streamable HTTP"}
                            {server.protocolVersion ? ` · ${server.protocolVersion}` : ""}
                          </span>
                          <span style={{ color: "var(--faint)" }}>·</span>
                          <span>
                            {server.transport === "stdio"
                              ? "local process"
                              : `auth: ${server.authKind}`}
                            {server.authKind === "bearer" && !server.hasBearer
                              ? " (missing token)"
                              : ""}
                            {server.authKind === "api_key" && !server.hasApiKey
                              ? " (missing X-API-Key)"
                              : ""}
                            {server.authKind === "oauth" && !server.hasOAuthToken
                              ? " (not connected)"
                              : ""}
                          </span>
                          {server.lastError ? (
                            <span className="w-full" style={{ color: "var(--danger)" }}>
                              {server.lastError}
                            </span>
                          ) : null}
                        </div>
                      </div>
                      <div className="flex shrink-0 items-center gap-0.5">
                        <button
                          type="button"
                          onClick={() => void refreshOne(server)}
                          disabled={refreshing === server.id}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)] disabled:opacity-45"
                          title="Refresh server"
                          aria-label="Refresh server"
                        >
                          <RefreshCw
                            size={14}
                            className={refreshing === server.id ? "animate-spin" : ""}
                          />
                        </button>
                        <button
                          type="button"
                          onClick={() => void toggleTools(server)}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
                          title={expanded === server.id ? "Hide tools" : "Show tools"}
                          aria-label={expanded === server.id ? "Hide tools" : "Show tools"}
                          aria-expanded={expanded === server.id}
                        >
                          <ChevronDown
                            size={14}
                            className={cn(
                              "transition-transform",
                              expanded === server.id && "rotate-180",
                            )}
                          />
                        </button>
                        <button
                          type="button"
                          onClick={() => {
                            setEditing(server);
                            setFormOpen(true);
                          }}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--panel-2)]"
                          title="Edit server"
                          aria-label="Edit server"
                        >
                          <Pencil size={13} />
                        </button>
                        <button
                          type="button"
                          onClick={() => setDeleting(server)}
                          className="focus-ring rounded-lg p-1.5 transition hover:bg-[var(--danger-soft)] hover:text-[var(--danger)]"
                          title="Delete server"
                          aria-label="Delete server"
                        >
                          <Trash2 size={13} />
                        </button>
                      </div>
                    </div>

                    {expanded === server.id ? (
                      <div className="border-t px-4 py-3" style={{ borderColor: "var(--border)" }}>
                        {loadingTools === server.id ? (
                          <div className="text-[11.5px]" style={{ color: "var(--faint)" }}>
                            Loading tools…
                          </div>
                        ) : expandedTools && expandedTools.length ? (
                          <div className="grid gap-1.5">
                            {expandedTools.map((tool) => (
                              <div
                                key={tool.name}
                                className="rounded-lg border px-3 py-2"
                                style={{ borderColor: "var(--border)", background: "var(--panel-2)" }}
                              >
                                <div className="flex flex-wrap items-center gap-2">
                                  <code className="font-mono text-[11.5px]">{tool.name}</code>
                                  <span className="min-w-0 flex-1 truncate text-[11.5px]" style={{ color: "var(--muted)" }}>
                                    {tool.description}
                                  </span>
                                  <Button
                                    size="sm"
                                    variant="ghost"
                                    icon={<Play size={10} />}
                                    onClick={() => openRun(server, tool)}
                                  >
                                    Run tool
                                  </Button>
                                </div>
                                <details className="mt-1.5">
                                  <summary className="cursor-pointer text-[10.5px]" style={{ color: "var(--faint)" }}>
                                    Input schema
                                  </summary>
                                  <pre
                                    className="mt-1 max-h-52 overflow-auto rounded border p-2 font-mono text-[10.5px] whitespace-pre-wrap"
                                    style={{ borderColor: "var(--border)", background: "var(--code)" }}
                                  >
                                    {formatJson(tool.inputSchema)}
                                  </pre>
                                </details>
                              </div>
                            ))}
                          </div>
                        ) : (
                          <div className="text-[11.5px]" style={{ color: "var(--faint)" }}>
                            No tools advertised. Try Refresh.
                          </div>
                        )}
                      </div>
                    ) : null}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>

      <ServerForm
        server={editing}
        open={formOpen}
        onClose={() => {
          setFormOpen(false);
          setEditing(null);
        }}
      />

      <Modal
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        title="Delete MCP server?"
        width="sm"
        footer={
          <>
            <Button size="sm" variant="ghost" onClick={() => setDeleting(null)}>
              Cancel
            </Button>
            <Button size="sm" variant="danger" onClick={() => void deleteServer()}>
              Delete
            </Button>
          </>
        }
      >
        <p className="text-[13.5px]" style={{ color: "var(--muted)" }}>
          Its cached tools, permission rules and stored credentials will be removed. A running
          local process is stopped. Conversations keep their transcripts.
        </p>
      </Modal>

      <Modal
        open={report !== null}
        onClose={() => setReport(null)}
        title="Connection report"
        subtitle={report ? `${report.serverInfo?.name ?? "server"} · ${report.transport}` : ""}
        width="md"
      >
        {report ? (
          <div>
            <div className="flex flex-wrap gap-1.5">
              <Chip label={report.protocolVersion} accent />
              {Object.keys((report.capabilities ?? {}) as Record<string, unknown>).map((key) => (
                <Chip key={key} label={key} />
              ))}
              <Chip label={`${report.tools.length} tools`} />
            </div>
            {report.instructions ? (
              <p className="mt-3 text-[12.5px]" style={{ color: "var(--muted)" }}>
                {report.instructions}
              </p>
            ) : null}
            <div className="mt-3 grid gap-1">
              {report.tools.map((tool) => (
                <div key={tool.name} className="text-[11.5px]" style={{ color: "var(--muted)" }}>
                  <code className="font-mono" style={{ color: "var(--fg)" }}>
                    {tool.name}
                  </code>{" "}
                  {tool.description}
                </div>
              ))}
            </div>
          </div>
        ) : null}
      </Modal>

      <Modal
        open={running !== null}
        onClose={() => setRunning(null)}
        title={running ? `Run ${running.tool.name}` : "Run tool"}
        subtitle={running?.server.name}
        width="md"
        footer={
          <>
            <Button size="sm" variant="ghost" onClick={() => setRunning(null)}>
              Close
            </Button>
            <Button size="sm" variant="primary" loading={runBusy} onClick={() => void runTool()}>
              Run
            </Button>
          </>
        }
      >
        <label className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
          Arguments (JSON)
        </label>
        <textarea
          value={runArgs}
          onChange={(event) => setRunArgs(event.target.value)}
          className={inputClass}
          style={{ borderColor: "var(--border)", minHeight: 120, fontFamily: "var(--font-mono)" }}
          spellCheck={false}
        />
        {running ? (
          <details className="mt-2">
            <summary className="cursor-pointer text-[10.5px]" style={{ color: "var(--faint)" }}>
              Input schema
            </summary>
            <pre
              className="mt-1 max-h-40 overflow-auto rounded border p-2 font-mono text-[10.5px] whitespace-pre-wrap"
              style={{ borderColor: "var(--border)", background: "var(--code)" }}
            >
              {formatJson(running.tool.inputSchema)}
            </pre>
          </details>
        ) : null}
        {runResult ? (
          <>
            <div className="mt-3 mb-1 text-[11.5px]" style={{ color: "var(--muted)" }}>
              Result
            </div>
            <pre
              className="max-h-64 overflow-auto rounded-lg border p-2.5 font-mono text-[11.5px] whitespace-pre-wrap"
              style={{ borderColor: "var(--border)", background: "var(--code)" }}
            >
              {runResult}
            </pre>
          </>
        ) : null}
      </Modal>
    </div>
  );
}

function StatusDot({ status }: { status: string | null }) {
  const color =
    status === "connected"
      ? "var(--accent)"
      : status === "error"
        ? "var(--danger)"
        : "var(--faint)";
  return (
    <span
      className={cn("mt-1.5 h-2 w-2 shrink-0 rounded-full", status === "connected" ? "" : "opacity-80")}
      style={{ background: color }}
      title={status ?? "not checked"}
    />
  );
}

function Chip({ label, accent }: { label: string; accent?: boolean }) {
  return (
    <span
      className="rounded border px-1.5 py-0.5 text-[10.5px]"
      style={{
        borderColor: accent ? "var(--accent)" : "var(--border)",
        color: accent ? "var(--accent)" : "var(--muted)",
      }}
    >
      {label}
    </span>
  );
}
