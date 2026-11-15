---
name: antigravity-cli
description: "Operate the Antigravity CLI (agy): install, auth, plugins, sandbox, one-shot runs."
version: 0.2.0
author: Tony Simons (asimons81), Hermes Agent (adapted for Jarvis)
license: MIT
platforms: [linux, macos, windows]
tags: [coding-agent, antigravity, cli, auth, plugins, sandbox]
---

# Antigravity CLI (`agy`)

Operator guide for the Antigravity CLI, invoked as `agy`. Run commands through
Jarvis's `bash` tool; inspect config and logs with `read`. This skill is
reference plus procedure — it does not wrap a network API, so there is nothing
to authenticate from Jarvis itself.

## When to use

- Install, update, or smoke-test the `agy` binary.
- Drive non-interactive `agy --print` / `agy -p` one-shots.
- Debug Antigravity auth, sandbox, permissions, or plugin state.
- Read Antigravity settings, keybindings, conversations, or logs.

## Mental model

Antigravity has two layers — keep them distinct:

1. **Shell wrapper commands** — `agy help`, `agy install`, `agy plugin`,
   `agy update`, `agy changelog`. Run these through Jarvis's `bash` tool.
2. **Interactive in-session slash commands** — `/config`, `/permissions`,
   `/skills`, `/agents`, etc. These only exist inside a running `agy` TUI, not
   on the shell wrapper.

`agy help` shows the shell wrapper surface, NOT the in-session slash commands.

## Prerequisites

- The `agy` binary on PATH. Verify with `bash`:

```bash
command -v agy && agy --version
```

- No env vars or API keys are required by this skill. Antigravity manages its
  own auth through the OS keyring or browser sign-in.
- Jarvis has no PTY or background-terminal tool. Use `agy -p` only. For a long
  task, split the work or run the interactive TUI outside Jarvis.

## How to run

Use Jarvis's `bash` tool. Set `cwd` for the repository and `timeoutMs` for
bounded runs; the maximum is 600000 ms (10 minutes).

```text
bash(command="agy --version")
bash(command="agy help")
bash(command="agy plugin list")
bash(command="agy -p 'Summarize the repo in 3 bullets'", cwd="/path/to/project", timeoutMs=300000)
```

For longer print runs, raise Antigravity's own timeout and keep the outer
Jarvis timeout within 10 minutes:

```text
bash(command="agy -p 'Implement the change in TASK.md and run the tests' --print-timeout 8m", cwd="/path/to/repo", timeoutMs=540000)
```

Jarvis can run independent `bash` calls in parallel, up to four in flight. For
a worktree fan-out, launch one `agy -p` per worktree and collect the outputs;

keep concurrency within what the machine and your review capacity can handle.

To inspect Antigravity's files, use `read` on the paths below. Do not `cat`
them through `bash` unless you need raw bytes.

## One-shot delegation

`agy -p` is non-interactive: it runs the prompt and exits. Pick the engine with
`--model` (`agy models` prints the exact display strings, for example
`'Gemini 3.1 Pro (High)'`). Add extra context roots with repeatable
`--add-dir`.

```text
bash(command="agy -p 'Review this diff for bugs and security issues' --model 'Gemini 3.1 Pro (High)'", cwd="/path/to/repo", timeoutMs=300000)
```

`agy -p` returns **plain text** — there is no `--output-format json` and no
session/cost envelope. Parse stdout directly. There is also no `--max-turns`;
bound a run with `--print-timeout` (default `5m`) and Jarvis's `timeoutMs`.

### Orchestration boundary

Antigravity is a **worker backend or third-opinion reviewer**, not a first-class
orchestration primitive. Route work through the normal task graph and let the
assigned worker choose `agy` versus direct tools. Reach for it explicitly when
the user asks, when a worker is configured to wrap it, or when you want a
Gemini-family cross-check against another agent's plan or diff.

## Core paths

