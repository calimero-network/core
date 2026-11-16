"""Fail if a workflow has no top-level permissions block, grants an unlisted write scope, or uses an action by a mutable ref."""

import re
import sys
from pathlib import Path

import yaml

WORKFLOWS = Path(__file__).resolve().parent.parent / ".github" / "workflows"
PINNED = re.compile(r"^[^@]+@[0-9a-f]{40}$")
# Write scopes a job may hold, keyed by (workflow file, job); everything else is read-only.
ALLOWED_WRITES = {
    ("build-wheels.yml", "publish-pypi"): {"id-token"},
    ("docs-site.yml", "build-deploy"): {"id-token", "pages"},
}


def used_actions(doc) -> list:
    jobs = (doc.get("jobs") or {}).values()
    steps = [step for job in jobs for step in job.get("steps") or []]
    return [x["uses"] for x in [*jobs, *steps] if "uses" in x]


def writes(permissions) -> set:
    if permissions == "write-all":
        return {"write-all"}
    if not isinstance(permissions, dict):
        return set()
    return {scope for scope, level in permissions.items() if level == "write"}


def excess_writes(workflow: Path, doc) -> list:
    held = {None: writes(doc.get("permissions"))}
    held.update({n: writes(j.get("permissions")) for n, j in doc["jobs"].items()})
    found = []
    for job, scopes in held.items():
        allowed = ALLOWED_WRITES.get((workflow.name, job), set())
        for scope in sorted(scopes - allowed):
            where = f"job {job}" if job else "workflow level"
            found.append(f"{workflow.name}: {where} grants {scope}: write")
    return found


def violations(workflow: Path) -> list:
    doc = yaml.safe_load(workflow.read_text())
    found = []
    if "permissions" not in doc:
        found.append(f"{workflow.name}: no top-level permissions block")
    found += excess_writes(workflow, doc)
    for uses in used_actions(doc):
        if not uses.startswith("./") and not PINNED.match(uses):
            found.append(f"{workflow.name}: {uses} is not pinned to a commit SHA")
    return found


def main(paths) -> int:
    found = [v for path in paths for v in violations(path)]
    for v in found:
        print(f"::error::{v}")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sorted(WORKFLOWS.glob("*.y*ml"))))
