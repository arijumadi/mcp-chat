import { useId, useState, type ReactNode } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  CheckCircle2,
  Clock3,
  Copy,
  Loader2,
  XCircle,
} from "lucide-react";
import type { Part, ToolCallPayload, ToolImage } from "@/lib/types";
import { parsePayload } from "@/app/store";
import { formatJson } from "@/lib/format";
import { call } from "@/lib/ipc";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { cn } from "@/lib/cn";
import { ActivityRow } from "./ActivityRow";
import { DiffView } from "./DiffView";
import { capabilityOf } from "@/lib/capabilities";
import { summarizeTool, type Tone } from "./activity";

const toneColor: Record<Tone, string> = {
  muted: "var(--muted)",
  warn: "var(--warn)",
  danger: "var(--danger)",
};

const IN_FLIGHT = ["streaming", "queued", "running"];


/**
 * One tool call as a compact activity row. Collapsed it is a single line with
 * the tool, its main argument and result metadata. Expanding it reveals the
 * full arguments, result, image or error, and the file diff when there is one.
 */
export function ToolCard({ part }: { part: Part }) {
  const raw = parsePayload(part);
  const payload = raw as unknown as ToolCallPayload;
  const state = payload.state ?? "running";
  const exitCode = typeof payload.exitCode === "number" ? payload.exitCode : null;
  const nonzeroExit = state === "completed" && exitCode !== null && exitCode !== 0;
  const image = (payload.image ?? null) as ToolImage | null;
  const metaArgs = (payload.args ?? {}) as Record<string, unknown>;
  const isMetaCall = payload.name === "mcp_call_tool";
  const displayArgs =
    isMetaCall && metaArgs.arguments && typeof metaArgs.arguments === "object"
      ? (metaArgs.arguments as Record<string, unknown>)
      : (payload.args ?? {});
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  const [full, setFull] = useState<string | null>(null);
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [total, setTotal] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const toast = useApp((store) => store.toast);
  const [copied, setCopied] = useState(false);

  const summary = summarizeTool(payload);
  const inFlight = IN_FLIGHT.includes(state);
  // The diff only exists once the tool has finished; never show an empty one.
  const diff = typeof payload.diff === "string" && payload.diff !== "" && !inFlight ? payload.diff : null;
  const resultIsError = state === "error" && payload.resultPreview === payload.error;
  // A finished write, edit or patch opens straight onto its diff: no Arguments,
  // no Result, no second toggle. A failed call, or one without a diff, keeps the
  // regular body so its error and arguments stay visible.
  // Which tools carry a diff comes from the shared capability table.
  const isFileMutation = capabilityOf(payload.server, payload.name)?.producesDiff === true;
  const mutationDiff = isFileMutation && state === "completed" ? diff : null;

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
      <div className="my-px" data-tool-state={state} data-tool-name={payload.name}>
        <ActivityRow
          icon={<StatusIcon state={state} nonzeroExit={nonzeroExit} />}
          expanded={open}
          onToggle={() => setOpen((value) => !value)}
          controlsId={bodyId}
          title={summary.title}
          trailing={
            <>
              {summary.needsApproval ? (
                <span style={{ color: "var(--warn)" }}>needs approval</span>
              ) : null}
              {summary.stateWord ? <span>{summary.stateWord}</span> : null}
              {summary.duration ? (
                <span className="flex items-center gap-1">
                  <Clock3 size={10.5} aria-hidden /> {summary.duration}
                </span>
              ) : null}
            </>
          }
        >
          <span className="max-w-[40%] shrink-0 truncate font-medium">
            {summary.prefix ? (
              <span className="font-normal" style={{ color: "var(--muted)" }}>
                {summary.prefix}
              </span>
            ) : null}
            {summary.label}
          </span>
          <span
            className="min-w-0 flex-1 truncate font-mono text-[length:var(--fs-mono)]"
            style={{ color: "var(--fg)" }}
          >
            {summary.target}
          </span>
          {summary.result ? (
            <span
              className="max-w-[16rem] shrink-0 truncate text-[length:var(--fs-mono)]"
              style={{ color: toneColor[summary.tone] }}
            >
              {summary.result}
            </span>
          ) : null}
        </ActivityRow>

        {state === "running" && typeof raw.progress === "number" ? (
          <div className="mx-2 mt-0.5 mb-1">
            <div
              className="h-0.5 overflow-hidden rounded-full"
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

        {open ? (
          <div
            id={bodyId}
            className="mt-0.5 mb-2 ml-[15px] space-y-2.5 border-l pl-4 text-[length:var(--fs-row)]"
            style={{ borderColor: "var(--border)" }}
          >
            {mutationDiff ? (
              <DiffView diff={mutationDiff} defaultOpen />
            ) : (
              <>
                <Section label="Arguments">
                  <pre
                    className="max-h-56 overflow-auto rounded-md p-2.5 font-mono text-[length:var(--fs-mono)] leading-relaxed whitespace-pre-wrap"
                    style={{ background: "var(--code)" }}
                  >
                    {formatJson(displayArgs)}
                  </pre>
                </Section>

                {payload.resultPreview && !resultIsError ? (
                  <Section
                    label="Result"
                    action={
                      <span className="flex items-center gap-2">
                        {payload.truncated ? <span>truncated</span> : null}
                        <button
                          type="button"
                          onClick={() => void copyResult()}
                          className="focus-ring rounded p-0.5 hover:bg-[var(--panel-2)]"
                          title="Copy result"
                          aria-label="Copy result"
                        >
                          {copied ? <CheckCircle2 size={11} /> : <Copy size={11} />}
                        </button>
                      </span>
                    }
                  >
                    <pre
                      className="max-h-72 overflow-auto rounded-md p-2.5 font-mono text-[length:var(--fs-mono)] leading-relaxed whitespace-pre-wrap"
                      style={{ background: "var(--code)" }}
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
                  </Section>
                ) : null}

                {payload.error && state === "error" ? (
                  <div
                    role="alert"
                    className="rounded-md px-2.5 py-2 text-[length:var(--fs-mono)] whitespace-pre-wrap"
                    style={{ color: "var(--danger)", background: "var(--danger-soft)" }}
                  >
                    {payload.error}
                  </div>
                ) : null}

                {image && image.path ? (
                  <div>
                    {imageUrl(image.path) ? (
                      <img
                        src={imageUrl(image.path)}
                        alt={image.name}
                        className="max-h-56 max-w-full rounded-md border object-contain"
                        style={{ borderColor: "var(--border)" }}
                      />
                    ) : (
                      <div className="text-[length:var(--fs-mono)]" style={{ color: "var(--muted)" }}>
                        Image: {image.name} ({image.mime})
                      </div>
                    )}
                  </div>
                ) : null}

    {diff ? <DiffView diff={diff} /> : null}
              </>
            )}
          </div>
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
          className="max-h-[60vh] overflow-auto rounded-lg border p-3 font-mono text-[length:var(--fs-mono)] leading-relaxed whitespace-pre-wrap"
          style={{ borderColor: "var(--border)", background: "var(--code)" }}
        >
          {full}
        </pre>
        <div className="mt-3 flex items-center justify-between">
          <span
            className="text-[length:var(--fs-meta)]"
            style={{ color: "var(--faint)" }}
            title="Counts the formatted tool output, including line numbers; it can differ from the source file size."
          >
            {total != null ? `${full?.length ?? 0} of ${total} formatted chars` : ""}
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

function Section({
  label,
  action,
  children,
}: {
  label: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section>
      <div
        className="mb-1 flex items-center gap-2 text-[11px] font-medium"
        style={{ color: "var(--muted)" }}
      >
        <span>{label}</span>
        {action ? (
          <span className="ml-auto font-normal" style={{ color: "var(--faint)" }}>
            {action}
          </span>
        ) : null}
      </div>
      {children}
    </section>
  );
}

function imageUrl(path: string): string {
  if (!path || window.__MCP_CHAT_MOCK__ || !("__TAURI_INTERNALS__" in window)) return "";
  try {
    return convertFileSrc(path);
  } catch {
    return "";
  }
}

const STATE_NAME: Record<string, string> = {
  streaming: "preparing",
  queued: "queued",
  running: "running",
  completed: "completed",
  error: "failed",
  aborted: "stopped",
};

function StatusIcon({ state, nonzeroExit }: { state: string; nonzeroExit?: boolean }) {
  const label = nonzeroExit ? "completed with non-zero exit" : (STATE_NAME[state] ?? state);
  if (state === "completed") {
    return nonzeroExit ? (
      <AlertTriangle size={13} aria-label={label} role="img" style={{ color: "var(--warn)" }} />
    ) : (
      <CheckCircle2 size={13} aria-label={label} role="img" style={{ color: "var(--accent)" }} />
    );
  }
  if (state === "error" || state === "aborted") {
    return <XCircle size={13} aria-label={label} role="img" style={{ color: "var(--danger)" }} />;
  }
  if (state === "queued") {
    return <Clock3 size={13} aria-label={label} role="img" style={{ color: "var(--faint)" }} />;
  }
  return (
    <Loader2
      size={13}
      aria-label={label}
      role="img"
      className={cn("animate-spin")}
      style={{ color: "var(--warn)" }}
    />
  );
}
