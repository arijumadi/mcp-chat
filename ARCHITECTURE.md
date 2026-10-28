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

A tool call is one part on the assistant message; its terminal state and result preview live in that payload. A separate `tool` message holds the result block sent back to the provider. The UI hides `tool` messages and renders the call card instead. This keeps the provider transcript valid while giving the renderer a compact card. Assistant text emitted before the last tool call is retained for provider context but omitted from the activity panel; only thoughts, tool cards and the post-tool answer are shown.

Synthetic user messages (continue prompts, max-step notices) are ordinary rows with `status = 'synthetic'`; they are part of the provider context but hidden from the transcript and skipped by Regenerate. Reverting a user message deletes that row and every later row with `delete_from`, removes matching spilled tool files, returns the original text to the composer, and resets the title to “New chat” when nothing remains.

## MCP negotiation

| Probe / situation | Result |
|---|---|
| `server/discover` succeeds and supports `2026-07-28` (or no version list) | Modern stateless client. Every POST carries `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name` when applicable, and per-request `_meta`. No `initialize`, no session id, no GET stream. |
| `server/discover` fails with 400/404/405 without a recognized modern error, `-32601`, or `-32022` where only `2025-11-25` is offered | Legacy client: `initialize` → `notifications/initialized`, keep `Mcp-Session-Id`, protocol header on later requests. |
| Legacy `initialize` fails with 404/405/transport error, or the URL ends in `/sse` | Deprecated HTTP+SSE: GET the URL, wait for the `endpoint` event, POST to it, correlate responses from the long-lived GET stream. |
| Any modern error such as `-32020` header mismatch or `-32021` missing capability | No fallback; humanized error surface. |
| Cached `2025-11-25` protocol in SQLite | Try legacy directly; on failure clear the assumption and re-probe. |

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

Per-server policy defaults to `ask`; rules are evaluated last-match-wins with `*` globs on action (`mcp.<slug>.<tool>`) and a normalized resource (URL host/path/channel or `*`). “Always allow” appends a saved rule. Rejections with feedback become tool errors the model can read.

## Decisions log

