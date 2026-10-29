import { useState } from "react";
import {
  CheckCircle2,
  ChevronDown,
  Clock3,
  Copy,
  Loader2,
  Wrench,
  XCircle,
} from "lucide-react";
import type { Part, ToolCallPayload } from "@/lib/types";
import { parsePayload } from "@/app/store";
import { formatDuration, formatJson } from "@/lib/format";
import { call } from "@/lib/ipc";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { cn } from "@/lib/cn";

const stateLabel: Record<string, string> = {
  streaming: "preparing",
  queued: "queued",
  running: "running",
  completed: "ok",
  error: "error",
  aborted: "stopped",
};

export function ToolCard({ part }: { part: Part }) {
  const raw = parsePayload(part);
  const payload = raw as unknown as ToolCallPayload;
  const state = payload.state ?? "running";
  const metaArgs = (payload.args ?? {}) as Record<string, unknown>;
  const isMetaCall = payload.name === "mcp_call_tool";
  const isBatch = payload.name === "mcp_execute";
  const batchCalls = isBatch && Array.isArray(metaArgs.calls) ? metaArgs.calls.length : 0;
  const displayName = isMetaCall
    ? String(metaArgs.tool ?? payload.name)
    : isBatch
      ? `${payload.name} · ${batchCalls} call${batchCalls === 1 ? "" : "s"}`
      : payload.name;
  const displayServer = isMetaCall
    ? String(metaArgs.server ?? payload.server)
    : payload.server;
  const displayArgs =
    isMetaCall && metaArgs.arguments && typeof metaArgs.arguments === "object"
      ? (metaArgs.arguments as Record<string, unknown>)
      : (payload.args ?? {});
  const [full, setFull] = useState<string | null>(null);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [total, setTotal] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const toast = useApp((store) => store.toast);
  const [copied, setCopied] = useState(false);

  const duration =
    payload.startedAt && payload.endedAt
      ? formatDuration(payload.endedAt - payload.startedAt)
      : "";

  const showAll = async (offset = 0) => {
    setLoading(true);
    try {
      const value = await call<string>("read_tool_output", {
        callId: payload.callId,
        offset,
      });
      try {
        const parsed = JSON.parse(value) as {
          content?: string;
          next_offset?: number;
          total?: number;
        };
        const chunk = parsed.content ?? value;
        setFull((current) => (offset === 0 || !current ? chunk : current + chunk));
        setTotal(parsed.total ?? null);
        setNextOffset(
          parsed.next_offset != null && parsed.total != null && parsed.next_offset < parsed.total
            ? parsed.next_offset
            : null,
        );
      } catch {
        setFull(value);
        setNextOffset(null);
      }
    } catch (error) {
      toast(String(error), "error");
    } finally {
      setLoading(false);
    }
  };

  const copyResult = async () => {
    try {
      await navigator.clipboard.writeText(payload.resultPreview ?? "");
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // optional
    }
  };

  return (
    <>
      <div
        className="my-2.5 overflow-hidden rounded-xl border text-[12.5px]"
        style={{ borderColor: "var(--border)", background: "var(--panel)" }}
      >
        <div className="flex items-center gap-2 px-3 py-2.5">
          <StatusIcon state={state} />
          <Wrench size={13} className="shrink-0" style={{ color: "var(--muted)" }} />
          <span className="min-w-0 truncate font-medium" title={displayName}>
            {displayName}
          </span>
          <span
            className="truncate rounded border px-1.5 py-0.5 text-[10.5px]"
            style={{
              borderColor: "var(--border)",
              color: "var(--muted)",
              maxWidth: 130,
            }}
            title={displayServer}
          >
            {displayServer || "unknown"}
          </span>
          <span
            className="ml-auto flex items-center gap-1.5 text-[10.5px]"
            style={{ color: "var(--faint)" }}
          >
            {payload.approval === "pending" ? (
              <span style={{ color: "var(--warn)" }}>needs approval</span>
            ) : null}
            {duration ? (
              <>
                <Clock3 size={10.5} /> {duration}
              </>
            ) : null}
            <span>{stateLabel[state] ?? state}</span>
          </span>
        </div>

        {state === "running" && typeof raw.progress === "number" ? (
          <div className="px-3 pb-2">
            <div
              className="h-1 overflow-hidden rounded-full"
              style={{ background: "var(--panel-2)" }}
            >
              <div
                className="h-full rounded-full transition-all"
                style={{
                  width: `${Math.round((raw.progress as number) * 100)}%`,
                  background: "var(--accent)",
                }}
              />
            </div>
          </div>
        ) : null}

        <details
          className="group border-t"
          style={{ borderColor: "var(--border)" }}
        >
          <summary
            className="flex cursor-pointer list-none items-center gap-1.5 px-3 py-1.5 text-[11.5px] select-none"
            style={{ color: "var(--muted)" }}
          >
            <ChevronDown
              size={12}
              className="transition group-open:rotate-180"
            />
            Arguments
          </summary>
          <div className="px-3 pb-3">
            <pre
              className="max-h-56 overflow-auto rounded-lg border p-2.5 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap"
              style={{ borderColor: "var(--border)", background: "var(--code)" }}
            >
              {formatJson(displayArgs)}
            </pre>
          </div>
        </details>

        {payload.error && state === "error" ? (
          <div
            className="border-t px-3 py-2 text-[11.5px]"
            style={{
              borderColor: "var(--border)",
              color: "var(--danger)",
              background: "var(--danger-soft)",
            }}
          >
            {payload.error}
          </div>
        ) : null}

        {payload.resultPreview ? (
          <details
            className="group border-t"
            style={{ borderColor: "var(--border)" }}
            open={state !== "completed" ? true : undefined}
          >
            <summary
              className="flex cursor-pointer list-none items-center gap-1.5 px-3 py-1.5 text-[11.5px] select-none"
              style={{ color: "var(--muted)" }}
            >
              <ChevronDown size={12} className="transition group-open:rotate-180" />
              Result
              {payload.resultPreview ? (
                <span
                  className="ml-1 min-w-0 flex-1 truncate font-normal"
                  style={{ color: "var(--faint)" }}
                  title={payload.resultPreview}
                >
                  {payload.resultPreview.split("\n")[0]}
                </span>
              ) : null}
              <span className="ml-auto flex items-center gap-2">
                {payload.truncated ? <span>truncated</span> : null}
                <button
                  onClick={(event) => {
                    event.preventDefault();
                    void copyResult();
                  }}
                  className="focus-ring rounded p-0.5 hover:bg-[var(--panel-2)]"
                  title="Copy result"
                >
                  {copied ? <CheckCircle2 size={11} /> : <Copy size={11} />}
                </button>
              </span>
            </summary>
            <div className="px-3 pb-3">
              <pre
                className="max-h-72 overflow-auto rounded-lg border p-2.5 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap"
                style={{ borderColor: "var(--border)", background: "var(--code)" }}
              >
                {payload.resultPreview}
              </pre>
              {payload.fullResultRef ? (
                <div className="mt-2 flex justify-end">
                  <Button size="sm" variant="ghost" loading={loading} onClick={() => void showAll()}>
                    Show all
                  </Button>
                </div>
              ) : null}
            </div>
          </details>
        ) : null}
      </div>

      <Modal
        open={full !== null}
        onClose={() => setFull(null)}
        title={payload.name}
        subtitle={payload.server}
        width="xl"
      >
        <pre
          className="max-h-[60vh] overflow-auto rounded-lg border p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap"
          style={{ borderColor: "var(--border)", background: "var(--code)" }}
        >
          {full}
        </pre>
        <div className="mt-3 flex items-center justify-between">
          <span className="text-[10.5px]" style={{ color: "var(--faint)" }}>
            {total != null ? `${full?.length ?? 0} of ${total} characters` : ""}
          </span>
          {nextOffset != null ? (
            <Button
              size="sm"
              variant="secondary"
              loading={loading}
              onClick={() => void showAll(nextOffset)}
            >
              Load more
            </Button>
          ) : null}
        </div>
      </Modal>
    </>
  );
}

function StatusIcon({ state }: { state: string }) {
  if (state === "completed") {
    return <CheckCircle2 size={14} className="shrink-0" style={{ color: "var(--accent)" }} />;
  }
  if (state === "error" || state === "aborted") {
    return <XCircle size={14} className="shrink-0" style={{ color: "var(--danger)" }} />;
  }
  if (state === "queued") {
    return <Clock3 size={13} className="shrink-0" style={{ color: "var(--faint)" }} />;
  }
  return (
    <Loader2
      size={13}
      className={cn("shrink-0 animate-spin")}
      style={{ color: "var(--warn)" }}
    />
  );
}
