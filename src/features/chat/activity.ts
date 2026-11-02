import type { ToolCallPayload } from "@/lib/types";
import { formatDuration } from "@/lib/format";
import { statsForDiff } from "./diff";

export type Tone = "muted" | "warn" | "danger";

export interface ToolSummary {
  /** Muted "server/" prefix for MCP tools; empty for built-ins. */
  prefix: string;
  label: string;
  /** Most relevant argument(s), already reduced to one line. */
  target: string;
  /** Short result metadata such as "+12 −3", "exit 0" or "21 matches". */
  result: string;
  tone: Tone;
  duration: string;
  /** "running", "queued" or "preparing" while in flight; empty once settled. */
  stateWord: string;
  needsApproval: boolean;
  /** The full text of the collapsed row, for the tooltip. */
  title: string;
}

const BUILTIN_SERVER = "mcp-chat";

const BUILTIN_LABELS: Record<string, string> = {
  read: "Read",
  write: "Write",
  edit: "Edit",
  apply_patch: "Patch",
  bash: "Bash",
  python: "Python",
  grep: "Grep",
  glob: "Glob",
  skill: "Skill",
  question: "Question",
  read_tool_output: "Read output",
  mcp_search_tools: "Search tools",
  mcp_get_tool_schema: "Tool schema",
  mcp_execute: "MCP batch",
};

// Large bodies never belong in a one-line preview.
const BODY_KEYS = new Set(["content", "oldString", "newString", "patchText", "code", "command"]);
const SENSITIVE_KEY = /pass(word|wd)?|secret|token|api[-_]?key|auth|credential|cookie|private[-_]?key/i;
const MINUS = "\u2212";

function str(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function oneLine(value: unknown, max: number): string {
  const text = String(value).replace(/\s+/g, " ").trim();
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/** Keep the last three path segments so long absolute paths stay readable. */
export function shortPath(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const segments = normalized.split("/").filter(Boolean);
  if (segments.length <= 3) return normalized;
  return `…/${segments.slice(-3).join("/")}`;
}

/** `key=value` pairs for primitive, non-sensitive arguments. */
function keyValues(args: Record<string, unknown>, skip: string[] = [], limit = 3): string[] {
  const out: string[] = [];
  for (const [key, value] of Object.entries(args)) {
    if (out.length >= limit) break;
    if (skip.includes(key) || BODY_KEYS.has(key) || SENSITIVE_KEY.test(key)) continue;
    if (typeof value !== "string" && typeof value !== "number" && typeof value !== "boolean") {
      continue;
    }
    const text = oneLine(value, 80);
    if (text === "") continue;
    out.push(`${key}=${text}`);
  }
  return out;
}

function firstLine(text: string, max: number): string {
  const line = text.split("\n").find((item) => item.trim() !== "") ?? "";
  return oneLine(line, max);
}

function countPatchFiles(patchText: string): number {
  return patchText.match(/^\*\*\* (?:Add|Update|Delete) File:/gm)?.length ?? 0;
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

function changeStats(diff: string | null | undefined): string {
  if (!diff) return "";
  const stats = statsForDiff(diff);
  return `+${stats.added} ${MINUS}${stats.removed}`;
}

export function summarizeTool(payload: ToolCallPayload): ToolSummary {
  const args = (payload.args ?? {}) as Record<string, unknown>;
  const state = payload.state ?? "running";
  const name = String(payload.name ?? "");
  const builtin = payload.server === BUILTIN_SERVER;
  let prefix = "";
  let label = name;
  let target = "";
  let result = "";
  let tone: Tone = "muted";

  if (name === "mcp_call_tool") {
    prefix = `${str(args.server) || payload.server || "mcp"}/`;
    label = str(args.tool) || name;
    const inner = args.arguments;
    if (inner && typeof inner === "object" && !Array.isArray(inner)) {
      target = keyValues(inner as Record<string, unknown>).join(" ");
    }
  } else if (builtin) {
    label = BUILTIN_LABELS[name] ?? name;
    switch (name) {
      case "read": {
        const path = str(args.filePath);
        target = [path && shortPath(path), ...keyValues(args, ["filePath"])]
          .filter(Boolean)
          .join(" ");
        break;
      }
      case "write":
      case "edit": {
        const path = str(args.filePath);
        target = path ? shortPath(path) : "";
        result = changeStats(payload.diff);
        break;
      }
      case "apply_patch": {
        const files = countPatchFiles(str(args.patchText));
        target = files > 0 ? plural(files, "file", "files") : "";
        result = changeStats(payload.diff);
        break;
      }
      case "bash":
      case "python": {
        const source = str(name === "bash" ? args.command : args.code);
        const lines = source.split("\n").filter((line) => line.trim() !== "");
        target = lines.length > 0 ? oneLine(lines[0]!, 200) + (lines.length > 1 ? " …" : "") : "";
        if (typeof payload.exitCode === "number") {
          result = `exit ${payload.exitCode}`;
          if (payload.exitCode !== 0) tone = "warn";
        }
        break;
      }
      case "grep":
      case "glob": {
        const path = str(args.path);
        const pattern = str(args.pattern);
        const include = str(args.include);
        target = [
          path && shortPath(path),
          pattern && `pattern=${oneLine(pattern, 200)}`,
          include && `include=${oneLine(include, 80)}`,
        ]
          .filter(Boolean)
          .join(" ");
        if (name === "grep" && state === "completed") {
          const head = firstLine(payload.resultPreview ?? "", 200);
          const found = /^(\d+) matching line/.exec(head);
          if (found) result = plural(Number(found[1]), "match", "matches");
          else if (head.startsWith("No matches")) result = "no matches";
        }
        break;
      }
      case "skill":
        target = str(args.name) || keyValues(args).join(" ");
        break;
      case "mcp_execute": {
        const calls = Array.isArray(args.calls) ? (args.calls as Record<string, unknown>[]) : [];
        const names = calls
          .slice(0, 3)
          .map((call) => `${str(call?.server) || "?"}/${str(call?.tool) || "?"}`);
        target =
          calls.length === 0
            ? ""
            : names.join(", ") + (calls.length > 3 ? ` +${calls.length - 3}` : "");
        break;
      }
      default:
        target = keyValues(args).join(" ");
    }
  } else {
    prefix = payload.server ? `${payload.server}/` : "";
    target = keyValues(args).join(" ");
  }

  if (state === "error") {
    const message = firstLine(payload.error ?? payload.resultPreview ?? "", 120);
    result = message || "failed";
    tone = "danger";
  } else if (state === "aborted") {
    result = "stopped";
    tone = "warn";
  }

  const stateWord =
    state === "streaming" ? "preparing" : state === "queued" || state === "running" ? state : "";
  const duration =
    payload.startedAt && payload.endedAt ? formatDuration(payload.endedAt - payload.startedAt) : "";
  const title = [`${prefix}${label}`, target, result, duration && `· ${duration}`]
    .filter(Boolean)
    .join(" ");

  return {
    prefix,
    label,
    target,
    result,
    tone,
    duration,
    stateWord,
    needsApproval: payload.approval === "pending",
    title,
  };
}

/** Duration of a thought, or "" when the payload carries no timing. */
export function thoughtDuration(payload: Record<string, unknown>): string {
  const direct = payload.durationMs;
  if (typeof direct === "number" && direct >= 0) return formatDuration(direct);
  const started = payload.startedAt;
  const ended = payload.endedAt;
  if (typeof started === "number" && typeof ended === "number" && ended >= started) {
    return formatDuration(ended - started);
  }
  return "";
}
