# Jarvis

A local-first desktop chat app for OpenAI-compatible and Anthropic models, with **remote and local stdio MCP tools**, explicit approvals, and no telemetry.

![Chat with a tool card](docs/screenshots/chat.png)

## What works (M0–M5)

- Streaming chat from any OpenAI-compatible `/chat/completions` endpoint and from Anthropic `/v1/messages`.
- Local SQLite transcripts with full-text search, inline rename, delete, regenerate and stop.
- Remote MCP over **Streamable HTTP 2026-07-28** (stateless headers, `server/discover`, request-scoped SSE progress, MRTR input), with legacy **2025-11-25** `initialize` fallback and deprecated **HTTP+SSE** fallback.
- Local MCP servers over **stdio**, including `npx` packages. Add a command, one argument per line, an optional working directory and optional environment variables; Jarvis keeps the child process alive between tool calls and stops it when the server is disabled, edited or deleted.
- The model can call discovered MCP tools inside a normal turn. Tool calls and reasoning are grouped into a collapsible activity summary with the final answer below; detail cards stream live (`queued → running → ok / error`) and run in parallel (max 4). Any text the model emits before a tool call stays in the stored message for context but is not rendered as an interim note — detail cards show only thoughts and tools.
- Per-chat MCP server selection with **lazy catalog + call** delivery. The MCP popover only selects which servers are active; there is no direct-schema mode. A compact server/tool/argument catalog goes in the system prompt, and the model calls `mcp_call_tool` directly (`mcp_get_tool_schema` only when it needs full JSON Schema; `mcp_search_tools` only when the catalog is truncated). `mcp_execute` runs up to 8 independent MCP calls in one step (4 in flight), so a multi-tool answer does not need many model round trips. Approval and policy checks still apply to each real target.
- The model picker sits in the composer footer; the trigger shows only the model name after the final `/`, provider choices show only the provider name, and long model lists are searchable. Export conversation lives in Settings → Export conversation.
- The composer accepts up to 6 attachments through the native file picker, by dragging files anywhere onto the chat window, or by pasting an image from the clipboard. Images (PNG/JPEG/GIF/WebP, max 5 MB each) are sent to vision models as native OpenAI/Anthropic image blocks. Text, code, and text-based PDF files are inlined into the user turn (up to 100k characters per file, 200k total); scanned PDFs and other binary files are copied into app data, shown in the transcript, and represented to the model by metadata only. Total attachments are capped at 25 MB. Files are copied and previewed from Rust-owned storage, cleaned up with their conversation, and restored to the composer when a message is reverted.
- Live prompt/completion usage in the composer and a step counter, with automatic token-based compaction using a structured handoff summary, ~15k recent-token retention and a 15%/16k buffer; the compactor reuses the normal system/tool prefix where provider caching applies.
- The transcript loads the latest 80 messages first and lazy-loads older pages when you scroll up; full history and compaction remain in SQLite.
- Workspace hygiene: `.jarvisignore` filters `glob`/`grep`, including last-match-wins negation, default `grep` skips non-text extensions, and `.mcp-chat/artifacts` + `.mcp-chat/tmp` are excluded from searches. When the selected workspace is a git repo, Jarvis adds `**/.mcp-chat/artifacts/` and `**/.mcp-chat/tmp/` to the repo's local `.git/info/exclude`, so runtime output is not committed without changing a tracked `.gitignore`; `.mcp-chat/skills` and `.mcp-chat/AGENTS.md` remain committable. `bash` supports `maxLines` with optional `tail`, while `read_tool_output` supports line-based paging. There is **no default step cap**; an optional per-conversation `max_steps` still supports the old **Continue** flow when explicitly configured.
- The desktop window remembers its size, position and maximized state between launches.
- A per-turn loop guard stops three identical tool calls with identical results, injects a strategy-change prompt, and forces one text-only step. While a turn is active, **Steer** stops the current turn and starts a fresh one with the new user message.
- Tool calls and thoughts show in the transcript as compact one-line rows, all collapsed by default. A tool row previews its main argument and result metadata (for example `Edit src/app.ts +12 −3`, `Patch 3 files +48 −12`, `Bash npm test exit 0`, `Grep … (21 matches)`); a thought row shows only **Thought** and its duration when known. Click a row for the full arguments, result, image or error. A thin header above the rows keeps the **Used N tools** summary with failed-call count and duration, and can hide the whole list. While reasoning is streaming the header shows **Thinking…**, and a running tool shows its name. Rows are collapsed again after a reload or chat switch.
- `edit`, `apply_patch` and `write` keep their diff after they finish: it is stored on the tool call, and one click on the row opens it directly (no Arguments or Result, no second toggle) with added and removed lines highlighted; a multi-file patch shows every file in one body. A failed call, or one without a diff, keeps the regular Arguments/Result/error view. The approval prompt uses the same diff view. The diff is display-only and is never sent to the model.
- An expanded **Thought** that is still streaming follows its newest text. Scroll up inside it to stop following, and scroll back near the bottom to resume. Only the thought body scrolls, not the transcript.
- Tool routing is enforced, not only prompted. `write` creates new files and refuses to replace an existing one unless `overwrite: true`. `bash` refuses commands that clearly write file contents (redirects to a file, `tee`, `sed -i`, `dd of=`, `Set-Content`/`Out-File`/`Add-Content`/`Clear-Content`/`Tee-Object`, `[IO.File]::Write*`, `New-Item -ItemType File`) unless `allowFileWrites: true`; build, test, git, package-manager and plain file-management commands (`mkdir`, `rm`, `mv`) are not affected. The check is a conservative routing aid, not a security boundary, and both refusals happen before any approval prompt. The approval prompt flags a bash call that sets `allowFileWrites`.
- **`/compact [focus]`** in the composer (or the palette, or `Ctrl/Cmd+Shift+K`) summarizes the older messages to free context. It asks first because it cannot be undone, shows the current context size, and takes an optional focus such as "keep the database decisions". It holds the conversation while it runs, so nothing can be sent meanwhile, and Stop cancels it.
- **Settings > Appearance** has density (comfortable or compact), chat text size (13 to 19 px) and chat width (narrow, default, wide, full). They apply at once and are remembered. The defaults are the original look.
- **Desktop notifications** (Settings > Notifications, on by default) tell you when a reply finishes, fails, or a tool waits for your approval or answer, while Jarvis is in the background or another chat is open. They show only the chat title and what happened, never your messages or commands. Jarvis asks the system for permission when you send a message, never from the background. There is no tray icon yet.
- **Update checks and automatic updates** (Settings > About) compare the running version with the latest public GitHub release. Check on startup is on by default and can be turned off; **Check for updates** runs it manually. On Windows NSIS and Linux AppImage, Jarvis can download, install and relaunch automatically, optionally without asking after the startup check. Linux `.deb` users still get the release notice and manual download link because `.deb` is owned by the system package manager.
- **Auto** in the composer enables approval-free calls for built-in tools in the current chat after a warning. In auto mode, `read`/`grep`/`glob` may also use explicit paths outside the workspace. Without auto mode, those outside reads ask for an explicit per-call approval instead of failing immediately. `write`/`edit`/`apply_patch` inside the workspace follow auto mode, but outside-workspace writes always require an explicit per-call approval and cannot be auto-approved. Destructive shell commands (`rm -rf`, `del`, `Remove-Item -Recurse`, `git reset --hard`, etc.) and patch file deletions still ask for approval; explicit deny policies still win. MCP still evaluates its server policy first; as before, auto mode can relax an MCP `Ask` to `Allow`, while `Deny` remains.
- Built-in local tools available in every chat: `bash`, `python`, `read`, `grep`, `glob`, `edit`, `apply_patch`, `write`, `skill` and `question`. `apply_patch` applies compact multi-file patches (add/update/move/delete) with CRLF-aware, whitespace-tolerant hunk matching, `@@` context anchors and `*** End of File` anchoring; file deletes still require approval even when auto-approve is on, and outside-workspace targets require explicit approval. `edit` replaces one exact string in one existing file, matches files as the model sees them (LF view) and tolerates typographic punctuation and trailing whitespace; use `apply_patch` for multiple hunks or files. File-mutation approval prompts include a unified diff when the target is inside the workspace. `bash` defaults to PowerShell on Windows and `sh` elsewhere; `cmd`/`pwsh`/`bash` remain explicit options. It supports a per-call `cwd`, labelled stdout/stderr and echo detection; PowerShell uses `-EncodedCommand` to avoid nested-quote failures. Absolute workspace paths are normalized (`\\?\`, case and trailing separators) consistently across filesystem tools. File writes stay inside the workspace by default; outside-workspace `write`/`edit`/`apply_patch` targets always require explicit approval. `read`/`grep`/`glob` can use explicit external paths after per-call approval, or without approval in auto mode. `bash`, `edit`, `apply_patch` and `write` ask for approval by default. `python` runs complete source through stdin with UTF-8 environment, a workspace `cwd`, and timeout, avoiding shell quoting. Shell exit codes are part of the command result; a non-zero exit does not by itself mark the tool call as failed. `question` asks the user one or more blocking questions in a dialog and blocks the turn until they answer or decline. `options` are optional: a question with options shows a picker plus a field for a custom answer, and a question without them is a plain text field, so it also works for free-form answers. The model is told to use it only when the answer changes what it does next and the workspace cannot tell it; status updates and explanations belong in the final answer.
- Provider models are fetched once in the provider form and saved with the provider; the chat model picker uses that saved list. Model is optional on the provider.
- Approval policies per server (`auto` / `ask` / `deny`), “allow once”, “always allow” rules, rejection with feedback delivered back to the model. The approval dialog previews the exact rule that “Always allow” will save; shell commands use a conservative prefix pattern such as `cat *`, while risky, compound, or unknown commands keep the exact command. Saved rules apply across chats.
- Tool output is capped at 12k characters for the model, 16k for `bash`/`python`, 24k for `read`, and 24k per `read_tool_output` page. The full payload spills to disk and can be paged without re-truncating the stored result.
- Token-based compaction on context overflow, `finish_reason: length` continuations, retries with backoff, interrupted-stream continuation.
- Markdown with code blocks (language label, copy, horizontal scroll), tables, reasoning sections and a strict no-raw-HTML renderer.
- Default system prompt is **Jarvis**, still editable and resettable in Settings.
- The model receives the local day, date, hour and minute (minute precision) stamped on each user message. Time is injected into the user turn, not the system prompt, so the system + tool catalog prefix stays byte-identical. The timestamp is rendered once at send time and stored as a hidden `time` part, so historical user messages stay byte-identical across later requests; the UI never shows that raw marker.
- Hovering a user message shows its local date and time below the bubble. Hovering any user or assistant message reveals **Copy**; assistant Markdown is copied as rendered rich HTML plus rendered plain text, so pasting into Word/Docs keeps headings, bold, lists, tables and code blocks. User messages also show **Revert**, which asks for confirmation, removes that message and everything after it, then puts the original text and attachments back in the composer for editing and resending.
- The system prompt is Jarvis plus non-editable harness/tool rules, a workspace environment block, optional `AGENTS.md` instructions, and a skill catalog. The timestamp stays on the user turn, so the static prefix remains cache-friendly.
- Each conversation has its own workspace, selectable from the composer's **Workspace** chip with a native folder dialog. New chats inherit the current chat's workspace; existing chats fall back to the previous global default or the process directory. File writes and `bash` use the per-chat workspace; read-only tools may read explicit outside paths in auto mode. Optional project instructions are read from `AGENTS.md` in the home config, app data, workspace root, `.mcp-chat`, and `.opencode`, in that order. A `skill` is a directory containing `SKILL.md` under `.mcp-chat/skills/<name>`, `.opencode/skills/<name>`, app-data `skills`, `~/.config/opencode/skills`, or `~/.config/mcp-chat/skills`. The `skill` tool can also read a file or list a subfolder inside the selected skill, but never outside that skill directory. Jarvis bundles `excalidraw`, `spreadsheet`, and `manage-skill`, and seeds them into `~/.config/mcp-chat/skills` at startup. The installer tracks file hashes in `.jarvis-builtins.json`: untouched built-ins are refreshed, while files you edit are left alone. Use `manage-skill` to create or update a global skill without editing the app bundle. The **Skills** page lists the effective catalog with its source and path, and a global toggle enables or disables each name. Disabled skills are removed from the prompt and rejected by the `skill` tool; the choice applies to every workspace and every new chat. `spreadsheet` still requires Python 3 and `openpyxl`; Excalidraw link uploads require `cryptography`.
- The transcript renders the newest 80 messages first with **Show earlier messages**; conversation lists and the export selector can load older pages, and conversations can be pinned to the top. Settings has a sticky section navigator. The first-run empty state includes a setup checklist for provider, key and optional MCP server. Error toasts stay longer, can be copied, and expose Retry where the action is useful; modals trap and restore focus. Provider/MCP chips show warning dots when a key or connection is missing.
- Providers, keys and MCP servers are managed from the UI. API keys live in the OS keychain; the webview never receives them back.

Packaging is configured for Windows (NSIS + MSI), macOS (app + dmg) and Linux (deb + rpm + AppImage). See `PACKAGING.md`. Installers must be built on the target OS; this headless repository does not produce native bundles.

Not in this batch: OAuth 2.1/DCR (M6). Static bearer tokens and an `X-API-Key` header (stored in the OS keychain) for MCP work.

## Tool loading and context

Use the **Tools** button in the chat header only to choose which MCP servers this conversation may use. Tool delivery is always **lazy catalog + call**: the model gets a compact catalog of servers, tool names, short descriptions and argument summaries, plus `mcp_call_tool`, `mcp_get_tool_schema` and `mcp_execute`. It calls one target directly, or batches up to 8 independent targets in a single `mcp_execute` call; it only falls back to `mcp_search_tools` if the catalog was truncated. This avoids the old search→schema→call loop. Approvals and server policy still apply to every real target.

The composer shows live prompt/completion usage. Compaction is automatic using the thresholds in **Settings → Context management** (keep the most recent ~15k tokens, 15%-of-window buffer with a 16k floor); there is no manual compact button. During summarization the transcript shows **Compacting context…**, and if UI event delivery falls behind it reloads the active transcript instead of leaving a half-streamed message stuck.

## Quickstart

Requirements: Node.js 22+, pnpm 10+, Rust 1.90+, and the Tauri v2 Linux prerequisites on Linux.

```bash
pnpm install
pnpm tauri dev
```

On Debian/Ubuntu/Mint, the Tauri prerequisites are:

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

If you built this on a headless server and need to move it to a desktop, follow **[RUN-ON-DESKTOP.md](RUN-ON-DESKTOP.md)**.

Then in the app:

1. **Settings → Add provider.** For a local OpenAI-compatible server use e.g. `http://127.0.0.1:11434/v1` and a model name; keys are optional for localhost.
2. **MCP servers → Add server.** Use an HTTPS MCP endpoint (or localhost during development), choose a policy and run **Test connection**. For a local server choose **Local command (stdio / npx)**, set Command to `npx`, then add the package and arguments one per line, for example `-y`, `@modelcontextprotocol/server-filesystem`, `/path/to/folder`.
3. Create a chat and ask the model to use one of the server's tools.

## Mock server walkthrough

The repository ships an axum mock that implements three MCP transport generations plus a scripted LLM provider. It is the same server used by the Rust integration tests.

```bash
cargo run -p mcp-mock
```

It prints:

```
MCP 2026-07-28 : http://127.0.0.1:8787/mcp
MCP 2025-11-25 : http://127.0.0.1:8789/mcp
MCP HTTP+SSE   : http://127.0.0.1:8790/sse
OpenAI-compat  : http://127.0.0.1:8788/v1
```

Add a provider with base URL `http://127.0.0.1:8788/v1` and model `mock-tool`. Add an MCP server with URL `http://127.0.0.1:8787/mcp`, auth **none**, policy **ask**. Send “Use the echo tool to say hello”, then approve the call.

The mock exposes:

| Tool | Behaviour |
|---|---|
| `echo(text)` | returns `echo: <text>` |
| `add(a, b)` | returns `structuredContent.sum` |
| `slow_progress(seconds)` | emits three progress notifications on the POST SSE stream |
| `fail()` | returns JSON-RPC error `-32603` |
| `elicit(question)` | returns MRTR `input_required`; retry carries the form response |
| `big(n)` | returns 50 KB of text, forcing spill + `read_tool_output` |
| `header_echo(region)` | mirrors `region` into `Mcp-Param-Region` (x-mcp-header) |
| `sleep(seconds)` | test-only delayed tool used by the cancellation test |

Provider model names select scripted scenarios: `mock-text`, `mock-tool`, `mock-parallel`, `mock-length`, `mock-cut`, `mock-overflow`, `mock-weird-finish`, `mock-elicit`, `mock-big`, `mock-slow`, `mock-fail`, `mock-header`, `mock-always-tool`, `mock-badjson`, `mock-flaky`. See `tests/mock_servers/src/provider.rs`.

## Verification

Everything below **was run in this repository** on Linux without a GUI/WebKit development environment:

```bash
pnpm lint          # ESLint
pnpm typecheck     # TypeScript
pnpm test          # Vitest: 31 tests
pnpm build         # Vite production build
pnpm version:check # package.json, Cargo workspace and Tauri version agree
pnpm package       # native installer for the current OS
pnpm fmt:check     # cargo fmt --check
pnpm core:test     # 85 unit + 32 agent + 6 MCP integration + 1 auth + 1 provider test = 125 Rust tests
pnpm core:clippy   # cargo clippy -p mcp-chat-core --all-targets -- -D warnings
pnpm mock:clippy   # same for the mock server
pnpm test:e2e      # Playwright: 6 smoke + 6 visual-regression tests against vite preview
```

`pnpm check` runs all of the above except Playwright.

The Rust tests cover the acceptance matrix: split tool arguments, two parallel calls, invalid arguments, unknown tools, `finish_reason: length`, a cut stream, context overflow + guided/cache-aware compaction, manual compaction/context status, per-chat server selection, lazy MCP catalog/schema/call, batch execute, read_tool_output paging, built-in file/shell/search/skill/apply_patch tools, shell output labels/echo handling, shell-not-found errors, CLIXML cleanup, prompt composition, AGENTS/skill catalog, auto-approve/dangerous-command guard, built-in question round trip (with and without options), loop guard, continue-turn resume, default/custom system prompt, cache-stable timestamps in user turns, revert-to-composer, attachment staging/text inlining/image content blocks, unknown finish reason, policy denial, user rejection with feedback, crash sweep, progress notifications, MRTR elicitation, output spill/paging, optional max steps, cancellation during a tool, and retryable provider errors.

**What is not verified on this machine:** the Tauri shell itself could not be compiled or launched here because the machine has no WebKit/GTK development headers and no display. `pnpm tauri dev` is the intended path on a desktop with the prerequisites above. The frontend was verified in a browser through Playwright using a stubbed IPC bridge; screenshots are from that harness, not the native window. No real provider API key was used for automated tests; the keychain path is exercised with an in-memory store in tests.

## Screenshots

| Settings | MCP servers |
|---|---|
| ![Settings](docs/screenshots/settings.png) | ![MCP servers](docs/screenshots/servers.png) |

## Keyboard shortcuts

`Ctrl` on Windows and Linux, `Cmd` on macOS. The list below is generated from the command registry, which also feeds the palette and **Settings > Keyboard shortcuts**.

| Shortcut | Action |
|---|---|
| `Ctrl/Cmd+Shift+P` | Command palette (every command below, and why one is unavailable) |
| `Ctrl/Cmd+K` | Search chats. Typing `>` there opens the palette |
| `Ctrl/Cmd+N` | New chat |
| `Ctrl/Cmd+Shift+K` | Compact the conversation (asks first) |
| `Ctrl/Cmd+Shift+M` | Switch model |
| `Ctrl/Cmd+Shift+O` | Switch workspace |
| `Ctrl/Cmd+B` | Toggle the sidebar |
| `Ctrl/Cmd+,` | Settings |
| `Esc` | Stop the current turn (from the message box) |
| `Ctrl/Cmd+Enter` | Allow the pending tool call once |
| `Ctrl/Cmd+Shift+Enter` | Always allow the pending tool call |
| `Ctrl/Cmd+Backspace` | Reject the pending tool call |
| `Enter` / `Shift+Enter` | Send / newline |

The approval shortcuts never fire while you are typing in a text field, so writing a rejection reason cannot approve the call by accident.

## Privacy and security

- No accounts, cloud sync, telemetry or crash reporting.
- All network I/O happens in Rust. The webview never calls LLM or MCP endpoints. Update checks are the only optional GitHub request: they run on startup (unless disabled) or when you press **Check for updates**, and send no message content or identifiers.
- API keys and MCP bearer tokens are stored in the OS keychain (`com.mcpchat.app`), not SQLite or `localStorage`.
- Markdown raw HTML is disabled, links are opened in the system browser, and remote images are not loaded.
- MCP server URLs must be HTTPS, except loopback for development. Local stdio servers run as child processes with your user permissions; only add commands and packages you trust. Stdio environment values are stored in the local SQLite database.
- Request logs never include message content; `Authorization`/`Cookie` values are not logged.
- CSP is strict and `connect-src` is limited to Tauri IPC. The only deviation from the prompt's CSP is the required `ipc:` / `http://ipc.localhost` entries for Tauri's invoke bridge; it is documented in `ARCHITECTURE.md`.

## Repository layout

```
crates/mcp-chat-core/   db, llm, mcp, secrets, agent loop (testable without Tauri)
src-tauri/              thin Tauri shell: commands, events, capabilities
src/                    React 19 + TypeScript + Tailwind 4 renderer
tests/mock_servers/     axum MCP + OpenAI/Anthropic mock server
e2e/                    Playwright smoke and visual-regression tests
docs/screenshots/       UI captures from the mock bridge
PACKAGING.md             Windows/macOS/Linux installer and signing guide
```

## Scope and next steps

M6 (OAuth 2.1 + PKCE + DCR + loopback + refresh/Reconnect) is still not implemented. M7 packaging is configured; build the native installers on the target OS with `PACKAGING.md`. Automatic updates work for Windows NSIS and Linux AppImage; Linux `.deb` stays manual, and MSIX is future work. See `ARCHITECTURE.md` for the remaining protocol gaps.
