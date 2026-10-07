#!/usr/bin/env python3
"""A privileged workflow must not run PR code unguarded or write caches.

`pull_request_target` and `workflow_run` run with the base repository's token and
cache scope. For each such workflow this fails when:

- a job checks out the PR head (a checkout `ref` or `repository` naming the head
  commit, branch or repository) and neither it nor a job it needs gates on
  the head repository being this one;
- any job saves a cache: `actions/cache`, or `setup-rust-ci` or `rust-cache`
  without `save-if: false`.

    python3 scripts/check-workflow-triggers.py [workflow.yml ...]
"""

import re
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
# The comparison a job's `if` must contain, per trigger.
GUARDS = {
    "workflow_run": "github.event.workflow_run.head_repository.full_name == github.repository",
    "pull_request_target": "github.event.pull_request.head.repo.full_name == github.repository",
}
HEAD_REF = re.compile(r"\bhead_(?:sha|ref|branch|repository)\b|\.head\.(?:sha|ref|repo)\b|workflow_run\.head")
STATUS_FUNCTION = re.compile(r"\b(?:always|failure|cancelled)\(\)")  # run even when a needed job is skipped
SAVES_CACHE = ("actions/cache", "actions/cache/save")  # unconditional writers
SAVE_IF_ACTIONS = ("./.github/actions/setup-rust-ci", "swatinem/rust-cache")  # write unless save-if is false


def condition(node):
    return " ".join(str(node.get("if", "")).split())


def uses(step):
    return str(step.get("uses", "")).split("@")[0]


def checks_out_head(step):
    if uses(step) != "actions/checkout":
        return False
    with_ = step.get("with") or {}
    return any(HEAD_REF.search(str(with_.get(key, ""))) for key in ("ref", "repository"))


def saves_cache(step):
    action = uses(step).lower()
    if action in SAVES_CACHE:
        return True
    return action in SAVE_IF_ACTIONS and str((step.get("with") or {}).get("save-if", "")).lower() != "false"


def guarded(name, jobs, guard, seen=()):
    job = jobs[name]
    if guard in condition(job):
        return True
    if STATUS_FUNCTION.search(condition(job)):
        return False
    needs = job.get("needs") or []
    needs = [needs] if isinstance(needs, str) else needs
    return any(guarded(n, jobs, guard, seen + (name,)) for n in needs if n in jobs and n not in seen)


def check(path):
    workflow = yaml.safe_load(path.read_text())
    triggers = workflow.get("on", workflow.get(True)) or {}
    triggers = [triggers] if isinstance(triggers, str) else list(triggers)
    privileged = [t for t in GUARDS if t in triggers]
    if not privileged:
        return []
    errors = []
    jobs = workflow.get("jobs") or {}
    for name, job in jobs.items():
        steps = job.get("steps") or []
        if any(checks_out_head(s) for s in steps):
            for trigger in privileged:
                if not guarded(name, jobs, GUARDS[trigger]):
                    errors.append(f"{path.name}: job `{name}` checks out the PR head under {trigger} without `{GUARDS[trigger]}`")
        for step in steps:
            if saves_cache(step):
                errors.append(f"{path.name}: job `{name}` saves a cache under a privileged trigger ({step.get('uses')}); set `save-if: false`")
    return errors


def main(argv):
    paths = [Path(a) for a in argv] or sorted((ROOT / ".github/workflows").glob("*.y*ml"))
    errors = [e for p in paths for e in check(p)]
    if errors:
        print("\n".join(errors))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
