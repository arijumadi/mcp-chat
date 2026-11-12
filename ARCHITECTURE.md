# Architecture

## Data flow

```
┌──────────────────────────────────────────────────────────────┐
│ React 19 webview (renderer only)                             │
│  store.ts        typed IPC wrapper         markdown / cards  │
└───────────────▲───────────────────────────────┬──────────────┘
                │ chat://delta / tool / status   │ invoke(command)
┌───────────────┴───────────────────────────────▼──────────────┐
│ src-tauri (thin shell)                                       │
│  setup: paths, logging, DB, keychain, events, startup sweep   │
│  commands: chat, attachments, providers, servers, settings   │
└───────────────▲───────────────────────────────┬──────────────┘
                │ CoreEvent (broadcast)          │ calls
┌───────────────┴───────────────────────────────▼──────────────┐
│ crates/mcp-chat-core                                         │
│  db      sqlx/SQLite: conversations, messages, parts, FTS5    │
│  llm     OpenAI-compatible + Anthropic SSE, retries, agent    │
│  mcp     JSON-RPC client, modern/legacy/SSE, tool cache       │
│  agent   turn state machine, approvals, MRTR, compaction      │
│  secrets SecretStore trait → OS keyring                      │
└───────────────┬───────────────────────────────┬──────────────┘
                │ reqwest (no redirects)         │ files
        LLM endpoint / MCP server          tool-output/*.json, attachments/
```

There is no API key, MCP call, or provider request in the webview. The frontend only invokes commands and listens to three event channels.

## Persistence

SQLite in WAL mode, foreign keys on, migrations via `sqlx::migrate!`.

- `conversations` — title, timestamps, provider/model, per-conversation `workspace_root`, system prompt, archived/pinned flags, `max_steps` (0 = unlimited by default).
- `messages` — role, status (`completed`, `streaming`, `interrupted`, `error`, `synthetic`), error.
- `parts` — ordered typed JSON payloads: `text`, `attachment`, `reasoning`, `tool_call`, `tool_result`, `error`, `summary`, and a hidden `time` marker used for the user-message timestamp.
- `providers` — kind, base URL, model, `api_key_ref` account id, context window, max output.
- `servers` — URL, transport, auth kind, headers JSON, policy, negotiated version, capability/server info, cached tools + timestamp, OAuth issuer/client id.
- `settings`, `permission_rules`.
- `messages_fts` — FTS5 over text parts, maintained by insert/update/delete triggers.

A tool call is one part on the assistant message; its terminal state and result preview live in that payload. A separate `tool` message holds the result block sent back to the provider. The UI hides `tool` messages and renders each call as a compact activity row instead. This keeps the provider transcript valid while giving the renderer a one-line row. Assistant text emitted before the last tool call is retained for provider context but omitted from the activity list; only thoughts, tool rows and the post-tool answer are shown.

File diffs follow the same rule: the transcript renders `tool_call` parts only, so `ToolExecutor::finish` moves the `diff` that `edit`, `apply_patch` and `write` return in their structured output onto the `tool_call` payload (and onto the live `tool` event) and removes it from the persisted `tool_result`. It is built from the text captured before the write, never by re-reading the file, and capped at about 48k characters with a truncation marker. Only the `content` of a tool result reaches the model, so the diff is display-only. Only built-in file tools are treated this way; an external MCP server's own `diff` field is left untouched.

Tool routing guards run in `ToolExecutor::execute` before the permission check, so a call that is certain to be refused never reaches an approval prompt; `run_write` and `run_bash` repeat the same checks as defense in depth. `write` rejects an existing file unless `overwrite` is true (new files are created with `create_new`, so a file that appears after the check is still not replaced). `agent/shell_guard.rs` decides whether a bash command writes file contents: it tokenizes the command, ignores quoted text, heredoc bodies and comments, looks through wrappers such as `sudo`/`xargs`/`find -exec`, and inspects the payload of `sh -c`/`powershell -Command`/`cmd /c`. It is deliberately conservative and is not a security boundary. The routing table and a one-line self-check live in the harness rules once, not in every tool schema, which stays under the `builtin_tool_schema_stays_compact` budget.

