import table from "../../shared/capabilities.json";

/**
 * The built-in tool table, shared with the backend.
 *
 * `shared/capabilities.json` is the single source of truth for tool labels,
 * risk classes, which tools produce a diff, and the guard flags. The Rust side
 * renders descriptions, the routing table and guard messages from the same
 * file, so nothing here repeats a rule.
 */

export type Risk = "read_only" | "interactive" | "file_write" | "shell" | "code_exec";

interface RawGuard {
  override: string;
  overrideNeedsReview: boolean;
}

interface RawTool {
  name: string;
  label: string;
  risk: string;
  diff?: boolean;
  guard?: RawGuard;
}

export interface Capability {
  name: string;
  label: string;
  risk: Risk;
  /** The finished call carries a unified diff for the transcript. */
  producesDiff: boolean;
  /** Name of the boolean argument that lifts this tool's guard. */
  overrideParam: string | null;
}

/** The server name the backend reports for built-in tools. */
export const BUILTIN_SERVER = "mcp-chat";

const capabilities: Capability[] = (table.tools as RawTool[]).map((tool) => ({
  name: tool.name,
  label: tool.label,
  risk: tool.risk as Risk,
  producesDiff: tool.diff === true,
  overrideParam: tool.guard?.override ?? null,
}));

const byName = new Map(capabilities.map((capability) => [capability.name, capability]));

/** The capability of a built-in tool call, or undefined for an MCP tool. */
export function capabilityOf(server: string | undefined, name: string): Capability | undefined {
  return server === BUILTIN_SERVER ? byName.get(name) : undefined;
}

/** The tool filling a role such as `modify_file`. */
export function toolForRole(role: string): string | undefined {
  return (table.roles as Record<string, string>)[role];
}

export const ALL_CAPABILITIES: readonly Capability[] = capabilities;
