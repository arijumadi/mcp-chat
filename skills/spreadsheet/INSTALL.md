# Jarvis spreadsheet skill

Jarvis installs this skill automatically into the global skills directory on
startup. The manual steps below remain for older builds or a custom install.
The installer does not overwrite a file you have edited.

Requires Python 3 and `openpyxl`:

```bash
python -m pip install openpyxl
```

Optional: LibreOffice (`soffice`) for headless recalculation and format
conversion.

Global install path:

```text
~/.config/mcp-chat/skills/spreadsheet
```

On Windows:

```text
%USERPROFILE%\.config\mcp-chat\skills\spreadsheet
```

Workspace-only:

```text
<workspace>/.mcp-chat/skills/spreadsheet
```

The skill includes:

```text
spreadsheet/SKILL.md
spreadsheet/scripts/probe.py
spreadsheet/scripts/diff.py
spreadsheet/scripts/xlsx_create.py
spreadsheet/scripts/xlsx_read.py
spreadsheet/scripts/xlsx_edit.py
spreadsheet/scripts/xlsx_restructure.py
spreadsheet/scripts/xlsx_recalc.py
spreadsheet/scripts/csv_to_xlsx.py
spreadsheet/scripts/xlsx_to_csv.py
spreadsheet/references/restructuring.md
```

Jarvis reads only the skill name and description in the prompt. The full
`SKILL.md` and helper scripts load on demand through the `skill` tool.