Synthetic user messages (continue prompts, max-step notices) are ordinary rows with `status = 'synthetic'`; they are part of the provider context but hidden from the transcript and skipped by Regenerate. Reverting a user message deletes that row and every later row with `delete_from`, removes matching spilled tool files, returns the original text to the composer, and resets the title to “New chat” when nothing remains.

## MCP negotiation

| Probe / situation | Result |
|---|---|
| `server/discover` succeeds and supports `2026-07-28` (or no version list) | Modern stateless client. Every POST carries `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name` when applicable, and per-request `_meta`. No `initialize`, no session id, no GET stream. |
| `server/discover` fails with 400/404/405 without a recognized modern error, `-32601`, or `-32022` where only `2025-11-25` is offered | Legacy client: `initialize` → `notifications/initialized`, keep `Mcp-Session-Id`, protocol header on later requests. |
| Legacy `initialize` fails with 404/405/transport error, or the URL ends in `/sse` | Deprecated HTTP+SSE: GET the URL, wait for the `endpoint` event, POST to it, correlate responses from the long-lived GET stream. |
| Any modern error such as `-32020` header mismatch or `-32021` missing capability | No fallback; humanized error surface. |
| Cached `2025-11-25` protocol in SQLite | Try legacy directly; on failure clear the assumption and re-probe. |
| Transport is `stdio` | Spawn the configured command, exchange newline-delimited JSON-RPC over stdin/stdout, run the `2025-11-25` `initialize` handshake once, and keep the child process alive until the server is edited, disabled or deleted. `npx`/`npm`/`pnpm`/`yarn`/`bunx`/`uvx` are wrapped through `cmd.exe` on Windows. Stderr is drained to the app log. |

Tool mapping:

- Exposed name = sanitized `{server_slug}__{remote_name}`, max 64 chars, collision suffix `_2`.
- The remote name is sent to the server; the namespaced name is only for the model/UI.
- `inputSchema` is passed through byte-for-byte (JSON Schema 2020-12).
- `x-mcp-header` annotations are validated; invalid definitions are dropped and never offered to the model. Valid values are mirrored as `Mcp-Param-*` with the spec's Base64 sentinel when needed.
- Tool result precedence: `structuredContent` → concatenated `content[]` text → raw JSON. The model sees at most 12k characters (16k for bash, 24k for read); the full payload is spilled and paged via `read_tool_output` in 24k-character pages.
- Tool lists are cached per server with the server `ttlMs`, capped at 10 minutes. Stale cache is used if a refresh fails.

MRTR: an `input_required` result opens the elicitation form; the retry carries `inputResponses` and the exact opaque `requestState`, with a new JSON-RPC id. Maximum 3 round trips. Sampling/roots are declined by the UI.

Progress: notifications arrive on the POST response SSE stream and update the existing tool card. A broken stream re-issues the request with a new id.

## Agent loop

One turn = one or more steps. A step is one provider request plus the tool executions it caused.

```
prepare context
  ├─ load summary + messages after its boundary
  ├─ build canonical ChatMessage[] (repair dangling tool calls)
  ├─ estimate tokens; compact at ~80% of context
  └─ keep tool schemas even when tool_choice = none
stream step
  ├─ text/reasoning parts are created lazily and flushed every ~250 ms
  ├─ dispatch a tool as soon as its arguments parse as a JSON object
  ├─ max 4 in flight (semaphore); settle in original call order
  ├─ approval / MRTR may block one call while others run
  └─ persist results before the next step
outcome
  ├─ stop + no tools → end
  ├─ tools → next step
  ├─ finish_reason length → synthetic continue (max 2)
  ├─ stream cut after output → keep partial + synthetic continue
  ├─ retryable error before output → backoff 250 ms / 2 s / 8 s, max 3
  ├─ context overflow before output → compact once + retry same step
  └─ optional max steps → synthetic notice, tool_choice none, errors for any calls
```

Settlement invariant: every tool call gets exactly one terminal result, including unknown tools, schema failures, JSON-RPC errors, user rejection, cancellation, missing provider results and crash recovery. Startup `sweep_interrupted` marks dangling parts `aborted`; the context builder also synthesizes a tool result for any historic call that lacks one, so a crash never produces a transcript the provider rejects.

