#!/usr/bin/env python3
"""Fail a `pull_request_target` or `workflow_run` workflow that may run PR code without a
same-repository guard, or that writes a cache; anything it cannot judge fails too.

    python3 scripts/check-workflow-triggers.py [workflow.yml ...]
"""

import re
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
GUARDS = {  # the comparison a job's `if` must imply, per trigger (either operand order)
    "workflow_run": {"github.event.workflow_run.head_repository.full_name", "github.repository"},
    "pull_request_target": {"github.event.pull_request.head.repo.full_name", "github.repository"},
}
FETCHES_CODE = re.compile(
    r"\bgh\s+(?:pr\s+checkout|repo\s+clone|run\s+download)\b|\bgit\s+(?:fetch|checkout|switch|pull|clone|worktree|submodule)\b"
    r"|\b(?:curl|wget)\b|refs/pull/|\bpull/\d|/(?:tarball|zipball|archive)/",
    re.IGNORECASE,
)
STATUS_FUNCTION = re.compile(r"\b(?:always|failure|cancelled)\s*\(", re.IGNORECASE)  # runs even when a needed job is skipped
EVENT_TEST = re.compile(r"^github\.event_name\s*(==|!=)\s*'([^']*)'$")
CACHE_INPUT = re.compile(r"cache|cache-to|(?!no-)[\w-]*-cache")  # inputs that turn on a cache write
CROSS_RUN_ARTIFACTS = {("comment.yml", "submit")}  # reads a PR run's payload as data and verifies it against the PR
SAVE_IF_ACTION = "./.github/actions/setup-rust-ci"  # saves unless `save-if: false`
CACHE_READER = "actions/cache/restore"


class Loader(yaml.SafeLoader):
    """YAML 1.2 booleans as GitHub reads them (`on`, `yes` and `no` stay strings), duplicate keys rejected."""


def construct_mapping(loader, node, deep=False):
    keys = [loader.construct_object(k, deep=True) for k, _ in node.value]
    if len(keys) != len(set(keys)):
        raise ValueError(f"duplicate key in {keys}")
    return yaml.SafeLoader.construct_mapping(loader, node, deep)


