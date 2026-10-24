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
│  commands: chat, providers, servers, settings                 │
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
        LLM endpoint / MCP server          tool-output/*.json
```

There is no API key, MCP call, or provider request in the webview. The frontend only invokes commands and listens to three event channels.

## Persistence

SQLite in WAL mode, foreign keys on, migrations via `sqlx::migrate!`.

- `conversations` — title, timestamps, provider/model, system prompt, archived flag, `max_steps` (default 25).
- `messages` — role, status (`completed`, `streaming`, `interrupted`, `error`, `synthetic`), error.
- `parts` — ordered typed JSON payloads: `text`, `reasoning`, `tool_call`, `tool_result`, `error`, `summary`, and a hidden `time` marker used for the user-message timestamp.
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
- Tool result precedence: `structuredContent` → concatenated `content[]` text → raw JSON. The model sees at most 8k characters; the full payload is spilled and paged via `read_tool_output`.
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
  └─ max steps → synthetic notice, tool_choice none, errors for any calls
```

Settlement invariant: every tool call gets exactly one terminal result, including unknown tools, schema failures, JSON-RPC errors, user rejection, cancellation, missing provider results and crash recovery. Startup `sweep_interrupted` marks dangling parts `aborted`; the context builder also synthesizes a tool result for any historic call that lacks one, so a crash never produces a transcript the provider rejects.

Per-server policy defaults to `ask`; rules are evaluated last-match-wins with `*` globs on action (`mcp.<slug>.<tool>`) and a normalized resource (URL host/path/channel or `*`). “Always allow” appends a saved rule. Rejections with feedback become tool errors the model can read.

## Decisions log

1. **Core crate split.** `crates/mcp-chat-core` holds DB/LLM/MCP/agent so the engine can be tested with `cargo test` on a machine without WebKit. `src-tauri` only wires commands, paths, keychain and events.
2. **Thin MCP client instead of `rmcp` for this pass.** `rmcp` 3.7.0 is the official SDK and does support 2026-07-28 clients and Streamable HTTP, but this implementation needs exact mirrored header construction, `x-mcp-header`, legacy session/SSE fallback, and a UI-mediated MRTR handshake. The transport surface is isolated in `mcp/client.rs` and `mcp/legacy.rs`, so it can be replaced by `rmcp` later without touching the agent.
3. **CSP exception for Tauri IPC.** Production CSP is `default-src 'self'; script-src 'self'; ...; connect-src ipc: http://ipc.localhost`. The prompt's `connect-src 'none'` blocks Tauri invoke on the platforms we target. Everything else stays locked down; no remote connect-src.
4. **Schema additions.** `max_steps`, per-conversation `tool_mode`/server selection, provider context/output limits and saved model lists, cached tool JSON/TTL, negotiated-at and OAuth issuer/client id were added to SQLite because context management and the server cards need them. The original parts/messages shape is unchanged.
5. **MCP tool delivery is per chat and lazy-only.** The Tools popover selects active servers; delivery is always lazy. A compact catalog of active tools (server slug, tool name, short description, argument summary) goes in the system prompt and exposes `mcp_get_tool_schema`, `mcp_call_tool` and `mcp_execute`; `mcp_search_tools` is only kept when the catalog is truncated. The lazy meta-calls resolve the real target, validate the real input schema, and apply the target server's policy and approval rules before calling it. `mcp_execute` batches up to 8 independent calls with 4 in flight, preserving result order. This avoids both the search→schema→call loop and a model round trip per tool in multi-tool turns. Migration `0005_lazy_only` normalizes any old direct-schema conversation back to lazy.
6. **Prompt context is layered and cache-aware.** The user-editable Jarvis prompt is followed by non-editable harness/tool rules, a workspace environment block, optional `AGENTS.md` instructions, a skill catalog, the lazy MCP catalog, and the compaction summary in that order. The date/time stays on the user message, not in this prefix. Runtime context is assembled once per turn, so writes made by tools cannot change the prompt mid-turn.
7. **Built-in tools are local and workspace-scoped.** `bash`, `edit`, `write`, `read`, `grep`, `glob`, `skill` and `question` are always exposed alongside MCP tools. File tools resolve paths against `workspace.root` (Settings, default process cwd) and reject lexical or symlink escapes; `bash/edit/write` default to approval, while read-only tools are allowed. `bash` supports explicit shells, a per-call `cwd`, labels stdout/stderr, detects command echo-back as a failed call, and encodes PowerShell commands with `-EncodedCommand` to avoid `cmd.exe` nested-quote breakage. Each invocation is a fresh process; `cd` does not persist. Skills load `SKILL.md` from workspace `.mcp-chat/skills`, `.opencode/skills`, or app-data `skills`. `question` uses the same InputRegistry as MCP MRTR and renders a native question picker.
8. **Time context is cache-friendly.** The local date/time note is injected at the start of each user message, not into the system prompt. The system prompt and tool catalog stay byte-identical. The note is rendered once at send time (minute precision) and stored as a hidden `time` part, so historical user messages produce exactly the same bytes on every later request and prefix caches can hit. Seconds are omitted deliberately. OpenCode currently puts day-precision date in the cached system block, which only misses at local midnight; ours avoids even that by moving all time into the user turn.
9. **Tool loops are bounded.** The runner hashes `(tool, args, result)` per turn. Three identical results inject a strategy-change prompt and force one tool-free step. The step budget is streamed to the UI; after the default 25-step limit the composer offers **Continue**, which appends a hidden synthetic resume prompt and starts a fresh budget. Tool failures are not retried automatically; provider/network errors still use their own retry/backoff path.
10. **Usage is streamed.** Each step emits an estimated prompt-token event before the request; provider usage overrides it when available, and completion tokens are estimated when the provider does not report usage. The composer shows it live.
11. **Compaction follows the V2 shape.** Older turns are summarized, the newest ~15k tokens are kept, and the trigger uses a 15%-of-window buffer with a 16k floor. The buffer and retention are configurable in Settings; the manual compact command exists in the backend but is not exposed in the UI.
12. **Tool results have two representations.** A `tool_call` part is the UI card and terminal-state record; a `tool` message is the provider protocol block. The UI deliberately hides raw tool messages.

13. **Token estimates use `chars / 3`.** No tokenizer dependency; conservative enough to trigger compaction early. Exact provider usage is not required for correctness.
14. **Frontend verification without Tauri.** The renderer is exercised in Chromium through Playwright with a stubbed IPC bridge. The native shell is intentionally thin so the untested surface is limited to command wiring and window setup (see gaps).

## Test map

| Area | Test |
|---|---|
| SSE framing | split chunks, CRLF, comments, multiline data, UTF-8 split, EOF flush |
| MCP modern | discover, tools/list, tools/call, headers, `_meta`, progress, MRTR, x-mcp-header, 50 KB payload |
| MCP legacy | fallback initialize, session header, tools |
| MCP HTTP+SSE | endpoint event, GET response correlation, tools call |
| Namespacing | sanitize, collisions, remote-name mapping |
| Permissions | last-match-wins, glob, default policy, resource normalization |
| Agent | text, tool loop, parallel, invalid args, unknown tool, policy deny, approval/rejection/always, per-chat server gating, lazy catalog/schema/call, batch execute, read_tool_output paging, built-in file/shell/search/skill/question tools, shell labels/echo handling, loop guard, continue-turn resume, layered prompt/AGENTS/skill catalog, default time/Jarvis prompt, cache-stable system prompt, length, cut, overflow, manual compaction/context status, unknown finish, max steps, retries, spill/paging |
| DB | migrations from empty, repo CRUD, FTS sync/cascade, crash sweep |
| UI | transcript reducer, tool card rendering, date grouping, rich Markdown copy, revert hover actions, Playwright smoke (5) |

## Known gaps

- **Tauri shell is not compiled/launched in this environment.** The host lacks `libwebkit2gtk-4.1-dev`, GTK development headers, a display and sudo. The core and renderer are verified; the shell is intended for a desktop with the README prerequisites.
- **M6 OAuth is not implemented.** Static bearer tokens and per-server `X-API-Key` headers (value in the OS keychain) work; the OAuth option is disabled. `oauth_issuer`/`oauth_client_id` columns are reserved, and `McpManager` already looks up tokens by issuer, but there is no DCR/PKCE/loopback/refresh flow yet.
- **M7 packaging is not implemented.** No AppImage/deb/dmg build was produced; `tauri.conf.json` is configured but packaging needs a desktop toolchain.
- **Legacy Streamable HTTP does not open the optional GET notification stream.** Responses to POST requests are handled; server-initiated messages on a legacy GET stream are not consumed.
- **`subscriptions/listen` is not implemented.** The `MCP_SUBSCRIPTIONS=1` path and `toolsListChanged` invalidation are deferred.
- **Background compaction is not implemented.** Compaction runs before a step or on context overflow; it is not precomputed at ~85% like OpenCode V2’s experimental background mode.
- **Real keychain, real API key and native window were not exercised.** Keyring is behind the `SecretStore` trait and is replaced with an in-memory store in tests. The UI screenshots use the mock IPC bridge.
- **`pnpm tauri build`/AppImage was not run.** Frontend production build is verified; native bundling is not.