Per-server policy defaults to `ask`; rules are evaluated last-match-wins with `*` globs on action (`mcp.<slug>.<tool>`) and a normalized resource (URL host/path/channel or `*`). “Always allow” appends a saved rule and the approval event carries the action, exact resource, and the pattern that will be saved. Shell calls use a conservative prefix (`cat *`, `git status *`, `npm run test *`) only for simple known-safe commands; compound, globbed, unknown, or risky commands keep the exact command. Rejections with feedback become tool errors the model can read.

## Decisions log

1. **Core crate split.** `crates/mcp-chat-core` holds DB/LLM/MCP/agent so the engine can be tested with `cargo test` on a machine without WebKit. `src-tauri` only wires commands, paths, keychain and events.
2. **Thin MCP client instead of `rmcp` for this pass.** `rmcp` 3.7.0 is the official SDK and does support 2026-07-28 clients and Streamable HTTP, but this implementation needs exact mirrored header construction, `x-mcp-header`, legacy session/SSE fallback, and a UI-mediated MRTR handshake. The transport surface is isolated in `mcp/client.rs` and `mcp/legacy.rs`, so it can be replaced by `rmcp` later without touching the agent.
3. **CSP exception for Tauri IPC and local images.** Production CSP is `default-src 'self'; script-src 'self'; ...; img-src 'self' asset: http://asset.localhost https://asset.localhost data:; connect-src ipc: http://ipc.localhost`. The prompt's `connect-src 'none'` blocks Tauri invoke on the platforms we target. The asset protocol is scoped to `$APPDATA/attachments/**` so the webview can preview only files Jarvis has staged. Everything else stays locked down; no remote connect-src.
4. **Schema additions.** `max_steps`, per-conversation `tool_mode`/server selection, provider context/output limits and saved model lists, cached tool JSON/TTL, negotiated-at and OAuth issuer/client id were added to SQLite because context management and the server cards need them. The original parts/messages shape is unchanged.
5. **MCP tool delivery is per chat and lazy-only.** The MCP popover selects active servers; delivery is always lazy. A compact catalog of active tools (server slug, tool name, short description, argument summary) goes in the system prompt and exposes `mcp_get_tool_schema`, `mcp_call_tool` and `mcp_execute`; `mcp_search_tools` is only kept when the catalog is truncated. The lazy meta-calls resolve the real target, validate the real input schema, and apply the target server's policy and approval rules before calling it. `mcp_execute` batches up to 8 independent calls with 4 in flight, preserving result order. This avoids both the search→schema→call loop and a model round trip per tool in multi-tool turns. Migration `0005_lazy_only` normalizes any old direct-schema conversation back to lazy.
6. **Prompt context is layered and cache-aware.** The user-editable Jarvis prompt is followed by non-editable harness/tool rules, a workspace environment block, optional `AGENTS.md` instructions, a skill catalog, the lazy MCP catalog, and the compaction summary in that order. The date/time stays on the user message, not in this prefix. Runtime context is assembled once per turn, so writes made by tools cannot change the prompt mid-turn.
7. **Built-in tools are local, with a deliberately narrow read-only escape.** `bash`, `python`, `read`, `grep`, `glob`, `edit`, `apply_patch`, `write`, `skill` and `question` are always exposed alongside MCP tools. Writes resolve against the conversation's `workspace_root` (migration `0006_conversation_workspace`; fallback global setting/process cwd); lexical or symlink escapes are rejected unless the user explicitly approves that outside target. Read-only `read/grep/glob` may canonicalize explicit paths outside the workspace after per-call approval, or without approval when auto mode is on. Outside-workspace `write/edit/apply_patch` targets are allowed only after an explicit per-call approval, even in auto mode. `bash/edit/apply_patch/write` default to approval, while read-only tools are allowed. `apply_patch` uses CRLF-aware, whitespace-tolerant hunk matching with optional `@@` context anchors and `*** End of File` anchoring for multi-file add/update/move/delete; all hunks and paths are validated before writes, and file deletion still requires approval under auto-approve. File mutations are serialized per resolved path and the approval prompt includes a bounded unified diff for inside-workspace `edit`, `write` and `apply_patch` targets. `bash` defaults to PowerShell on Windows (with `cmd` opt-in), supports explicit shells, and `python` runs code directly through stdin with UTF-8 environment and timeout, a per-call `cwd`, labels stdout/stderr, detects command echo-back as a failed call, and encodes PowerShell commands with `-EncodedCommand` to avoid `cmd.exe` nested-quote breakage. Filesystem paths are normalized across tools, including Windows `\\?\` prefixes, case differences and trailing separators. Each invocation is a fresh process; `cd` does not persist. Shell output includes the exit code; a non-zero exit is a command result, while launch failure, timeout, termination and command echo are tool errors. The PowerShell wrapper propagates the last native `$LASTEXITCODE` and treats cmdlet errors as failures via `$ErrorActionPreference = 'Stop'`. Skills load `SKILL.md` or a file/subfolder inside the selected skill from workspace `.mcp-chat/skills`, `.opencode/skills`, app-data `skills`, `~/.config/opencode/skills`, or `~/.config/mcp-chat/skills`; traversal outside the skill directory is rejected. `question` uses the same InputRegistry as MCP MRTR and renders a native question dialog. Its `options` are optional (1 to 12 labelled items when present); a question without them is answered in free text. Its risk class stays `interactive`, so it never asks for approval.
8. **Time context is cache-friendly.** The local date/time note is injected at the start of each user message, not into the system prompt. The system prompt and tool catalog stay byte-identical. The note is rendered once at send time (minute precision) and stored as a hidden `time` part, so historical user messages produce exactly the same bytes on every later request and prefix caches can hit. Seconds are omitted deliberately. OpenCode currently puts day-precision date in the cached system block, which only misses at local midnight; ours avoids even that by moving all time into the user turn.
9. **Auto-approve is per-chat and session-scoped.** The composer toggle is stored in memory in `ChatEngine`, not SQLite. It converts only `Ask` decisions to `Allow`; explicit `Deny` policies remain. Dangerous shell commands and apply_patch file deletions still ask even in auto mode. Built-in writes are workspace-confined by default; outside-workspace writes always require explicit per-call approval and are never auto-approved. Read-only tools may leave the workspace after per-call approval, or without approval in auto mode. MCP still evaluates server policy first, with the existing auto relaxation of an MCP `Ask` to `Allow`; `Deny` remains. The setting resets when the app closes.
10. **Tool loops are bounded by repetition, not by a default step count.** The runner hashes `(tool, args, result)` per turn. Three identical results inject a strategy-change prompt and force one tool-free step. `max_steps` defaults to 0 (unlimited); if explicitly set, the old Continue flow is still available. The step number is streamed to the UI. `Steer` cancels the active turn, waits for it to settle, then creates a fresh user message and starts a new turn. Tool failures are not retried automatically; provider/network errors still use their own retry/backoff path.
11. **Usage is streamed.** Each step emits an estimated prompt-token event before the request; provider usage overrides it when available, and completion tokens are estimated when the provider does not report usage. The composer shows it live.
12. **Compaction is guided and cache-aware.** Older turns are summarized into a structured handoff (objective, constraints, decisions, completed/active work, blockers, next move, files/commands). The compactor reuses the normal system prompt, tool definitions, and older-message prefix, then appends the compaction instruction, so providers with prefix caching can avoid re-billing the shared prefix. The newest ~15k tokens are kept, and the trigger uses a 15%-of-window buffer with a 16k floor. The buffer and retention are configurable in Settings; the manual compact command exists in the backend but is not exposed in the UI. A `compacting` status is streamed while the summary is written, and an event-channel lag emits `resync` so the renderer reloads the active transcript instead of remaining stuck on stale streaming state.
13. **Tool results have two representations.** A `tool_call` part is the UI card and terminal-state record; a `tool` message is the provider protocol block. The UI deliberately hides raw tool messages.

