---
name: spreadsheet
description: Probe, read, compare, and safely edit Excel .xlsx workbooks with openpyxl. Use when the user mentions Excel, .xlsx, sheets, workbook merges, cell comparisons, accounting workbooks, or verifying that an edit did not change existing data.
---

# Spreadsheet workflow

Use the `python` tool for spreadsheet work. Do not load a whole workbook with pandas or with `openpyxl.load_workbook()` in normal mode before checking its shape.

## 1. Probe first

Run `scripts/probe.py` before reading or editing a workbook. It uses `read_only=True` and bounds the row scan, so a sheet whose formatting inflated `max_row` to 1,048,576 does not take minutes to inspect.

The helper can be at either:

```text
<workspace>/.mcp-chat/skills/spreadsheet/scripts/probe.py
%USERPROFILE%/.config/mcp-chat/skills/spreadsheet/scripts/probe.py
```

Use this resolver in a `python` call:

```python
import os, subprocess, sys
from pathlib import Path

roots = [
    Path(".mcp-chat/skills/spreadsheet"),
    Path(os.environ.get("USERPROFILE", "")) / ".config/mcp-chat/skills/spreadsheet",
    Path.home() / ".config/mcp-chat/skills/spreadsheet",
]
skill = next((p for p in roots if (p / "scripts/probe.py").is_file()), None)
assert skill, "spreadsheet skill scripts not found"
subprocess.run([sys.executable, str(skill / "scripts/probe.py"), r"path\to\file.xlsx"], check=True)
```

Read the JSON output. Prefer sheets where `safe_to_read_full` is true. For `bloated: true`, read only the known range with `iter_rows(min_row=..., max_row=...)`.

## 2. Read safely

- Use `openpyxl.load_workbook(path, read_only=True, data_only=False)` for formulas and structural checks.
- Use `data_only=True` only when cached formula values are required. If the file was generated outside Excel, those cached values may be `None`.
- Never call `pandas.read_excel()` on a bloated sheet.
- Never iterate the full reported `max_row` when the probe says the sheet is bloated.
- Read the smallest useful range, then expand deliberately.

## 3. Edit safely

- Make a copy in `.mcp-chat/artifacts/` before changing a workbook:
  `__jarvis-backup__-<filename>-<timestamp>.xlsx`.
- `read_only=True` cannot be saved. For a small/normal workbook, reload it normally only when you need to write.
- For a very large or bloated workbook, avoid a blind normal load. Prefer a targeted write path or Excel COM if available.
- Do not re-save an existing workbook unless the task requires it; adding a new sheet and preserving the original sheets is safer.
- After saving, verify original sheets and key totals with `scripts/diff.py` or a bounded cell comparison.

## 4. Diff before declaring success

Run `scripts/diff.py` on the original and edited copy:

```python
subprocess.run([
    sys.executable, str(skill / "scripts/diff.py"),
    r"before.xlsx", r"after.xlsx",
    "--sheets", "Dirman,Staff",
], check=True)
```

Default comparison uses formulas. Add `--values` to compare cached values. The script caps rows/columns by default; increase limits only when necessary.

## 5. Report

State:

- file and sheet names;
- reported `max_row` vs real rows when bloat matters;
- exact range read/edited;
- backup path;
- diff result and whether any original sheet changed;
- formula-cache caveats when `data_only=True`.

Do not describe a worksheet as "small" based only on file size. A 12 MB workbook can contain only 42 real rows if its used range was stretched to the Excel limit.
