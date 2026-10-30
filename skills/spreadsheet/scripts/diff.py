#!/usr/bin/env python3
"""Compare two .xlsx workbooks cell-by-cell without a full normal load.

Usage:
    python diff.py BEFORE.xlsx AFTER.xlsx [--sheets Sheet1,Sheet2]
                     [--values] [--max-rows 10000] [--max-cols 200]

Default mode compares formulas (data_only=False). --values compares cached
values (data_only=True); cached values are None if the file was never saved by
Excel after formulas were written.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

try:
    from openpyxl import load_workbook
    from openpyxl.utils import get_column_letter
except ImportError as exc:  # pragma: no cover - depends on the desktop environment
    raise SystemExit(
        "openpyxl is required. Install it with: python -m pip install openpyxl"
    ) from exc


def compare_sheet(ws_a, ws_b, max_rows: int, max_cols: int) -> tuple[list[dict], bool]:
    rows_a = int(ws_a.max_row or 0)
    rows_b = int(ws_b.max_row or 0)
    cols_a = int(ws_a.max_column or 0)
    cols_b = int(ws_b.max_column or 0)
    rows = min(max(rows_a, rows_b), max_rows)
    cols = min(max(cols_a, cols_b), max_cols)
    truncated = max(rows_a, rows_b) > rows or max(cols_a, cols_b) > cols

    differences: list[dict] = []
    if rows <= 0 or cols <= 0:
        return differences, truncated

    rows_b_iter = ws_b.iter_rows(
        min_row=1,
        max_row=rows,
        min_col=1,
        max_col=cols,
        values_only=True,
    )
    for row_index, row_a in enumerate(
        ws_a.iter_rows(
            min_row=1,
            max_row=rows,
            min_col=1,
            max_col=cols,
            values_only=True,
        ),
        start=1,
    ):
        row_b = next(rows_b_iter, ())
        for col_index in range(cols):
            old = row_a[col_index] if col_index < len(row_a) else None
            new = row_b[col_index] if col_index < len(row_b) else None
            if old != new:
                differences.append(
                    {
                        "sheet": ws_a.title,
                        "cell": f"{get_column_letter(col_index + 1)}{row_index}",
                        "old": old,
                        "new": new,
                    }
                )
    return differences, truncated


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument(
        "--sheets",
        help="Comma-separated sheet names. Default: all sheets from both files.",
    )
    parser.add_argument(
        "--values",
        action="store_true",
        help="Compare cached values instead of formulas.",
    )
    parser.add_argument("--max-rows", type=int, default=10_000)
    parser.add_argument("--max-cols", type=int, default=200)
    args = parser.parse_args()

    before = args.before.expanduser().resolve()
    after = args.after.expanduser().resolve()
    for path in (before, after):
        if not path.is_file():
            print(f"File not found: {path}", file=sys.stderr)
            return 2

    max_rows = max(1, args.max_rows)
    max_cols = max(1, args.max_cols)
    data_only = args.values

    wb_before = load_workbook(before, read_only=True, data_only=data_only)
    wb_after = load_workbook(after, read_only=True, data_only=data_only)
    try:
        names_before = wb_before.sheetnames
        names_after = wb_after.sheetnames
        requested = (
            [name.strip() for name in args.sheets.split(",") if name.strip()]
            if args.sheets
            else list(dict.fromkeys(names_before + names_after))
        )

        differences: list[dict] = []
        missing_sheets: list[dict] = []
        truncated_sheets: list[str] = []
        for name in requested:
            if name not in names_before or name not in names_after:
                missing_sheets.append(
                    {
                        "sheet": name,
                        "in_before": name in names_before,
                        "in_after": name in names_after,
                    }
                )
                continue
            found, truncated = compare_sheet(
                wb_before[name],
                wb_after[name],
                max_rows,
                max_cols,
            )
            differences.extend(found)
            if truncated:
                truncated_sheets.append(name)

        result = {
            "before": str(before),
            "after": str(after),
            "mode": "values" if data_only else "formulas",
            "sheets_checked": requested,
            "difference_count": len(differences),
            "differences": differences,
            "missing_sheets": missing_sheets,
            "truncated_sheets": truncated_sheets,
            "max_rows": max_rows,
            "max_cols": max_cols,
        }
        print(json.dumps(result, ensure_ascii=False, indent=2, default=str))
        return 0
    finally:
        wb_before.close()
        wb_after.close()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
