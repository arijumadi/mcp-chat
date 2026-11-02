export type DiffLineKind = "file" | "hunk" | "added" | "removed" | "context";

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

export interface DiffStats {
  added: number;
  removed: number;
  files: number;
  truncated: boolean;
}

/** Marker the backend appends when it caps a diff. */
export const DIFF_TRUNCATED_MARKER = "… diff truncated …";

/**
 * Classify the lines of a unified diff. A `--- ` line only counts as a file
 * header when the next line is `+++ `. A removed line whose own text starts
 * with dashes (a Markdown rule, a SQL comment) would otherwise be mistaken for
 * a header and dropped from the counts.
 */
export function parseDiff(diff: string): DiffLine[] {
  const text = diff.endsWith("\n") ? diff.slice(0, -1) : diff;
  if (text === "") return [];
  const raw = text.split("\n");
  const lines: DiffLine[] = [];
  for (let index = 0; index < raw.length; index += 1) {
    const line = raw[index]!;
    if (line.startsWith("--- ") && raw[index + 1]?.startsWith("+++ ")) {
      lines.push({ kind: "file", text: line });
      lines.push({ kind: "file", text: raw[index + 1]! });
      index += 1;
    } else if (line.startsWith("@@")) {
      lines.push({ kind: "hunk", text: line });
    } else if (line.startsWith("+")) {
      lines.push({ kind: "added", text: line });
    } else if (line.startsWith("-")) {
      lines.push({ kind: "removed", text: line });
    } else {
      lines.push({ kind: "context", text: line });
    }
  }
  return lines;
}

export function diffStats(lines: DiffLine[]): DiffStats {
  const stats: DiffStats = { added: 0, removed: 0, files: 0, truncated: false };
  for (const line of lines) {
    if (line.kind === "added") stats.added += 1;
    else if (line.kind === "removed") stats.removed += 1;
    else if (line.kind === "file" && line.text.startsWith("--- ")) stats.files += 1;
    else if (line.text === DIFF_TRUNCATED_MARKER) stats.truncated = true;
  }
  return stats;
}

export function statsForDiff(diff: string): DiffStats {
  return diffStats(parseDiff(diff));
}