14. **Token estimates use `chars / 3`.** No tokenizer dependency; conservative enough to trigger compaction early. Exact provider usage is not required for correctness.
15. **Workspace hygiene and paging.** `.jarvisignore` filters `glob`/`grep`; default grep restricts itself to text-like extensions; `.mcp-chat/artifacts` and `.mcp-chat/tmp` are excluded. When the workspace is a git repo, `runtime_context` adds `**/.mcp-chat/artifacts/` and `**/.mcp-chat/tmp/` to the repo-local `.git/info/exclude` (including linked worktrees via `commondir`), so runtime output cannot be committed without modifying a tracked `.gitignore`; `.mcp-chat/skills` and `.mcp-chat/AGENTS.md` stay committable. `bash` supports `maxLines`/`tail`, and `read_tool_output` supports line-based paging in addition to character offsets. The transcript itself is paged independently of model context. `get_messages_page` returns an 80-message page with a stable `(created_at, id)` cursor. The renderer keeps only the latest page initially and lazy-loads older pages on scroll-up; the complete history stays in SQLite and is still used for compaction and agent requests.
16. **Attachments are copied into Rust-owned storage.** The native picker or a Tauri drag-drop event supplies paths to a Tauri command; the core copies files into app data, classifies image/text/binary content from magic bytes and extension, and stores only metadata plus extracted text in message parts. Image bytes are read at provider-serialization time and sent as native image blocks; tool-returned images are copied to persistent `attachments/read-images`, then emitted as a synthetic user image block after the tool result so OpenAI-compatible chat providers receive them correctly. Text/code files and text-based PDFs are inlined with a 100k-character cap; scanned PDFs and other binaries are metadata-only. Pending copies are swept at startup, and conversation/revert cleanup removes or restores them. This keeps the webview out of arbitrary filesystem access and keeps prompts rebuildable.

