#!/usr/bin/env python3
"""Negative self-tests for the workflow execution policy guard."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path


CHECKER = Path(__file__).with_name("check-workflow-execution-policy.py")
WORKFLOW = """name: Fixture
on: push
permissions:
  contents: read
concurrency:
  group: fixture
  cancel-in-progress: true
jobs:
  test:
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    steps: []
"""


def check(root: Path) -> int:
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            str(root),
            "--workflow-root",
            str(root / ".github/workflows"),
            "--policy",
            str(root / ".github/policies/workflow-execution.json"),
            "--today",
            TODAY,
        ],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode


def write_fixture(
    root: Path,
    workflow: str = WORKFLOW,
    job_permissions: dict[str, object] | None = None,
) -> tuple[Path, Path]:
    workflow_path = root / ".github/workflows/fixture.yml"
    policy_path = root / ".github/policies/workflow-execution.json"
    workflow_path.parent.mkdir(parents=True, exist_ok=True)
    policy_path.parent.mkdir(parents=True, exist_ok=True)
    workflow_path.write_text(workflow, encoding="utf-8")
    entry: dict[str, object] = {
        "permissions": {"contents": "read"},
        "cancel_in_progress": True,
    }
    if job_permissions is not None:
        entry["job_permissions"] = job_permissions
    policy_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "max_job_timeout_minutes": 60,
                "workflows": {".github/workflows/fixture.yml": entry},
            }
        ),
        encoding="utf-8",
    )
    return workflow_path, policy_path


def escalating_workflow(levels: str = "      contents: write\n") -> str:
    return WORKFLOW.replace("    steps: []", f"    permissions:\n{levels}    steps: []")


#: Scenarios are evaluated at a pinned date, so a fixture cannot pass today and
#: fail once its lease runs out.
TODAY = "2026-09-04"
LIVE = "2027-01-01"


def lease(
    permissions: dict[str, str],
    *,
    owner: str = "pingora-panel-security",
    expires_on: str = LIVE,
) -> dict[str, object]:
    return {
        "test": {
            "owner": owner,
            "reason": "test fixture escalation",
            "expires_on": expires_on,
            "permissions": permissions,
        }
    }


def require_rejected(
    root: Path,
    workflow: str,
    scenario: str,
    job_permissions: dict[str, object] | None = None,
) -> None:
    write_fixture(root, workflow, job_permissions)
    if check(root) == 0:
        raise AssertionError(f"{scenario} was not rejected")


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="workflow-execution-policy-") as temporary:
        root = Path(temporary)
        workflow, policy = write_fixture(root)
        if check(root) != 0:
            raise AssertionError("valid workflow execution policy was rejected")

        quoted_workflow = (
            WORKFLOW.replace("permissions:", '"permissions":', 1)
            .replace("  contents:", '  "contents":', 1)
            .replace("concurrency:", "'concurrency':", 1)
            .replace("  group:", "  'group':", 1)
            .replace("  cancel-in-progress:", '  "cancel-in-progress":', 1)
            .replace("jobs:", '"jobs":', 1)
            .replace("  test:", "  'test':", 1)
            .replace("    timeout-minutes:", '    "timeout-minutes":', 1)
        )
        write_fixture(root, quoted_workflow)
        if check(root) != 0:
            raise AssertionError("canonical quoted mapping keys were rejected")

        require_rejected(root, WORKFLOW.replace("permissions:\n", ""), "missing permissions")
        require_rejected(
            root,
            WORKFLOW.replace("contents: read", "contents: write"),
            "permission escalation",
        )
        require_rejected(
            root,
            WORKFLOW.replace("    timeout-minutes: 10\n", ""),
            "missing job timeout",
        )
        require_rejected(
            root,
            WORKFLOW.replace("timeout-minutes: 10", "timeout-minutes: 61"),
            "excessive job timeout",
        )
        require_rejected(root, WORKFLOW.replace("concurrency:\n", ""), "missing concurrency")
        require_rejected(
            root,
            WORKFLOW.replace("cancel-in-progress: true", "cancel-in-progress: false"),
            "unexpected concurrency cancellation",
        )
        require_rejected(
            root,
            escalating_workflow(),
            "unleased job-level permission override",
        )
        require_rejected(
            root,
            WORKFLOW.replace(
                "    steps: []", '    "permissions":\n      contents: write\n    steps: []'
            ),
            "quoted unleased job-level permission override",
        )
        require_rejected(
            root,
            WORKFLOW.replace(
                "concurrency:", '"permissions":\n  contents: write\nconcurrency:'
            ),
            "quoted duplicate top-level permissions override",
        )
        require_rejected(
            root,
            WORKFLOW
            + '  "hidden":\n'
            + "    runs-on: ubuntu-24.04\n"
            + "    steps: []\n",
            "quoted job without a timeout",
        )
        require_rejected(
            root,
            WORKFLOW
            + '  "test":\n'
            + "    runs-on: ubuntu-24.04\n"
            + "    timeout-minutes: 10\n"
            + "    steps: []\n",
            "quoted duplicate job identifier",
        )

        write_fixture(root, escalating_workflow(), lease({"contents": "write"}))
        if check(root) != 0:
            raise AssertionError("leased job-level permission override was rejected")

        require_rejected(
            root,
            escalating_workflow("      contents: write\n      issues: write\n"),
            "job-level permissions wider than the lease",
            lease({"contents": "write"}),
        )
        require_rejected(
            root,
            WORKFLOW,
            "lease for a job that does not override permissions",
            lease({"contents": "write"}),
        )
        require_rejected(
            root,
            escalating_workflow(),
            "lease naming an absent job",
            {
                "absent": {
                    "owner": "pingora-panel-security",
                    "reason": "test fixture escalation",
                    "expires_on": LIVE,
                    "permissions": {"contents": "write"},
                }
            },
        )
        require_rejected(
            root,
            escalating_workflow(),
            "lease without an accountable owner",
            {"test": {"permissions": {"contents": "write"}}},
        )
        require_rejected(
            root,
            escalating_workflow(),
            "escalation lease with no review date",
            {
                "test": {
                    "owner": "pingora-panel-security",
                    "reason": "test fixture escalation",
                    "permissions": {"contents": "write"},
                }
            },
        )
        require_rejected(
            root,
            escalating_workflow(),
            "expired escalation lease",
            lease({"contents": "write"}, expires_on="2026-09-03"),
        )
        require_rejected(
            root,
            escalating_workflow(),
            "escalation lease expiring beyond the review horizon",
            lease({"contents": "write"}, expires_on="2999-01-01"),
        )
        require_rejected(
            root,
            escalating_workflow(),
            "escalation lease naming nobody accountable",
            lease({"contents": "write"}, owner="nobody-in-particular"),
        )
        require_rejected(
            root,
            escalating_workflow("      contents: write\n    permissions:\n      issues: write\n"),
            "duplicate job-level permissions mappings",
            lease({"contents": "write"}),
        )
        require_rejected(
            root,
            WORKFLOW.replace("    steps: []", "    permissions: write-all\n    steps: []"),
            "job-level write-all permission scalar",
            lease({"contents": "write"}),
        )
        require_rejected(
            root,
            WORKFLOW.replace("    steps: []", "    permissions: {}\n    steps: []"),
            "job-level inline permissions mapping",
        )

        write_fixture(root, WORKFLOW)
        workflow.write_text(WORKFLOW, encoding="utf-8")
        extra = root / ".github/workflows/unclassified.yml"
        extra.write_text(WORKFLOW, encoding="utf-8")
        if check(root) == 0:
            raise AssertionError("unclassified workflow was not rejected")
        extra.unlink()

        malformed = json.loads(policy.read_text(encoding="utf-8"))
        malformed["unknown"] = True
        policy.write_text(json.dumps(malformed), encoding="utf-8")
        if check(root) == 0:
            raise AssertionError("unknown policy field was not rejected")

    print("Workflow execution policy self-test passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
