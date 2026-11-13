# Jarvis spreadsheet skill

Jarvis installs this skill automatically into the global skills directory on
startup. The manual steps below remain for older builds or a custom install.
The installer does not overwrite a file you have edited.

Requires Python 3 and `openpyxl` (`python -m pip install openpyxl`).

Extract the `spreadsheet` folder from this zip into one of these directories:

Global (available in every workspace):

```text
C:\Users\<your-user>\.config\mcp-chat\skills
```

Workspace-only:

```text
<your-workspace>\.mcp-chat\skills
```

PowerShell example for global installation:

```powershell
$skills = Join-Path $env:USERPROFILE ".config\mcp-chat\skills"
New-Item -ItemType Directory -Force $skills | Out-Null
Expand-Archive .\mcp-spreadsheet-skill.zip -DestinationPath $skills -Force
```

The result must be:

```text
...\skills\spreadsheet\SKILL.md
...\skills\spreadsheet\scripts\probe.py
...\skills\spreadsheet\scripts\diff.py
```

Jarvis reads only the skill name and description in the prompt. The full
`SKILL.md` and helper scripts load on demand through the `skill` tool.