17. **Window state is persisted.** `tauri-plugin-window-state` restores size, position and maximized state on launch. The plugin is configured in the Rust shell and does not add webview permissions.

18. **Frontend verification without Tauri.** The renderer is exercised in Chromium through Playwright with a stubbed IPC bridge. The native shell is intentionally thin so the untested surface is limited to command wiring and window setup (see gaps).

## Test map

| Area | Test |
|---|---|
| SSE framing | split chunks, CRLF, comments, multiline data, UTF-8 split, EOF flush |
| MCP modern | discover, tools/list, tools/call, headers, `_meta`, progress, MRTR, x-mcp-header, 50 KB payload |
| MCP legacy | fallback initialize, session header, tools |
| MCP HTTP+SSE | endpoint event, GET response correlation, tools call |
| MCP stdio | newline JSON-RPC handshake, tools/list, tools/call, progress, missing command, args/env validation |
| Namespacing | sanitize, collisions, remote-name mapping |
| Permissions | last-match-wins, glob, default policy, resource normalization |
| Agent | text, tool loop, parallel, invalid args, unknown tool, policy deny, approval/rejection/always, auto-approve, per-chat server gating, lazy catalog/schema/call, batch execute, read_tool_output paging, built-in file/shell/search/skill/question tools, skill references and outside-workspace read-only access, shell labels/echo handling, loop guard, steering, optional max steps, continue-turn resume, layered prompt/AGENTS/skill catalog, default time/Jarvis prompt, cache-stable system prompt, length, cut, overflow, manual compaction/context status, guided/cache-aware compaction, unknown finish, retries, spill/paging |
| DB | migrations from empty, repo CRUD, FTS sync/cascade, crash sweep |
| Updates | numeric version comparison, malformed tags, store update state and release actions |
| UI | transcript reducer, compact tool/thought rows, diff view and stats parsing, date grouping, rich Markdown copy, confirm-before-revert, auto-approve warning, model/workspace pickers, onboarding checklist, actionable toasts, accessible modals, Playwright smoke and activity specs |

## Tool routing, guards and the capability table

`shared/capabilities.json` is the single description of the built-in tools: label, risk class, description, the path argument it touches, its danger rule, its guard and whether it produces a diff. Text uses role placeholders (`{modify_file}`, `{create_file}`, `{replace_once}`, `{shell}`), so "existing files are changed with `apply_patch`" is written once. `agent/capabilities.rs` loads and strictly validates it (unknown fields, unknown roles or placeholders, duplicate tools and a diff tool that is not a file writer are all errors) and renders:

- the tool descriptions and the guard override flags (`overwrite`, `allowFileWrites`) in `builtin_tool_defs`
- the routing table and self-check in the harness rules
- the refusal messages of the `write` and `bash` guards
- default approval effects, dangerous-call rules, outside read/write targets, which tools return a diff, and whether using a guard override still needs review in auto mode

Routing covers asking too: the `ask_user` role (`question`) has its own "Klarifikasi blocking" route, and the file and shell routes never list it. The harness rules say when to ask: only for blocking input whose answer changes the action, file, format or target (or confirms a destructive step) and that the workspace cannot answer; status updates, pleasantries and light questions go in the final answer.

The renderer imports the same file for labels and the diff list. Tests pin the rendered tool definitions and the harness text, so the cached system prefix only changes on purpose, and fail when the table and the handlers drift apart.