- Binary: `agy`
- App data: `~/.gemini/antigravity-cli/`
- Settings: `~/.gemini/antigravity-cli/settings.json`
- Keybindings: `~/.gemini/antigravity-cli/keybindings.json`
- Logs: `~/.gemini/antigravity-cli/log/cli-*.log`
- Conversations: `~/.gemini/antigravity-cli/conversations/`
- Brain artifacts: `~/.gemini/antigravity-cli/brain/`
- History: `~/.gemini/antigravity-cli/history.jsonl`
- Plugin staging: `~/.gemini/antigravity-cli/plugins/<plugin_name>/`

## Quick reference

### Wrapper commands

- `agy changelog`
- `agy help`
- `agy install`
- `agy plugin` / `agy plugins`
- `agy update`

### Useful flags

- `--add-dir`
- `--continue` / `-c`
- `--conversation`
- `--dangerously-skip-permissions`
- `--print` / `-p`
- `--print-timeout`
- `--prompt`
- `--prompt-interactive` / `-i`
- `--sandbox`
- `--log-file`
- `--version`

### Plugin subcommands

`list`, `import [source]`, `install <target>`, `uninstall <name>`, `enable
<name>`, `disable <name>`, `validate [path]`, `link <mp> <target>`, `help`.

### In-session slash commands

- **Conversation:** `/resume` (`/switch`), `/rewind` (`/undo`), `/rename
  <name>`, `/clear`, `/fork`, `/reset`, `/new`
- **Settings and tools:** `/config`, `/settings`, `/permissions`, `/model`,
  `/keybindings`, `/statusline`, `/tasks`, `/skills`, `/mcp`, `/open <path>`,
  `/usage`, `/logout`, `/agents`
- **Prompt helpers:** `@` path autocomplete, `esc esc` clears the prompt, `!`
  runs a command (inside Antigravity), `?` opens help

## Settings and permissions

Common `settings.json` keys:

- `allowNonWorkspaceAccess`
- `colorScheme`
- `permissions.allow`
- `trustedWorkspaces`

Permission modes: `request-review`, `always-proceed`, `strict`,
`proceed-in-sandbox`.

`enableTerminalSandbox` is a boolean and defaults to `false`. Launch overrides
`--sandbox` and `--dangerously-skip-permissions` apply only to that run.

## Authentication

- Tries the OS secure keyring first.
- With no session, falls back to browser-based Google sign-in.
- Locally it opens the default browser; over SSH it prints an authorization URL
  and expects the code pasted back.
- `/logout` removes saved credentials.

## Plugins

- Plugins stage under `~/.gemini/antigravity-cli/plugins/<plugin_name>/`.
- They can bundle skills, agents, rules, MCP servers, and hooks.
- `agy plugin list` returning no imported plugins is valid.

## Pitfalls

- `agy help` shows wrapper commands, not interactive slash commands.
- `agy --version` is the safe non-interactive version check; `agy version` is
  interactive and can fail without a real TTY.
- First place to look for failures: `~/.gemini/antigravity-cli/log/cli-*.log`
  with `read`.
- Do not confuse persistent JSON settings with launch-time overrides.
- `~/.gemini/antigravity-cli/bin/agentapi` is a thin wrapper to `agy agentapi`.
- On WSL, token storage is file-based; auth issues are usually local session
  files, not browser-only problems.
- Workspace identity can depend on the launch directory and the
  `.antigravitycli` project marker.
- `agy -p` prints plain text only; do not parse it as JSON.
- Bound print runs with `--print-timeout`, not `--max-turns` (which does not
  exist).
- Jarvis cannot attach a PTY. Bare `agy` or `agy -i` is not usable through the
  tool loop; use `-p` for all automated work.

## Verification

Use `bash` for commands and `read` for files:

1. `bash(command="command -v agy")`
2. `bash(command="agy --version")`
3. `bash(command="agy help")`
4. `bash(command="agy plugin list")`
5. `read` on `~/.gemini/antigravity-cli/settings.json`
6. `read` on the latest `~/.gemini/antigravity-cli/log/cli-*.log`
7. If needed, `read` on `~/.gemini/antigravity-cli/keybindings.json`

## Support files

- `references/cli-docs.md` — condensed notes from the getting-started, usage and
  features docs.
