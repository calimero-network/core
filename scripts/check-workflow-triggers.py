#!/usr/bin/env python3
"""A privileged workflow must not run PR code unguarded or write caches.

`pull_request_target` and `workflow_run` run with the base repository's token and
cache scope. Anything this script cannot judge fails. For each such workflow:

- a job must check out only the default ref (no `ref`, or `github.sha` or a base
  ref), unless its `if`, or that of a job it needs, implies that the head
  repository is this one;
- a job without that guard runs no `git fetch`, `gh pr checkout` or similar;
- an unguarded job downloads no other run's artifact and reads no `artifacts_url`;
- no job calls a reusable workflow, uses another checkout action, or saves a cache
  (`setup-rust-ci` and `rust-cache` need `save-if: false`, a `cache` input must be off).

Local composite actions are followed. The file is read as YAML 1.2 (as GitHub does) and
anything else it cannot be sure of, such as tags or aliases, fails.

A guard is a conjunct of the `if`: `||` branches must each imply it or be limited
to a different event, and a status function on a dependent drops an inherited one.

    python3 scripts/check-workflow-triggers.py [workflow.yml ...]
"""

import re
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
# The comparison a job's `if` must imply, per trigger (either operand order).
GUARDS = {
    "workflow_run": ("github.event.workflow_run.head_repository.full_name", "github.repository"),
    "pull_request_target": ("github.event.pull_request.head.repo.full_name", "github.repository"),
}
SAFE_REFS = {  # refs that name the base repository's own code
    "${{ github.sha }}",
    "${{ github.event.repository.default_branch }}",
    "${{ github.base_ref }}",
    "${{ github.event.pull_request.base.sha }}",
    "${{ github.event.pull_request.base.ref }}",
}
FETCHES_CODE = re.compile(
    r"\bgh\s+(?:pr\s+checkout|repo\s+clone|run\s+download)\b|\bgit\s+(?:fetch|checkout|switch|pull|clone|worktree|submodule)\b"
    r"|\b(?:curl|wget)\b|refs/pull/|\bpull/\d|/(?:tarball|zipball|archive)/",
    re.IGNORECASE,
)
STATUS_FUNCTION = re.compile(r"\b(?:always|failure|cancelled)\s*\(", re.IGNORECASE)  # runs even when a needed job is skipped
CACHE_INPUT = re.compile(r"cache|cache-to|(?!no-)[\w-]*-cache")  # inputs that turn on a cache write
CROSS_RUN_ARTIFACTS = {("comment.yml", "submit")}  # reads a PR run's payload as data and verifies it against the PR
MAX_ACTION_DEPTH = 5
EVENT_TEST = re.compile(r"^github\.event_name\s*(==|!=)\s*'([^']*)'$")
SAVE_IF_ACTIONS = ("./.github/actions/setup-rust-ci", "swatinem/rust-cache")  # write unless save-if is false
CACHE_READERS = ("actions/cache/restore",)


class Loader(yaml.SafeLoader):
    """SafeLoader reading YAML 1.2 booleans (on, yes and no stay strings) and rejecting duplicate and merge keys."""



def construct_mapping(loader, node, deep=False):
    seen = set()
    for key_node, _ in node.value:
        if key_node.tag == "tag:yaml.org,2002:merge":
            raise ValueError("merge keys (<<) are not understood by GitHub")
        key = loader.construct_object(key_node, deep=True)
        if key in seen:
            raise ValueError(f"duplicate key {key!r}")
        seen.add(key)
    return yaml.SafeLoader.construct_mapping(loader, node, deep)


Loader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, construct_mapping)
Loader.yaml_implicit_resolvers = {
    char: [(tag, regexp) for tag, regexp in resolvers if tag != "tag:yaml.org,2002:bool"]
    for char, resolvers in yaml.SafeLoader.yaml_implicit_resolvers.items()
}
Loader.add_implicit_resolver("tag:yaml.org,2002:bool", re.compile(r"^(?:true|True|TRUE|false|False|FALSE)$"), list("tTfF"))


