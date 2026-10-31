#!/usr/bin/env python3
"""Convert a merobox workflow (`backend: cloud-v2` YAML) into a cluster scenario (JSON),
the format `crates/node/tests/harness/scenario.rs` runs on an in-process cluster
(`cargo test -p calimero-node --test scenarios`; see
docs/src/content/docs/contribute/testing.mdx, "Scenarios").

The steps carry over as they are: the same step types and fields, `{{var}}`
placeholders, `outputs`, `expected_error` / `expected_failure` / `allow_failure`,
and `assert` / `json_assert` statements. What changes:

- `cluster.base_port` and `cluster.binary` are dropped: the runner picks free
  ports, and its nodes are built with every test feature (outbound-testing).
  `cluster.nodes` and `cluster.settings` stay.
- The comment block at the top of the file becomes `"comment"` (a list of lines);
  comments elsewhere in the YAML are lost.

Where it goes: `apps/<app>/workflows/<path>.yml` ->
`apps/<app>/scenarios/cluster/<path>.json`; `workflows/<dir>/<name>.yml` ->
`workflows/<dir>/scenarios/<name>.json`.

It refuses, naming why, a file it cannot convert faithfully: not a cloud-v2
workflow, a port marked `not_applicable`, an unknown top-level key, a step type
the runner does not run, and node process faults (kill/pause/resume/start/restart
a node, `ops`), which belong to the Python fault suite.

Deterministic and idempotent: the same YAML always gives the same bytes.

Usage: scripts/workflow-to-scenario.py [--delete] [--check] [--stdout] FILE.yml ...
  --delete  remove each converted YAML, and its `workflows/` directory once empty
  --check   write nothing; fail if a converted file is missing or differs
  --stdout  print the JSON instead of writing it
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[1]

#: Step types the runner runs (crates/node/tests/harness/scenario.rs, STEP_TYPES).
STEP_TYPES = {
    "deploy", "migration_status", "upload_bundle", "create_app", "install_package",
    "upgrade_app", "uninstall_package", "delete_app", "packages", "call", "view",
    "wait_for", "events", "whoami", "create_api_key", "create_org", "create_team",
    "invite", "redeem_invite", "remove_member", "team_add", "team_remove",
    "team_update", "team_get", "team_list", "team_join", "team_delete",
    "capabilities", "member_capabilities", "add_device", "set_role", "http",
    "upload_blob", "download_blob", "list_blobs", "blob_info", "delete_blob",
    "publish_presence", "presence", "transfer_app", "approve_release",
    "approve_outbound", "upgrade_org", "upgrade_run", "retry_upgrade_run",
    "find_leader", "wait", "set", "assert", "json_assert", "repeat", "fuzzy_test",
}  # fmt: skip

PROCESS_FAULTS = {"kill_node", "pause_node", "resume_node", "start_node", "restart_node", "ops"}

TOP_LEVEL = {"name", "description", "backend", "cluster", "accounts", "variables", "steps"}
CLUSTER_KEYS = {"nodes", "base_port", "binary", "settings"}


class Refused(Exception):
    """The workflow cannot be converted faithfully."""


def destination(path: Path) -> Path:
    rel = path.resolve().relative_to(ROOT)
    parts = rel.parts
    if len(parts) >= 4 and parts[0] == "apps" and parts[2] == "workflows":
        return ROOT.joinpath("apps", parts[1], "scenarios", "cluster", *parts[3:]).with_suffix(".json")
    if parts[0] == "workflows" and len(parts) >= 2:
        return ROOT.joinpath(*parts[:-1], "scenarios", parts[-1]).with_suffix(".json")
    raise Refused(f"{rel}: not under apps/<app>/workflows/ or workflows/")


def header_comment(text: str) -> list[str]:
    lines = []
    for line in text.splitlines():
        if line.startswith("#"):
            lines.append(line[1:][1:] if line[1:2] == " " else line[1:])
        elif line.strip():
            break
        elif lines:
            lines.append("")
    while lines and not lines[-1]:
        lines.pop()
    return lines


def check_steps(steps, where: str) -> None:
    if not isinstance(steps, list) or not steps:
        raise Refused(f"{where}: 'steps' must be a non-empty list")
    for i, step in enumerate(steps, start=1):
        label = f"{where} step {i}"
        if not isinstance(step, dict) or "type" not in step:
            raise Refused(f"{label}: each step needs a 'type'")
        kind = step["type"]
        if kind in PROCESS_FAULTS:
            raise Refused(
                f"{label}: '{kind}' is a node process fault; it runs against real "
                "processes in the Python fault suite, not in a cluster scenario"
            )
        if kind not in STEP_TYPES:
            raise Refused(f"{label}: step type '{kind}' has no counterpart in the runner")
        if kind == "repeat":
            check_steps(step.get("steps"), f"{label} (repeat)")
        if kind == "fuzzy_test":
            for j, op in enumerate(step.get("operations") or [], start=1):
                check_steps(op.get("steps"), f"{label} operation {j}")


def check_json(value, where: str) -> None:
    """Only what JSON holds: no dates, bytes or non-string keys from YAML."""
    if isinstance(value, dict):
        for k, v in value.items():
            if not isinstance(k, str):
                raise Refused(f"{where}: key {k!r} is not a string")
            check_json(v, f"{where}.{k}")
    elif isinstance(value, list):
        for i, v in enumerate(value):
            check_json(v, f"{where}[{i}]")
    elif value is not None and not isinstance(value, (str, int, float, bool)):
        raise Refused(f"{where}: {type(value).__name__} value {value!r} has no JSON form")


def convert(path: Path) -> str:
    text = path.read_text()
    wf = yaml.safe_load(text)
    if not isinstance(wf, dict):
        raise Refused("a workflow is a mapping")
    if wf.get("backend") != "cloud-v2":
        raise Refused("not a cloud-v2 workflow (expected 'backend: cloud-v2')")
    if wf.get("not_applicable"):
        raise Refused(f"marked not_applicable: {wf['not_applicable']}")
    unknown = sorted(set(wf) - TOP_LEVEL)
    if unknown:
        raise Refused(f"unknown top-level keys: {', '.join(unknown)}")
    cluster = wf.get("cluster") or {}
    if not isinstance(cluster, dict):
        raise Refused("'cluster' must be a mapping")
    extra = sorted(set(cluster) - CLUSTER_KEYS)
    if extra:
        raise Refused(f"unknown cluster keys: {', '.join(extra)}")
    if "settings" in cluster and "\n" not in str(cluster["settings"]):
        raise Refused("cluster.settings names a file; inline it so the scenario stands alone")
    check_steps(wf.get("steps"), "workflow")
    out: dict = {"name": wf.get("name") or path.stem}
    if wf.get("description"):
        out["description"] = wf["description"]
    comment = header_comment(text)
    if comment:
        out["comment"] = comment
    out["cluster"] = {"nodes": int(cluster.get("nodes", 3))}
    if "settings" in cluster:
        out["cluster"]["settings"] = cluster["settings"]
    out["accounts"] = list(wf.get("accounts") or [])
    if wf.get("variables"):
        out["variables"] = wf["variables"]
    out["steps"] = wf["steps"]
    check_json(out, "scenario")
    return json.dumps(out, indent=2, ensure_ascii=False) + "\n"


def main(argv: list[str]) -> int:
    flags = {a for a in argv if a.startswith("--")}
    files = [Path(a) for a in argv if not a.startswith("--")]
    unknown = flags - {"--delete", "--check", "--stdout"}
    if unknown or not files:
        print(__doc__, file=sys.stderr)
        return 2
    failed = 0
    for f in files:
        try:
            body = convert(f)
            dest = destination(f)
        except (Refused, OSError, yaml.YAMLError) as e:
            print(f"refused {f}: {e}", file=sys.stderr)
            failed += 1
            continue
        if "--stdout" in flags:
            sys.stdout.write(body)
            continue
        if "--check" in flags:
            if not dest.exists() or dest.read_text() != body:
                print(f"stale {dest.relative_to(ROOT)} (from {f})", file=sys.stderr)
                failed += 1
            continue
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(body)
        print(f"{f} -> {dest.relative_to(ROOT)}")
        if "--delete" in flags:
            f.unlink()
            d = f.resolve().parent
            # Empty directories up to (and including) the `workflows/` that held it.
            while d != ROOT and not any(d.iterdir()):
                name = d.name
                d.rmdir()
                if name == "workflows":
                    break
                d = d.parent
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
