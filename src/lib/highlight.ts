import hljs from "highlight.js/lib/common";

const LANGUAGE_BY_EXTENSION: Record<string, string> = {
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  jsx: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  rs: "rust",
  py: "python",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  json: "json",
  jsonc: "json",
  md: "markdown",
  mdx: "markdown",
  css: "css",
  scss: "scss",
  less: "less",
  html: "xml",
  htm: "xml",
  svg: "xml",
  xml: "xml",
  toml: "ini",
  ini: "ini",
  cfg: "ini",
  yaml: "yaml",
  yml: "yaml",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  sql: "sql",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  cs: "csharp",
  php: "php",
  rb: "ruby",
  swift: "swift",
  diff: "diff",
  patch: "diff",
  dockerfile: "dockerfile",
  tf: "hcl",
  hcl: "hcl",
};

const LANGUAGE_BY_NAME: Record<string, string> = {
  dockerfile: "dockerfile",
  makefile: "makefile",
  "cmakelists.txt": "cmake",
};

const MAX_HIGHLIGHT_CHARS = 200_000;
const MAX_HIGHLIGHT_LINES = 5_000;

export function languageForPath(path: string): string | null {
  const name = (path.split("/").pop() ?? path).toLowerCase();
  const byName = LANGUAGE_BY_NAME[name];
  if (byName) return byName;
  const dot = name.lastIndexOf(".");
  if (dot < 0) return null;
  return LANGUAGE_BY_EXTENSION[name.slice(dot + 1)] ?? null;
}

function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function closeTags(tags: string[]): string {
  return tags
    .slice()
    .reverse()
    .map((tag) => {
      const name = /^<\s*([a-zA-Z0-9-]+)/.exec(tag)?.[1] ?? "span";
      return `</${name}>`;
    })
    .join("");
}

/**
 * Split highlighted HTML at newlines without leaving spans open across rows.
 * Each line is self-contained, so every row can be rendered independently
 * while keeping the highlighter's colours for multi-line comments and strings.
 */
export function splitHighlightedLines(html: string): string[] {
  const lines: string[] = [];
  const open: string[] = [];
  let current = "";
  let index = 0;

  while (index < html.length) {
    const char = html[index];
    if (char === "<") {
      const end = html.indexOf(">", index);
      if (end === -1) {
        current += html.slice(index);
        break;
      }
      const tag = html.slice(index, end + 1);
      if (tag.startsWith("</")) {
        open.pop();
      } else if (!tag.endsWith("/>")) {
        open.push(tag);
      }
      current += tag;
      index = end + 1;
      continue;
    }
    if (char === "\n") {
      lines.push(current + closeTags(open));
      current = open.join("");
      index += 1;
      continue;
    }
    let next = index;
    while (next < html.length && html[next] !== "<" && html[next] !== "\n") {
      next += 1;
    }
    current += html.slice(index, next);
    index = next;
  }

  lines.push(current + closeTags(open));
  return lines;
}

export function highlightLines(content: string, path: string): string[] {
  const lines = content.split("\n");
  const language = languageForPath(path);
  if (
    !language ||
    content.length > MAX_HIGHLIGHT_CHARS ||
    lines.length > MAX_HIGHLIGHT_LINES ||
    !hljs.getLanguage(language)
  ) {
    return lines.map(escapeHtml);
  }

  try {
    const highlighted = splitHighlightedLines(
      hljs.highlight(content, { language, ignoreIllegals: true }).value,
    );
    return highlighted.length === lines.length ? highlighted : lines.map(escapeHtml);
  } catch {
    return lines.map(escapeHtml);
  }
}
