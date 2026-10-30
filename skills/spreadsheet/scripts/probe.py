#!/usr/bin/env python3
"""Probe an .xlsx workbook without loading every formatted cell.

Usage:
    python probe.py BOOK.xlsx [--scan-limit 10000] [--empty-run 200]
                              [--max-cols 200]

Output is JSON intended for the agent, not a human-only report.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

try:
    from openpyxl import load_workbook
except ImportError as exc:  # pragma: no cover - depends on the desktop environment
    raise SystemExit(
        "openpyxl is required. Install it with: python -m pip install openpyxl"
    ) from exc


def value_is_empty(value: object) -> bool:
    return value is None or (isinstance(value, str) and not value.strip())


def probe_sheet(ws, scan_limit: int, empty_run: int, max_cols: int) -> dict:
    reported_rows = int(ws.max_row or 0)
    reported_cols = int(ws.max_column or 0)
    row_limit = min(reported_rows, scan_limit) if reported_rows else 0
    col_limit = min(reported_cols, max_cols) if reported_cols else 0

    real_rows = 0
    last_nonempty = 0
    scanned_rows = 0
    consecutive_empty = 0
    stopped_early = False

    if row_limit and col_limit:
        for row in ws.iter_rows(
            min_row=1,
            max_row=row_limit,
            min_col=1,
            max_col=col_limit,
            values_only=True,
        ):
            scanned_rows += 1
            if any(not value_is_empty(value) for value in row):
                real_rows += 1
                last_nonempty = scanned_rows
                consecutive_empty = 0
            else:
                consecutive_empty += 1
                # Do not let a leading blank block stop the probe.
                if scanned_rows > max(10, empty_run) and consecutive_empty >= empty_run:
                    stopped_early = True
                    break

    truncated = reported_rows > scanned_rows
    # A sheet is suspicious when its declared range is many times larger than
    # the observed data, or when it reaches Excel's row ceiling with little data.
    bloated = False
    if reported_rows > 5_000:
        bloated = reported_rows >= 100_000 or reported_rows > max(real_rows * 20, 5_000)
    safe_to_read_full = not truncated and not bloated and reported_rows <= scan_limit

    return {
        "sheet": ws.title,
        "state": ws.sheet_state,
        "max_row": reported_rows,
        "max_col": reported_cols,
        "real_rows_sampled": real_rows,
        "last_nonempty_row_sampled": last_nonempty,
        "scanned_rows": scanned_rows,
        "scanned_cols": col_limit,
        "scan_truncated": truncated,
        "stopped_after_empty_run": stopped_early,
        "bloated": bloated,
        "safe_to_read_full": safe_to_read_full,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path)
    parser.add_argument("--scan-limit", type=int, default=10_000)
    parser.add_argument("--empty-run", type=int, default=200)
    parser.add_argument("--max-cols", type=int, default=200)
    args = parser.parse_args()

    path = args.path.expanduser().resolve()
    if not path.is_file():
        print(f"File not found: {path}", file=sys.stderr)
        return 2

    scan_limit = max(1, args.scan_limit)
    empty_run = max(1, args.empty_run)
    max_cols = max(1, args.max_cols)

    workbook = load_workbook(path, read_only=True, data_only=False)
    try:
        sheets = [
            probe_sheet(ws, scan_limit, empty_run, max_cols)
            for ws in workbook.worksheets
        ]
    finally:
        workbook.close()

    result = {
        "path": str(path),
        "file_size_bytes": path.stat().st_size,
        "sheet_count": len(sheets),
        "bloated_sheet_count": sum(1 for sheet in sheets if sheet["bloated"]),
        "sheets": sheets,
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