Loader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, construct_mapping)
Loader.yaml_implicit_resolvers = {
    char: [(tag, regexp) for tag, regexp in resolvers if tag != "tag:yaml.org,2002:bool"]
    for char, resolvers in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
Loader.add_implicit_resolver("tag:yaml.org,2002:bool", re.compile(r"^(?:true|True|TRUE|false|False|FALSE)$"), list("tTfF"))


def top_level(text):
    """Indices of `text` outside quotes and parentheses; raises if either is unbalanced."""
    depth, quoted, found = 0, False, []
    for i, c in enumerate(text):
        quoted ^= c == "'"
        if not quoted:
            depth += (c == "(") - (c == ")")
            if depth < 0:
                break
            if depth == 0:
                found.append(i)
    if quoted or depth:
        raise ValueError(f"unbalanced expression: {text}")
    return found


def parse(text):
    """Expression to ('||' or '&&', [nodes]) or ('atom', text)."""
    text = text.strip()
    top = top_level(text)
    for op in ("||", "&&"):
        cuts = [i for i in top if text.startswith(op, i)]
        if cuts:
            return op, [parse(text[a:b]) for a, b in zip([0] + [i + 2 for i in cuts], cuts + [len(text)])]
    if text.startswith("(") and top == [len(text) - 1]:
        return parse(text[1:-1])
    if not text:
        raise ValueError("malformed expression")
    return "atom", text


def holds(node, test):
    """Whether every way `node` can be true includes an atom passing `test`."""
    kind, body = node
    if kind == "atom":
        return test(body)
    return (any if kind == "&&" else all)(holds(n, test) for n in body)


def is_guard(atom, trigger):
    return {s.strip().casefold() for s in atom.split("==", 1)} == GUARDS[trigger]  # context names are case-insensitive


def other_event(atom, trigger):
    match = EVENT_TEST.match(atom)
    return bool(match) and (match.group(1) == "==") != (match.group(2).casefold() == trigger)


def expression(job):
    text = " ".join(str(job.get("if", "")).split())
    match = re.fullmatch(r"\$\{\{(.*)\}\}", text)
    if "${{" in text and not match:
        raise ValueError(f"`if` mixes text and an expression: {text}")
    return match.group(1).strip() if match else text


def guarded(name, jobs, trigger, seen=()):
    job = jobs[name]
    text = expression(job)
    if text and holds(parse(text), lambda a: is_guard(a, trigger) or other_event(a, trigger)):
        return True
    if STATUS_FUNCTION.search(text):
        return False
    needs = job.get("needs") or []
    needs = [needs] if isinstance(needs, str) else needs
    return any(guarded(n, jobs, trigger, seen + (name,)) for n in needs if n in jobs and n not in seen)


def skips_privileged_events(step, privileged):
    text = expression(step)
    return bool(text) and all(holds(parse(text), lambda a: other_event(a, t)) for t in privileged)


def step_errors(step, unguarded, root, privileged, artifacts_ok=False):
    action = str(step.get("uses", "")).split("@")[0]
    lower = action.lower()
    with_ = step.get("with") or {}
    errors = []
    if action == "actions/checkout":
        ref, repo = str(with_.get("ref", "${{ github.sha }}")), str(with_.get("repository", "${{ github.repository }}"))
        if unguarded and (ref != "${{ github.sha }}" or repo != "${{ github.repository }}"):
            errors.append(f"checks out `{ref}` of `{repo}` without a same-repository guard")
    elif "checkout" in lower:
        errors.append(f"uses an unrecognised checkout action ({action})")
    if lower == SAVE_IF_ACTION and str(with_.get("save-if", "")).lower() != "false":
        errors.append(f"saves a cache ({action}); set `save-if: false`")
    elif "cache" in lower and lower not in (SAVE_IF_ACTION, CACHE_READER) and not skips_privileged_events(step, privileged):
        errors.append(f"uses a cache action that is not read-only ({action})")
    errors += [f"turns on a cache write with `{k}: {v}`" for k, v in with_.items() if CACHE_INPUT.fullmatch(k) and str(v).lower() != "false"]
    if unguarded and FETCHES_CODE.search(str(step.get("run", ""))):
        errors.append("fetches code or artifacts in `run` without a same-repository guard")
    if unguarded and not artifacts_ok:
        if lower == "actions/download-artifact" and {"run-id", "repository", "github-token"} & set(with_):
            errors.append("downloads another run's artifact without a same-repository guard")
        if "artifacts_url" in str(step).lower():
            errors.append("reads `artifacts_url` without a same-repository guard")
    if action.startswith("./") and lower != SAVE_IF_ACTION:
        errors += local_action_errors(action, unguarded, root, privileged, artifacts_ok)
    return errors


def local_action_errors(action, unguarded, root, privileged, artifacts_ok):
    path = root / action / "action.yml"
    runs = yaml.load(path.read_text(), Loader)["runs"] if path.is_file() else {}
    if runs.get("using") != "composite":
        return [f"uses a local action this check cannot read ({action})"]
    return [f"{action}: {e}" for step in runs.get("steps") or [] for e in step_errors(step, unguarded, root, privileged, artifacts_ok)]


def read_strictly(text):
    """Fail on what GitHub may read differently from PyYAML: explicit tags and aliases."""
    for event in yaml.parse(text, Loader):
        if isinstance(event, yaml.AliasEvent) or getattr(event, "tag", None):
            raise ValueError("explicit tags and aliases are not supported")


def check(path):
    text = path.read_text()
    workflow = yaml.load(text, Loader)
    if any(isinstance(k, str) and k != "on" and k.casefold() == "on" for k in workflow):
        raise ValueError("an `on` key spelt in another case")
    privileged = [t for t in GUARDS if t in workflow["on"]]  # a string `on` matches by substring, which only adds triggers
    if not privileged:
        return []
    read_strictly(text)
    jobs = workflow["jobs"]
    root = path.resolve().parents[2]
    errors = []
    for name, job in jobs.items():
        if "uses" in job:
            errors.append(f"job `{name}` calls a reusable workflow, which this check cannot follow")
            continue
        unguarded = any(not guarded(name, jobs, t) for t in privileged)
        artifacts_ok = (path.name, name) in CROSS_RUN_ARTIFACTS
        for step in job.get("steps") or []:
            errors += [f"job `{name}` {e}" for e in step_errors(step, unguarded, root, privileged, artifacts_ok)]
    return errors


def main(argv):
    paths = [Path(a) for a in argv] or sorted((ROOT / ".github/workflows").glob("*.y*ml"))
    errors = []
    for path in paths:
        try:
            errors += [f"{path.name}: {e}" for e in check(path)]
        except Exception as exc:  # anything unreadable fails the check
            errors.append(f"{path.name}: cannot be checked: {exc!r}")
    if errors:
        print("\n".join(errors))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
