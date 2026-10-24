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
- `parts` — ordered typed JSON payloads: `text`, `reasoning`, `tool_call`, `tool_result`, `error`, `summary`.
- `providers` — kind, base URL, model, `api_key_ref` account id, context window, max output.
- `servers` — URL, transport, auth kind, headers JSON, policy, negotiated version, capability/server info, cached tools + timestamp, OAuth issuer/client id.
- `settings`, `permission_rules`.
- `messages_fts` — FTS5 over text parts, maintained by insert/update/delete triggers.

A tool call is one part on the assistant message; its terminal state and result preview live in that payload. A separate `tool` message holds the result block sent back to the provider. The UI hides `tool` messages and renders the call card instead. This keeps the provider transcript valid while giving the renderer a compact card.

Synthetic user messages (continue prompts, max-step notices) are ordinary rows with `status = 'synthetic'`; they are part of the provider context but hidden from the transcript and skipped by Regenerate.

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
4. **Schema additions.** `max_steps`, provider context/output limits, cached tool JSON/TTL, negotiated-at and OAuth issuer/client id were added to SQLite because compaction and the server cards need them. The original parts/messages shape is unchanged.
5. **Tool results have two representations.** A `tool_call` part is the UI card and terminal-state record; a `tool` message is the provider protocol block. The UI deliberately hides raw tool messages.
6. **Compaction is synchronous before a step (and on overflow), not a background job after idle.** This is simpler and avoids a race between a background summarizer and the next turn. It costs the summarizer latency once per boundary. Marked as a gap below.
7. **Token estimates use `chars / 3`.** No tokenizer dependency; conservative enough to trigger compaction early. Exact provider usage is not required for correctness.
8. **Frontend verification without Tauri.** The renderer is exercised in Chromium through Playwright with a stubbed IPC bridge. The native shell is intentionally thin so the untested surface is limited to command wiring and window setup (see gaps).

## Test map

| Area | Test |
|---|---|
| SSE framing | split chunks, CRLF, comments, multiline data, UTF-8 split, EOF flush |
| MCP modern | discover, tools/list, tools/call, headers, `_meta`, progress, MRTR, x-mcp-header, 50 KB payload |
| MCP legacy | fallback initialize, session header, tools |
| MCP HTTP+SSE | endpoint event, GET response correlation, tools call |
| Namespacing | sanitize, collisions, remote-name mapping |
| Permissions | last-match-wins, glob, default policy, resource normalization |
| Agent | text, tool loop, parallel, invalid args, unknown tool, policy deny, approval/rejection/always, length, cut, overflow, unknown finish, max steps, retries, spill/paging |
| DB | migrations from empty, repo CRUD, FTS sync/cascade, crash sweep |
| UI | transcript reducer, tool card rendering, date grouping, Playwright smoke (4) |

## Known gaps

- **Tauri shell is not compiled/launched in this environment.** The host lacks `libwebkit2gtk-4.1-dev`, GTK development headers, a display and sudo. The core and renderer are verified; the shell is intended for a desktop with the README prerequisites.
- **M6 OAuth is not implemented.** Static bearer tokens and per-server `X-API-Key` headers (value in the OS keychain) work; the OAuth option is disabled. `oauth_issuer`/`oauth_client_id` columns are reserved, and `McpManager` already looks up tokens by issuer, but there is no DCR/PKCE/loopback/refresh flow yet.
- **M7 packaging is not implemented.** No AppImage/deb/dmg build was produced; `tauri.conf.json` is configured but packaging needs a desktop toolchain.
- **Legacy Streamable HTTP does not open the optional GET notification stream.** Responses to POST requests are handled; server-initiated messages on a legacy GET stream are not consumed.
- **`subscriptions/listen` is not implemented.** The `MCP_SUBSCRIPTIONS=1` path and `toolsListChanged` invalidation are deferred.
- **Compaction runs before a step, not after idle.** See decision 6.
- **Real keychain, real API key and native window were not exercised.** Keyring is behind the `SecretStore` trait and is replaced with an in-memory store in tests. The UI screenshots use the mock IPC bridge.
- **`pnpm tauri build`/AppImage was not run.** Frontend production build is verified; native bundling is not.
