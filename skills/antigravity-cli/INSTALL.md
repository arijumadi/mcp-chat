# Jarvis Antigravity CLI skill

Jarvis installs this skill automatically into the global skills directory on startup. The manual steps below are for older builds or a custom installation. The installer does not overwrite a file you have edited.

Install the Antigravity CLI so `agy` is on PATH, then verify it:

```bash
command -v agy
agy --version
```

Sign in the first time with `agy` or complete the browser flow Antigravity opens. This skill does not store Antigravity credentials; the CLI manages its own OS keyring session.

Global install path:

```text
~/.config/mcp-chat/skills/antigravity-cli
```

On Windows:

```text
%USERPROFILE%\.config\mcp-chat\skills\antigravity-cli
```

Workspace-only:

```text
<workspace>/.mcp-chat/skills/antigravity-cli
```

Jarvis reads only the skill name and description in the prompt. The full `SKILL.md` and reference docs load on demand through the `skill` tool.
