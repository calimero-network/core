"""Fail if a workflow can mint an OIDC token outside a GitHub environment.

PyPI trusted publishing accepts any OIDC token from build-wheels.yml, so only
an environment with required reviewers stands between a push and a release.
"""

import sys
from pathlib import Path

import yaml

WORKFLOWS = Path(__file__).resolve().parent.parent / ".github" / "workflows"


def grants_id_token(permissions) -> bool:
    if permissions == "write-all":
        return True
    return isinstance(permissions, dict) and permissions.get("id-token") == "write"


def fixed_environment(job) -> bool:
    """True if the job names an environment that cannot evaluate to empty."""
    env = job.get("environment")
    name = env.get("name") if isinstance(env, dict) else env
    return isinstance(name, str) and name != "" and "${{" not in name


def violations(workflow: Path) -> list:
    doc = yaml.safe_load(workflow.read_text())
    found = []
    for name, job in (doc.get("jobs") or {}).items():
        # A job without its own permissions block inherits the workflow's.
        permissions = job.get("permissions", doc.get("permissions"))
        if grants_id_token(permissions) and not fixed_environment(job):
            found.append(
                f"{workflow.name}: job {name} can mint an OIDC token outside a fixed environment"
            )
    return found


def main(paths) -> int:
    found = [v for path in paths for v in violations(path)]
    for v in found:
        print(f"::error::{v}")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sorted(WORKFLOWS.glob("*.y*ml"))))
