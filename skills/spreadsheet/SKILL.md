---
name: spreadsheet
description: "Create, inspect, safely edit, restructure, and diff Excel .xlsx workbooks and CSVs with openpyxl."
---

# Spreadsheet workflow

Use the `python` tool for spreadsheet work. Do not load a whole workbook with pandas or with `openpyxl.load_workbook()` in normal mode before checking its shape.

The skill is installed at `~/.config/mcp-chat/skills/spreadsheet` (or `%USERPROFILE%\.config\mcp-chat\skills\spreadsheet` on Windows), or in a workspace at `.mcp-chat/skills/spreadsheet`. Use this resolver in a `python` call:

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
```

## 1. Probe first

Run `scripts/probe.py` before reading or editing an existing workbook. It uses `read_only=True` and bounds the row scan, so a sheet whose formatting inflated `max_row` to 1,048,576 does not take minutes to inspect.

```python
subprocess.run([sys.executable, str(skill / "scripts/probe.py"), r"path\to\file.xlsx"], check=True)
```

Read the JSON output. Prefer sheets where `safe_to_read_full` is true. For `bloated: true`, read only the known range with `iter_rows(min_row=..., max_row=...)`.

## 2. Create from a JSON spec

Write a spec with the `write` tool, then run `xlsx_create.py`. The spec supports styled cells, multiple sheets, number formats, merges, freeze panes, autofilter, conditional formatting, charts, data-validation dropdowns, native tables, defined names, hyperlinks, notes and sheet protection.

```python
subprocess.run([sys.executable, str(skill / "scripts/xlsx_create.py"), "spec.json", "report.xlsx"], check=True)
```

For formulas, add `"full_calc_on_load": true` to the spec or run `xlsx_recalc.py` afterward. openpyxl never evaluates formulas.

## 3. Read safely

- Use `xlsx_read.py --sheets` for sheet names, dimensions, merges, charts, tables and protection.
- Use `--json` or `--csv` for data, `--formulas` for formula/cached-value pairs, `--notes` for comments, `--names` for defined names.
- Use `openpyxl.load_workbook(path, read_only=True, data_only=False)` for formulas and structural checks.
- Use `data_only=True` only when cached formula values are required. If the file was generated outside Excel, those cached values may be `None`.
- Never call `pandas.read_excel()` on a bloated sheet.
- Never iterate the full reported `max_row` when the probe says the sheet is bloated.
- Read the smallest useful range, then expand deliberately.

```python
subprocess.run([sys.executable, str(skill / "scripts/xlsx_read.py"), "file.xlsx", "--sheets"], check=True)
subprocess.run([sys.executable, str(skill / "scripts/xlsx_read.py"), "file.xlsx", "--json", "--sheet", "Data"], check=True)
```

## 4. Edit safely

- Make a copy in `.mcp-chat/artifacts/` before changing a workbook:
  `__jarvis-backup__-<filename>-<timestamp>.xlsx`.
- `read_only=True` cannot be saved. For a small/normal workbook, reload it normally only when you need to write.
- For a very large or bloated workbook, avoid a blind normal load. Prefer a targeted write path or Excel COM if available.
- Do not re-save an existing workbook unless the task requires it; adding a new sheet and preserving the original sheets is safer.
- Always verify original sheets and key totals after saving.

```python
subprocess.run([sys.executable, str(skill / "scripts/xlsx_edit.py"), "file.xlsx", "--sheet", "Data", "--set", "B2=42"], check=True)
subprocess.run([sys.executable, str(skill / "scripts/xlsx_edit.py"), "file.xlsx", "--append", '[1,"x",true]'], check=True)
```

Use `xlsx_restructure.py` for insert/delete rows or columns when formulas, merges, tables or filters must stay correct. It rewrites references across sheets and prints a `not_shifted` report.

```python
subprocess.run([sys.executable, str(skill / "scripts/xlsx_restructure.py"), "file.xlsx", "--insert-rows", "3:2"], check=True)
```

Use `xlsx_recalc.py` when LibreOffice is installed and formula results must be materialized. Use `csv_to_xlsx.py` / `xlsx_to_csv.py` for UTF-8 CSV interop; pass `--encoding` explicitly for legacy files.

## 5. Diff before declaring success

Run `scripts/diff.py` on the original and edited copy:

```python
subprocess.run([
    sys.executable, str(skill / "scripts/diff.py"),
    r"before.xlsx", r"after.xlsx",
    "--sheets", "Dirman,Staff",
], check=True)
```

Default comparison uses formulas. Add `--values` to compare cached values. The script caps rows/columns by default; increase limits only when necessary.

## Quick reference

| Task | Command |
| --- | --- |
| Probe a workbook | `probe.py file.xlsx` |
| Create from spec | `xlsx_create.py spec.json out.xlsx` |
| Sheet inventory | `xlsx_read.py file.xlsx --sheets` |
| Dump sheet as JSON | `xlsx_read.py file.xlsx --json --sheet Data` |
| List formulas + cached values | `xlsx_read.py file.xlsx --formulas` |
| Set a cell / formula | `xlsx_edit.py file.xlsx --set "A1==SUM(B:B)"` |
| Append a row | `xlsx_edit.py file.xlsx --append '[1,"x",true]'` |
| Insert rows with shifted refs | `xlsx_restructure.py file.xlsx --insert-rows 3:2` |
| Recalculate via LibreOffice | `xlsx_recalc.py file.xlsx` |
| CSV to styled workbook | `csv_to_xlsx.py data.csv out.xlsx` |
| Workbook to CSV | `xlsx_to_csv.py file.xlsx out.csv --sheet Data` |
| Diff before/after | `diff.py before.xlsx after.xlsx` |

## Pitfalls

- **openpyxl does not calculate formulas.** Use `xlsx_recalc.py` or Excel/LibreOffice; cached values are `None` in files never opened by a spreadsheet app.
- **Saving can drop charts/images.** openpyxl does not round-trip charts or images. Avoid re-saving charted workbooks, or re-add charts afterward.
- **Sheet protection is not security.** It only signals well-behaved apps to avoid edits.
- **Restructuring has limits.** `xlsx_restructure.py` shifts formulas, merges and ranges, but not chart anchors, images or conditional-format rule formulas; read its `not_shifted` report.
- **Dates are datetimes.** Excel stores serial numbers; dumps emit ISO strings.
- **CSV locale traps.** Pass explicit encodings and delimiters; European CSVs often use `;` and decimal commas.
- **Do not describe a workbook as small based on file size.** A 12 MB workbook can contain only 42 real rows if its used range was stretched to the Excel limit.

## Report

State:

- file and sheet names;
- reported `max_row` vs real rows when bloat matters;
- exact range read/edited;
- backup path;
- diff result and whether any original sheet changed;
- formula-cache, chart/image, or restructure caveats when relevant.
