"""Dump what an independent reader (openpyxl) sees in a workbook, as JSON on stdout.

`cargo xtask oracle` runs this against files GridCraft wrote and compares the result with the
model it saved, so a package our own reader tolerates but a foreign one does not shows up.

openpyxl is a different implementation from ours (and a different codebase from Excel), so it
catches the class of defect a self round-trip cannot: a part whose relationships, content types or
ordering are wrong in a way only a strict consumer notices. See docs/oracle.md for what this does
and does not prove.
"""

import json
import sys
import warnings

warnings.filterwarnings("ignore")
import openpyxl  # noqa: E402  (after the warning filter on purpose)

# A dump is a diagnostic, not a loader: cap the work on a workbook that is accidentally huge.
MAX_CELLS = 400_000


def cell_value(v):
    """A JSON-able view of one cell value. Formulas stay formulas (data_only=False)."""
    if v is None:
        return None
    if isinstance(v, bool) or isinstance(v, (int, float, str)):
        return v
    if hasattr(v, "text"):  # ArrayFormula / DataTableFormula
        return {"formula": v.text}
    if hasattr(v, "isoformat"):  # date / time / datetime
        return {"date": v.isoformat()}
    return str(v)


def dump(path):
    wb = openpyxl.load_workbook(path, data_only=False)
    out = {"defined_names": list(wb.defined_names), "sheets": [], "truncated": False}
    total = 0
    for name in wb.sheetnames:
        ws = wb[name]
        cells = {}
        for row in ws.iter_rows():
            for c in row:
                if c.value is None:
                    continue
                total += 1
                if total > MAX_CELLS:
                    out["truncated"] = True
                    break
                cells[c.coordinate] = {"v": cell_value(c.value), "fmt": c.number_format,
                                       "fmtId": getattr(getattr(c, "_style", None), "numFmtId", None)}
            if out["truncated"]:
                break
        out["sheets"].append(
            {
                "name": name,
                "dims": ws.dimensions,
                "cells": cells,
                "merges": sorted(str(r) for r in ws.merged_cells.ranges),
                "tables": {
                    t: {"range": str(ws.tables[t].ref), "columns": [c.name for c in ws.tables[t].tableColumns]}
                    for t in ws.tables
                },
                "conditional_formats": len(ws.conditional_formatting),
                "data_validations": len(ws.data_validations.dataValidation),
                "charts": len(ws._charts),
                "images": len(ws._images),
                "freeze": str(ws.freeze_panes) if ws.freeze_panes else None,
                "autofilter": str(ws.auto_filter.ref) if ws.auto_filter and ws.auto_filter.ref else None,
            }
        )
        if out["truncated"]:
            break
    return out


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.stderr.write("usage: dump.py <workbook.xlsx>\n")
        raise SystemExit(2)
    json.dump(dump(sys.argv[1]), sys.stdout)
