#!/usr/bin/env python3
"""Validate the product feature catalog before a status claim reaches CI."""

from __future__ import annotations

import csv
import re
import sys
from collections import Counter
from pathlib import Path

HEADER = (
    "Feature ID", "Legacy", "Requirement", "Phase", "Surface",
    "Permission", "Dependency", "Acceptance", "Status", "Thesis",
)
STATUSES = {"Planned", "In Progress", "Implemented", "Verified"}
ID = re.compile(r"[A-Z]+-[0-9]{3,}")


def cells(line: str) -> tuple[str, ...]:
    parsed = next(csv.reader([line], delimiter="|", escapechar="\\"))
    return tuple(value.strip() for value in parsed[1:-1])


def check(text: str, *, minimum_rows: int = 685, legacy_max: int = 580) -> list[str]:
    errors: list[str] = []
    match = re.search(r"^### 15\.2 功能矩阵（([0-9]+) 项）\s*$", text, re.M)
    end = re.search(r"^### 15\.3 目录统计\s*$", text, re.M)
    if match is None or end is None or end.start() <= match.end():
        return ["missing or misplaced feature matrix section"]
    lines = [line for line in text[match.end():end.start()].splitlines() if line.startswith("|")]
    if len(lines) < 2 or cells(lines[0]) != HEADER:
        return ["feature matrix header does not match the catalog schema"]
    if len(cells(lines[1])) != len(HEADER) or any(
        re.fullmatch(r":?-{3,}:?", cell) is None for cell in cells(lines[1])
    ):
        return ["feature matrix separator does not match the catalog schema"]
    rows = lines[2:]
    if len(rows) < minimum_rows:
        errors.append(f"feature matrix has {len(rows)} rows; expected at least {minimum_rows}")
    if len(rows) != int(match.group(1)):
        errors.append("feature matrix heading count does not match its rows")

    seen_ids: set[str] = set()
    legacy_numbers: list[int] = []
    statuses: Counter[str] = Counter()
    for number, line in enumerate(rows, 1):
        fields = cells(line)
        if len(fields) != len(HEADER):
            errors.append(f"feature row {number} has {len(fields)} fields; expected {len(HEADER)}")
            continue
        row = dict(zip(HEADER, fields))
        feature_id = row["Feature ID"]
        if not ID.fullmatch(feature_id):
            errors.append(f"feature row {number} has invalid Feature ID {feature_id!r}")
        if feature_id in seen_ids:
            errors.append(f"duplicate Feature ID {feature_id}")
        seen_ids.add(feature_id)
        for field in HEADER:
            if not row[field]:
                errors.append(f"{feature_id or f'row {number}'} is missing {field}")
        legacy = row["Legacy"]
        if legacy != "-":
            if not legacy.isdecimal():
                errors.append(f"{feature_id} has invalid Legacy number {legacy!r}")
            else:
                legacy_numbers.append(int(legacy))
        if not re.fullmatch(r"[0-9]+\.[0-9]+", row["Phase"]):
            errors.append(f"{feature_id} has invalid Phase {row['Phase']!r}")
        if row["Status"] not in STATUSES:
            errors.append(f"{feature_id} has invalid Status {row['Status']!r}")
        statuses[row["Status"]] += 1
        if row["Thesis"] not in {"Yes", "No"}:
            errors.append(f"{feature_id} has invalid Thesis value {row['Thesis']!r}")

    counts = Counter(legacy_numbers)
    missing = sorted(set(range(1, legacy_max + 1)) - counts.keys())
    repeated = sorted(number for number, count in counts.items() if count > 1)
    out_of_range = sorted(number for number in counts if not 1 <= number <= legacy_max)
    if missing:
        errors.append(f"missing Legacy numbers: {missing}")
    if repeated:
        errors.append(f"reused Legacy numbers: {repeated}")
    if out_of_range:
        errors.append(f"Legacy numbers outside 1..{legacy_max}: {out_of_range}")

    for label, actual in (
        ("原始需求映射", len(legacy_numbers)),
        ("新增团队/平台需求", len(rows) - len(legacy_numbers)),
        ("总 Feature ID", len(rows)),
        ("当前 `Verified`", statuses["Verified"]),
        ("当前 `Implemented`", statuses["Implemented"]),
        ("1.0 要求 `Verified`", len(rows)),
    ):
        summary = re.search(rf"^\| {re.escape(label)} \| ([0-9]+)", text[end.start():], re.M)
        if summary is None or int(summary.group(1)) != actual:
            errors.append(f"summary {label} does not match feature rows ({actual})")
    return errors


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[2]
    failures = check((root / "PRODUCT_SPEC.md").read_text(encoding="utf-8"))
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        sys.exit(1)
    print("Panel feature matrix verified.")