`agent/tools/` is split by concern: `mod.rs` (executor, permission flow, `finish`), `shell`, `files`, `workspace`, `skills`, `patch`, `mcp`, `guards`, `defs`, `ui` (UI-only metadata such as the approval diff), `output` (spilled results) and `question`. Public paths under `agent::tools` are unchanged.

## Manual compaction

`compact_conversation` takes an optional focus text (capped at 2,000 characters) that is added to the summarizer prompt. It claims the conversation's turn slot for its whole duration (`ChatEngine::claim_turn`, released on success, error and cancel), so a message, regenerate or second compaction is refused meanwhile and Stop cancels it. It reports `Compacting` then `Idle`. The UI always confirms first, from the `/compact` command, the palette and the shortcut alike.

## Keyboard and the command registry

`src/features/commands/registry.ts` lists every command once, with its shortcut, a reason it is unavailable, and whether it may fire from a text field. The palette, the global shortcut handler and the settings shortcut list are all built from it. Rules that matter: approval chords never fire from a text field; on macOS `Mod` is Cmd only, because Ctrl+K/N/B are cursor movement there; nothing fires while an overlay is open, during key repeat or IME composition.

## Appearance and notifications

Density, text size and width are `ui.*` settings applied as `data-density`, `--chat-fs` and `--chat-width` on the root (`lib/appearance.ts`); every chat text size is a fraction of `--chat-fs` over 30 so the defaults reproduce the original sizes exactly. The global `button, input, textarea, select { font: inherit }` rule is unlayered and beats Tailwind size utilities, so the few chat controls that need to follow the setting set their size inline.

Notifications are decided in `lib/notifications.ts` (pure: given an event and what the user is looking at) and sent by `lib/notify.ts` through the Tauri notification plugin, or the Web Notification API in the browser mock. A turn-ending notice is sent for a background window or a chat that is not open, never for a stop the user asked for, and carries only the chat title. Permission is requested when the user sends a message or presses the test button, never from a background event. Only `is-permission-granted`, `request-permission` and `notify` are granted in the capability file.

## Known gaps

- **Desktop notifications are verified only up to the shell.** The decision logic, the sender and the settings are tested, the plugin and capability resolve with the locked manifest, but the shell was not compiled and no notification was shown by a real operating system here. On Windows notifications work only for an installed app. There is no tray icon.
- **Tauri shell is not compiled/launched in this environment.** The host lacks `libwebkit2gtk-4.1-dev`, GTK development headers, a display and sudo. The core and renderer are verified; the shell is intended for a desktop with the README prerequisites.
- **M6 OAuth is not implemented.** Static bearer tokens and per-server `X-API-Key` headers (value in the OS keychain) work; the OAuth option is disabled. `oauth_issuer`/`oauth_client_id` columns are reserved, and `McpManager` already looks up tokens by issuer, but there is no DCR/PKCE/loopback/refresh flow yet.
- **Native installers are built on the target OS.** `tauri.conf.json`, platform icons, NSIS artwork and packaging scripts are in the repository, but no Windows/macOS/Linux bundle was produced on the headless host. Follow `PACKAGING.md` on a desktop or CI runner. Update checks are check-only against the public GitHub release API; automatic download/install (Tauri updater signatures, AppImage) and MSIX are not implemented yet.
- **Legacy Streamable HTTP does not open the optional GET notification stream.** Responses to POST requests are handled; server-initiated messages on a legacy GET stream are not consumed.
- **Stdio servers cannot open client-side elicitation/sampling/roots handlers.** Notifications and progress are handled, but server-initiated requests are declined with JSON-RPC `-32601`; the stdio `initialize` therefore advertises no client capabilities.
- **`subscriptions/listen` is not implemented.** The `MCP_SUBSCRIPTIONS=1` path and `toolsListChanged` invalidation are deferred.
- **Background compaction is not implemented.** Compaction runs before a step or on context overflow; it is not precomputed at ~85% like OpenCode V2’s experimental background mode.
- **Real keychain, real API key and native window were not exercised.** Keyring is behind the `SecretStore` trait and is replaced with an in-memory store in tests. The UI screenshots use the mock IPC bridge.
- **`pnpm tauri build`/AppImage was not run.** Frontend production build is verified; native bundling is not.
