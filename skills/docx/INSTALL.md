# Jarvis docx skill

Jarvis installs this skill automatically into the global skills directory on startup. The manual steps below are for older builds or a custom installation. The installer does not overwrite a file you have edited.

Requires Python 3.10+ and `python-docx`:

```bash
python -m pip install python-docx
```

Optional: LibreOffice (`soffice`) for converting `.docx` to PDF.

Global install path:

```text
~/.config/mcp-chat/skills/docx
```

On Windows:

```text
%USERPROFILE%\.config\mcp-chat\skills\docx
```

Workspace-only:

```text
<workspace>/.mcp-chat/skills/docx
```

Jarvis reads only the skill name and description in the prompt. The full `SKILL.md` and scripts load on demand through the `skill` tool.
