#!/usr/bin/env python3
"""Negative and positive contract cases for the feature catalog guard."""

from __future__ import annotations

import importlib.util
from pathlib import Path

guard = Path(__file__).with_name("check-panel-feature-matrix.py")
spec = importlib.util.spec_from_file_location("feature_matrix_guard", guard)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def fixture(rows: list[str]) -> str:
    parsed = [module.cells(row) for row in rows]
    legacy_count = sum(row[1] != "-" for row in parsed)
    verified_count = sum(row[8] == "Verified" for row in parsed)
    implemented_count = sum(row[8] == "Implemented" for row in parsed)
    return (
        f"### 15.2 功能矩阵（{len(rows)} 项）\n"
        "| Feature ID | Legacy | Requirement | Phase | Surface | Permission | Dependency | Acceptance | Status | Thesis |\n"
        "|---|---:|---|---:|---|---|---|---|---|---|\n"
        + "\n".join(rows)
        + "\n### 15.3 目录统计\n"
        + f"| 原始需求映射 | {legacy_count} |\n"
        + f"| 新增团队/平台需求 | {len(rows) - legacy_count} |\n"
        + f"| 总 Feature ID | {len(rows)} |\n"
        + f"| 当前 `Verified` | {verified_count} |\n"
        + f"| 当前 `Implemented` | {implemented_count} |\n"
        + f"| 1.0 要求 `Verified` | {len(rows)} |\n"
    )


first = "| SITE-001 | 1 | site | 0.2 | A | Viewer | config-service | check | Verified | No |"
second = "| SITE-002 | 2 | site | 0.2 | A | Viewer | config-service | check | Planned | No |"
valid = fixture([first, second])
check = lambda text: module.check(text, minimum_rows=2, legacy_max=2)
assert check(valid) == [], check(valid)
assert any("duplicate Feature ID" in error for error in check(fixture([first, first])))
assert any("missing Legacy" in error for error in check(fixture([first, second.replace("| 2 |", "| - |", 1)])))
assert any("missing Acceptance" in error for error in check(fixture([first, second.replace("| check |", "|  |", 1)])))
assert any("invalid Phase" in error for error in check(fixture([first, second.replace("| 0.2 |", "| later |", 1)])))
assert any("invalid Status" in error for error in check(fixture([first, second.replace("| Planned |", "| Done |", 1)])))
assert any("separator" in error for error in check(valid.replace("|---|---:|", "| not-a-separator |---:|", 1)))
assert any("summary" in error for error in check(valid.replace("| 总 Feature ID | 2 |", "| 总 Feature ID | 3 |")))
assert any("heading count" in error for error in check(valid.replace("（2 项）", "（3 项）")))
new_feature = second.replace("| 2 |", "| - |", 1)
assert module.check(fixture([first, new_feature]), minimum_rows=2, legacy_max=1) == []
assert any(
    "expected at least 2" in error
    for error in module.check(fixture([first]), minimum_rows=2, legacy_max=1)
)
print("Panel feature matrix guard self-test passed.")