1. **Core crate split.** `crates/mcp-chat-core` holds DB/LLM/MCP/agent so the engine can be tested with `cargo test` on a machine without WebKit. `src-tauri` only wires commands, paths, keychain and events.
2. **Thin MCP client instead of `rmcp` for this pass.** `rmcp` 3.7.0 is the official SDK and does support 2026-07-28 clients and Streamable HTTP, but this implementation needs exact mirrored header construction, `x-mcp-header`, legacy session/SSE fallback, and a UI-mediated MRTR handshake. The transport surface is isolated in `mcp/client.rs` and `mcp/legacy.rs`, so it can be replaced by `rmcp` later without touching the agent.
3. **CSP exception for Tauri IPC and local images.** Production CSP is `default-src 'self'; script-src 'self'; ...; img-src 'self' asset: http://asset.localhost https://asset.localhost data:; connect-src ipc: http://ipc.localhost`. The prompt's `connect-src 'none'` blocks Tauri invoke on the platforms we target. The asset protocol is scoped to `$APPDATA/attachments/**` so the webview can preview only files Jarvis has staged. Everything else stays locked down; no remote connect-src.
4. **Schema additions.** `max_steps`, per-conversation `tool_mode`/server selection, provider context/output limits and saved model lists, cached tool JSON/TTL, negotiated-at and OAuth issuer/client id were added to SQLite because context management and the server cards need them. The original parts/messages shape is unchanged.
5. **MCP tool delivery is per chat and lazy-only.** The MCP popover selects active servers; delivery is always lazy. A compact catalog of active tools (server slug, tool name, short description, argument summary) goes in the system prompt and exposes `mcp_get_tool_schema`, `mcp_call_tool` and `mcp_execute`; `mcp_search_tools` is only kept when the catalog is truncated. The lazy meta-calls resolve the real target, validate the real input schema, and apply the target server's policy and approval rules before calling it. `mcp_execute` batches up to 8 independent calls with 4 in flight, preserving result order. This avoids both the search→schema→call loop and a model round trip per tool in multi-tool turns. Migration `0005_lazy_only` normalizes any old direct-schema conversation back to lazy.
6. **Prompt context is layered and cache-aware.** The user-editable Jarvis prompt is followed by non-editable harness/tool rules, a workspace environment block, optional `AGENTS.md` instructions, a skill catalog, the lazy MCP catalog, and the compaction summary in that order. The date/time stays on the user message, not in this prefix. Runtime context is assembled once per turn, so writes made by tools cannot change the prompt mid-turn.
7. **Built-in tools are local, with a deliberately narrow read-only escape.** `bash`, `read`, `grep`, `glob`, `edit`, `apply_patch`, `write`, `skill` and `question` are always exposed alongside MCP tools. Writes resolve against the conversation's `workspace_root` (migration `0006_conversation_workspace`; fallback global setting/process cwd); lexical or symlink escapes are rejected unless the user explicitly approves that outside target. Read-only `read/grep/glob` may canonicalize explicit paths outside the workspace only when auto mode is on. Outside-workspace `write/edit/apply_patch` targets are allowed only after an explicit per-call approval, even in auto mode. `bash/edit/apply_patch/write` default to approval, while read-only tools are allowed. `apply_patch` uses exact-context hunks for multi-file add/update/move/delete; all hunks and paths are validated before writes, and file deletion still requires approval under auto-approve. `bash` supports explicit shells, a per-call `cwd`, labels stdout/stderr, detects command echo-back as a failed call, and encodes PowerShell commands with `-EncodedCommand` to avoid `cmd.exe` nested-quote breakage. Each invocation is a fresh process; `cd` does not persist. Skills load `SKILL.md` or a file/subfolder inside the selected skill from workspace `.mcp-chat/skills`, `.opencode/skills`, app-data `skills`, `~/.config/opencode/skills`, or `~/.config/mcp-chat/skills`; traversal outside the skill directory is rejected. `question` uses the same InputRegistry as MCP MRTR and renders a native question picker.
8. **Time context is cache-friendly.** The local date/time note is injected at the start of each user message, not into the system prompt. The system prompt and tool catalog stay byte-identical. The note is rendered once at send time (minute precision) and stored as a hidden `time` part, so historical user messages produce exactly the same bytes on every later request and prefix caches can hit. Seconds are omitted deliberately. OpenCode currently puts day-precision date in the cached system block, which only misses at local midnight; ours avoids even that by moving all time into the user turn.
9. **Auto-approve is per-chat and session-scoped.** The composer toggle is stored in memory in `ChatEngine`, not SQLite. It converts only `Ask` decisions to `Allow`; explicit `Deny` policies remain. Dangerous shell commands and apply_patch file deletions still ask even in auto mode. Built-in writes are workspace-confined by default; outside-workspace writes always require explicit per-call approval and are never auto-approved. Read-only tools may leave the workspace in auto mode. MCP still evaluates server policy first, with the existing auto relaxation of an MCP `Ask` to `Allow`; `Deny` remains. The setting resets when the app closes.
10. **Tool loops are bounded by repetition, not by a default step count.** The runner hashes `(tool, args, result)` per turn. Three identical results inject a strategy-change prompt and force one tool-free step. `max_steps` defaults to 0 (unlimited); if explicitly set, the old Continue flow is still available. The step number is streamed to the UI. `Steer` cancels the active turn, waits for it to settle, then creates a fresh user message and starts a new turn. Tool failures are not retried automatically; provider/network errors still use their own retry/backoff path.
11. **Usage is streamed.** Each step emits an estimated prompt-token event before the request; provider usage overrides it when available, and completion tokens are estimated when the provider does not report usage. The composer shows it live.
12. **Compaction follows the V2 shape.** Older turns are summarized, the newest ~15k tokens are kept, and the trigger uses a 15%-of-window buffer with a 16k floor. The buffer and retention are configurable in Settings; the manual compact command exists in the backend but is not exposed in the UI.
13. **Tool results have two representations.** A `tool_call` part is the UI card and terminal-state record; a `tool` message is the provider protocol block. The UI deliberately hides raw tool messages.