def tokenize(text):
    tokens, buf, quote, calls, i = [], [], None, 0, 0

    def flush():
        operand = " ".join("".join(buf).split())
        buf.clear()
        if operand:
            tokens.append(operand)

    while i < len(text):
        c = text[i]
        if quote:
            quote = None if c == quote else quote
        elif c == "'":
            quote = c
        elif calls:
            calls += (c == "(") - (c == ")")
        elif c == "(" and buf and (buf[-1].isalnum() or buf[-1] in "_."):
            calls = 1
        elif c in "()" or text.startswith(("&&", "||"), i):
            operator = c if c in "()" else text[i : i + 2]
            flush()
            tokens.append(operator)
            i += len(operator)
            continue
        buf.append(c)
        i += 1
    if quote or calls:
        raise ValueError(f"unbalanced expression: {text}")
    flush()
    return tokens


def parse(tokens):
    """Expression to ('or'|'and', [nodes]) or ('atom', text); raises on anything unbalanced."""

    def disjunction(pos):
        nodes, pos = [], pos
        node, pos = conjunction(pos)
        nodes.append(node)
        while pos < len(tokens) and tokens[pos] == "||":
            node, pos = conjunction(pos + 1)
            nodes.append(node)
        return (nodes[0] if len(nodes) == 1 else ("or", nodes)), pos

    def conjunction(pos):
        nodes = []
        node, pos = primary(pos)
        nodes.append(node)
        while pos < len(tokens) and tokens[pos] == "&&":
            node, pos = primary(pos + 1)
            nodes.append(node)
        return (nodes[0] if len(nodes) == 1 else ("and", nodes)), pos

    def primary(pos):
        if pos >= len(tokens) or tokens[pos] in ("&&", "||", ")"):
            raise ValueError("malformed expression")
        if tokens[pos] == "(":
            node, pos = disjunction(pos + 1)
            if pos >= len(tokens) or tokens[pos] != ")":
                raise ValueError("unbalanced expression")
            return node, pos + 1
        return ("atom", tokens[pos]), pos + 1

    node, pos = disjunction(0)
    if pos != len(tokens):
        raise ValueError("malformed expression")
    return node


def expression(job):
    text = " ".join(str(job.get("if", "")).split())
    match = re.fullmatch(r"\$\{\{(.*)\}\}", text)
    if "${{" in text and not match:
        raise ValueError(f"`if` mixes text and an expression: {text}")
    return match.group(1).strip() if match else text


def other_event(atom, trigger):
    match = EVENT_TEST.match(atom)
    if not match:
        return False
    same = match.group(2).casefold() == trigger  # GitHub compares strings case-insensitively
    return not same if match.group(1) == "==" else same


def implies(node, trigger):
    kind, body = node
    if kind == "atom":
        left, right = (s.strip() for s in body.split("==", 1)) if "==" in body else ("", "")
        operands = {left.casefold(), right.casefold()}  # context names are case-insensitive
        return operands == set(GUARDS[trigger]) and body.count("==") == 1 and "!" not in body
    if kind == "and":
        return any(implies(n, trigger) for n in body)
    return all(implies(n, trigger) or limited_to_other_events(n, trigger) for n in body)


def limited_to_other_events(node, trigger):
    kind, body = node
    if kind == "atom":
        return other_event(body, trigger)
    if kind == "and":
        return any(limited_to_other_events(n, trigger) for n in body)
    return all(limited_to_other_events(n, trigger) for n in body)


def guarded(name, jobs, trigger, seen=()):
    job = jobs[name]
    text = expression(job)
    if text and implies(parse(tokenize(text)), trigger):
        return True
    if STATUS_FUNCTION.search(text):
        return False
    needs = job.get("needs") or []
    needs = [needs] if isinstance(needs, str) else needs
    return any(guarded(n, jobs, trigger, seen + (name,)) for n in needs if n in jobs and n not in seen)


def skips_privileged_events(step, privileged):
    text = expression(step)
    node = parse(tokenize(text)) if text else None
    return node is not None and all(limited_to_other_events(node, t) for t in privileged)


def step_errors(step, unguarded, root, privileged, artifacts_ok=False, depth=0):
    action = str(step.get("uses", "")).split("@")[0]
    lower = action.lower()
    with_ = step.get("with") or {}
    errors = []
    if action == "actions/checkout":
        ref, repo = str(with_.get("ref", "${{ github.sha }}")), str(with_.get("repository", "${{ github.repository }}"))
        if unguarded and (ref not in SAFE_REFS or repo != "${{ github.repository }}"):
            errors.append(f"checks out `{ref}` of `{repo}` without a same-repository guard")
    elif "checkout" in lower:
        errors.append(f"uses an unrecognised checkout action ({action})")
    if lower in SAVE_IF_ACTIONS and str(with_.get("save-if", "")).lower() != "false":
        errors.append(f"saves a cache ({action}); set `save-if: false`")
    elif "cache" in lower and lower not in SAVE_IF_ACTIONS + CACHE_READERS and not skips_privileged_events(step, privileged):
        errors.append(f"uses a cache action that is not read-only ({action})")
    errors += [f"turns on a cache write with `{k}: {v}`" for k, v in with_.items() if CACHE_INPUT.fullmatch(k) and str(v).lower() != "false"]
    if unguarded and FETCHES_CODE.search(str(step.get("run", ""))):
        errors.append("fetches code or artifacts in `run` without a same-repository guard")
    if unguarded and not artifacts_ok:
        if lower == "actions/download-artifact" and {"run-id", "repository", "github-token"} & set(with_):
            errors.append("downloads another run's artifact without a same-repository guard")
        if "artifacts_url" in str(step).lower():
            errors.append("reads `artifacts_url` without a same-repository guard")
    if action.startswith("./") and lower not in SAVE_IF_ACTIONS:
        errors += local_action_errors(action, unguarded, root, privileged, artifacts_ok, depth)
    return errors


def local_action_errors(action, unguarded, root, privileged, artifacts_ok, depth):
    found = [f for f in (root / action / "action.yml", root / action / "action.yaml") if f.is_file()]
    if not found or depth >= MAX_ACTION_DEPTH:
        return [f"uses a local action this check cannot read ({action})"]
    runs = (yaml.load(found[0].read_text(), Loader) or {}).get("runs") or {}
    steps = runs.get("steps") if runs.get("using") == "composite" else []
    return [f"{action}: {e}" for step in steps or [] for e in step_errors(step, unguarded, root, privileged, artifacts_ok, depth + 1)]


def read_strictly(text):
    """Fail on what GitHub may read differently from PyYAML: explicit tags and aliases."""
    for event in yaml.parse(text, Loader):
        if isinstance(event, yaml.AliasEvent) or getattr(event, "tag", None):
            raise ValueError("explicit tags and aliases are not supported")


def check(path):
    text = path.read_text()
    workflow = yaml.load(text, Loader)
    if not isinstance(workflow, dict):
        raise ValueError("not a workflow mapping")
    if any(isinstance(k, str) and k != "on" and k.casefold() == "on" for k in workflow):
        raise ValueError("an `on` key spelt in another case")
    if "on" not in workflow:
        raise ValueError("no `on` key")
    triggers = workflow["on"]
    if isinstance(triggers, str):
        triggers = [triggers]
    if not isinstance(triggers, (list, dict)):
        raise ValueError("`on` is neither a string, a list nor a mapping")
    privileged = [t for t in GUARDS if t in triggers]
    if not privileged:
        return []
    read_strictly(text)
    jobs = workflow.get("jobs")
    if not isinstance(jobs, dict):
        raise ValueError("no jobs mapping")
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
            errors.append(f"{path.name}: cannot be checked: {exc}")
    if errors:
        print("\n".join(errors))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