14. **Token estimates use `chars / 3`.** No tokenizer dependency; conservative enough to trigger compaction early. Exact provider usage is not required for correctness.
15. **Attachments are copied into Rust-owned storage.** The native picker or a Tauri drag-drop event supplies paths to a Tauri command; the core copies files into app data, classifies image/text/binary content from magic bytes and extension, and stores only metadata plus extracted text in message parts. Image bytes are read at provider-serialization time and sent as native image blocks; text/code files and text-based PDFs are inlined with a 100k-character cap; scanned PDFs and other binaries are metadata-only. Pending copies are swept at startup, and conversation/revert cleanup removes or restores them. This keeps the webview out of arbitrary filesystem access and keeps prompts rebuildable.

16. **Frontend verification without Tauri.** The renderer is exercised in Chromium through Playwright with a stubbed IPC bridge. The native shell is intentionally thin so the untested surface is limited to command wiring and window setup (see gaps).

## Test map

| Area | Test |
|---|---|
| SSE framing | split chunks, CRLF, comments, multiline data, UTF-8 split, EOF flush |
| MCP modern | discover, tools/list, tools/call, headers, `_meta`, progress, MRTR, x-mcp-header, 50 KB payload |
| MCP legacy | fallback initialize, session header, tools |
| MCP HTTP+SSE | endpoint event, GET response correlation, tools call |
| Namespacing | sanitize, collisions, remote-name mapping |
| Permissions | last-match-wins, glob, default policy, resource normalization |
| Agent | text, tool loop, parallel, invalid args, unknown tool, policy deny, approval/rejection/always, auto-approve, per-chat server gating, lazy catalog/schema/call, batch execute, read_tool_output paging, built-in file/shell/search/skill/question tools, skill references and outside-workspace read-only access, shell labels/echo handling, loop guard, steering, optional max steps, continue-turn resume, layered prompt/AGENTS/skill catalog, default time/Jarvis prompt, cache-stable system prompt, length, cut, overflow, manual compaction/context status, unknown finish, retries, spill/paging |
| DB | migrations from empty, repo CRUD, FTS sync/cascade, crash sweep |
| UI | transcript reducer, tool card rendering, date grouping, rich Markdown copy, confirm-before-revert, auto-approve warning, model/workspace pickers, onboarding checklist, actionable toasts, accessible modals, Playwright smoke (5) |

## Known gaps

- **Tauri shell is not compiled/launched in this environment.** The host lacks `libwebkit2gtk-4.1-dev`, GTK development headers, a display and sudo. The core and renderer are verified; the shell is intended for a desktop with the README prerequisites.
- **M6 OAuth is not implemented.** Static bearer tokens and per-server `X-API-Key` headers (value in the OS keychain) work; the OAuth option is disabled. `oauth_issuer`/`oauth_client_id` columns are reserved, and `McpManager` already looks up tokens by issuer, but there is no DCR/PKCE/loopback/refresh flow yet.
- **Native installers are built on the target OS.** `tauri.conf.json`, platform icons, NSIS artwork and packaging scripts are in the repository, but no Windows/macOS/Linux bundle was produced on the headless host. Follow `PACKAGING.md` on a desktop or CI runner. Automatic updates and MSIX are not implemented.
- **Legacy Streamable HTTP does not open the optional GET notification stream.** Responses to POST requests are handled; server-initiated messages on a legacy GET stream are not consumed.
- **`subscriptions/listen` is not implemented.** The `MCP_SUBSCRIPTIONS=1` path and `toolsListChanged` invalidation are deferred.
- **Background compaction is not implemented.** Compaction runs before a step or on context overflow; it is not precomputed at ~85% like OpenCode V2’s experimental background mode.
- **Real keychain, real API key and native window were not exercised.** Keyring is behind the `SecretStore` trait and is replaced with an in-memory store in tests. The UI screenshots use the mock IPC bridge.
- **`pnpm tauri build`/AppImage was not run.** Frontend production build is verified; native bundling is not.
