#!/usr/bin/env python3
"""Pack the e2e merobox scenarios into groups, one CI runner per group.

Every runner in the scenario job pays the same setup before its first scenario:
checkout, the merod image download and `docker load`, the app artifacts and
bundles, the fixture registry and merobox, about as long as a typical scenario.
A group pays it once and runs its scenarios one after another
(scripts/e2e-run-scenario-group.sh), and queues for one 8-cpu runner instead of
one per scenario.

The packing is a pure function of the manifest and the durations file, so a
given tree always produces the same groups:

- each image gets its own groups, because the image is loaded and stamped with
  the registry source once per runner: groups go to images one at a time, each
  to the image whose groups are currently the most loaded, until GROUP_COUNT
  are handed out;
- within an image, longest scenario first, each to the least-loaded group, so
  the long ones (tee-cards-*, fugue-collab-figure2, rich-catchup,
  sync-resilience-*) land in different groups;
- within a group, scenarios keep manifest order.

Durations are each scenario's median time over recent CI runs, in seconds, in
scripts/e2e-scenario-durations.json (each group's step summary lists what every
scenario took, for refreshing them); a scenario not in it is packed as
DEFAULT_SECONDS. They only steer the balance and the timeouts: a stale or
missing figure never drops or adds a scenario.

    e2e-scenario-groups.py [--manifest FILE] [--durations FILE]   # matrix JSON
    e2e-scenario-groups.py --summary                              # markdown table

The matrix is checked before it is printed: every manifest entry is in exactly
one group and each group holds one image, or this exits non-zero.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ".github/e2e-scenarios.yml"
DEFAULT_DURATIONS = "scripts/e2e-scenario-durations.json"

# Runners the scenario job takes per run. More groups shorten the longest one and
# pay setup more often; fewer save setup and lengthen it. At 20 the longest group
# runs about half a minute past the longest single scenario (tee-cards-late-tee,
# which bounds the job however many groups there are).
GROUP_COUNT = 20
# A scenario with no measured duration yet.
DEFAULT_SECONDS = 90
# A scenario is killed after FACTOR x its duration, never sooner than FLOOR, so a
# hung one fails alone and the rest of its group still runs.
SCENARIO_TIMEOUT_FACTOR = 3
SCENARIO_TIMEOUT_FLOOR_SECONDS = 300
# The job's own timeout adds this to the sum of its scenarios' timeouts: checkout,
# image load, bundles, merobox, cleanup between scenarios and the log upload.
GROUP_SETUP_ALLOWANCE_SECONDS = 900

DEFAULT_IMAGE = "merod:local"
# Image -> the prefix of its group names, in the order groups are listed.
IMAGES = {
    "merod:local": "local",
    "merod:local-dht": "dht",
    "merod:local-mock-tee": "mock-tee",
}
REQUIRED_FIELDS = ("workflow", "file", "app")
OPTIONAL_FIELDS = ("image", "registry_fetch")


class ManifestError(Exception):
    pass


def load_manifest(path: Path) -> list[dict]:
    """The manifest's scenarios, refusing anything this script does not understand.

    An unknown field is an error rather than ignored: a per-scenario setting the
    runner never reads would look applied and do nothing.
    """
    doc = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    if not isinstance(doc, dict) or set(doc) != {"scenarios"}:
        raise ManifestError(f"{path}: expected exactly one top-level key, `scenarios`")
    entries = doc["scenarios"]
    if not isinstance(entries, list) or not entries:
        raise ManifestError(f"{path}: `scenarios` must be a non-empty list")

    seen_names: set[str] = set()
    seen_files: set[str] = set()
    scenarios = []
    for index, entry in enumerate(entries):
        where = f"{path}: scenarios[{index}]"
        if not isinstance(entry, dict):
            raise ManifestError(f"{where}: not a mapping")
        unknown = set(entry) - set(REQUIRED_FIELDS) - set(OPTIONAL_FIELDS)
        if unknown:
            raise ManifestError(f"{where}: unknown field(s) {sorted(unknown)}")
        for field in REQUIRED_FIELDS:
            if not isinstance(entry.get(field), str) or not entry[field]:
                raise ManifestError(f"{where}: `{field}` must be a non-empty string")
        name = entry["workflow"]
        if name in seen_names:
            raise ManifestError(f"{where}: duplicate workflow name {name!r}")
        seen_names.add(name)
        rel = f"apps/{entry['app']}/{entry['file']}"
        if rel in seen_files:
            raise ManifestError(f"{where}: {rel} is already registered")
        seen_files.add(rel)
        image = entry.get("image", DEFAULT_IMAGE)
        if image not in IMAGES:
            raise ManifestError(f"{where}: unknown image {image!r} (known: {sorted(IMAGES)})")
        fetch = entry.get("registry_fetch", "true")
        if fetch not in ("true", "false"):
            raise ManifestError(f"{where}: registry_fetch must be the string \"false\" when set")
        scenarios.append({
            "workflow": name,
            "file": entry["file"],
            "app": entry["app"],
            "image": image,
            "registry_fetch": fetch,
        })
    return scenarios


def load_durations(path: Path) -> dict[str, int]:
    if not path.is_file():
        return {}
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict) or not all(
        isinstance(v, int) and not isinstance(v, bool) and v > 0 for v in data.values()
    ):
        raise ManifestError(f"{path}: expected {{scenario: positive integer seconds}}")
    return data


def scenario_timeout(seconds: int) -> int:
    return max(SCENARIO_TIMEOUT_FLOOR_SECONDS, SCENARIO_TIMEOUT_FACTOR * seconds)


def allocate(loads: dict[str, list[int]], count: int) -> dict[str, int]:
    """How many groups each image gets: one each, then each extra to the image
    whose groups would otherwise be the most loaded. Never more groups than an
    image has scenarios, and never fewer than one per image in use."""
    if count < len(loads):
        raise ManifestError(f"GROUP_COUNT {count} is below the {len(loads)} images in use")
    groups = {image: 1 for image in loads}
    for _ in range(count - len(loads)):
        open_images = [i for i in loads if groups[i] < len(loads[i])]
        if not open_images:
            break
        image = max(open_images, key=lambda i: (sum(loads[i]) / groups[i], -list(IMAGES).index(i)))
        groups[image] += 1
    return groups


def plan(scenarios: list[dict], durations: dict[str, int], count: int = GROUP_COUNT) -> list[dict]:
    order = {s["workflow"]: n for n, s in enumerate(scenarios)}
    estimate = {s["workflow"]: durations.get(s["workflow"], DEFAULT_SECONDS) for s in scenarios}

    by_image: dict[str, list[dict]] = {}
    for scenario in scenarios:
        by_image.setdefault(scenario["image"], []).append(scenario)
    by_image = {i: by_image[i] for i in IMAGES if i in by_image}
    shares = allocate({i: [estimate[s["workflow"]] for s in ss] for i, ss in by_image.items()}, count)

    groups = []
    for image, members in by_image.items():
        bins: list[list[dict]] = [[] for _ in range(shares[image])]
        load = [0] * len(bins)
        for scenario in sorted(members, key=lambda s: (-estimate[s["workflow"]], s["workflow"])):
            target = min(range(len(bins)), key=lambda b: (load[b], b))
            bins[target].append(scenario)
            load[target] += estimate[scenario["workflow"]]
        for number, members_of_bin in enumerate(bins, start=1):
            members_of_bin.sort(key=lambda s: order[s["workflow"]])
            runs = [
                dict(s, timeout_seconds=scenario_timeout(estimate[s["workflow"]]))
                for s in members_of_bin
            ]
            names = [s["workflow"] for s in runs]
            groups.append({
                "group": f"{IMAGES[image]}-{number:02d}",
                "image": image,
                "scenario_count": len(runs),
                "estimated_seconds": sum(estimate[n] for n in names),
                "timeout_minutes": math.ceil(
                    (GROUP_SETUP_ALLOWANCE_SECONDS + sum(r["timeout_seconds"] for r in runs)) / 60
                ),
                # The one-scenario setup steps, run only by a group that needs them.
                # ephemeral-* assert with Node 22's global WebSocket; the size sweep
                # needs its 416 MiB of random blob fixtures.
                "setup_node": any(n.startswith("ephemeral-") for n in names),
                "blob_fixtures": "blob-cross-node-sizes" in names,
                "scenarios": runs,
            })
    verify(scenarios, groups)
    return groups


def verify(scenarios: list[dict], groups: list[dict]) -> None:
    """Every scenario in exactly one group, each group on one image, none empty."""
    placed = [s["workflow"] for g in groups for s in g["scenarios"]]
    expected = [s["workflow"] for s in scenarios]
    duplicated = sorted({n for n in placed if placed.count(n) > 1})
    missing = sorted(set(expected) - set(placed))
    invented = sorted(set(placed) - set(expected))
    if duplicated or missing or invented:
        raise ManifestError(
            f"grouping lost coverage: missing={missing} duplicated={duplicated} unknown={invented}"
        )
    for group in groups:
        if not group["scenarios"]:
            raise ManifestError(f"{group['group']} is empty")
        if {s["image"] for s in group["scenarios"]} != {group["image"]}:
            raise ManifestError(f"{group['group']} mixes images")
    if len({g["group"] for g in groups}) != len(groups):
        raise ManifestError("two groups share a name")


def summary(groups: list[dict]) -> str:
    total = sum(g["estimated_seconds"] for g in groups)
    lines = [
        f"### E2E scenario groups: {len(groups)} groups, "
        f"{sum(g['scenario_count'] for g in groups)} scenarios, ~{total // 60} min of scenarios",
        "",
        "| group | scenarios | estimated | timeout | members |",
        "| --- | ---: | ---: | ---: | --- |",
    ]
    for g in groups:
        members = ", ".join(s["workflow"] for s in g["scenarios"])
        lines.append(
            f"| {g['group']} | {g['scenario_count']} | {g['estimated_seconds']}s "
            f"| {g['timeout_minutes']} min | {members} |"
        )
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--manifest", default=DEFAULT_MANIFEST)
    ap.add_argument("--durations", default=DEFAULT_DURATIONS)
    ap.add_argument("--summary", action="store_true", help="print a markdown table instead")
    args = ap.parse_args()

    try:
        scenarios = load_manifest(ROOT / args.manifest)
        durations = load_durations(ROOT / args.durations)
        groups = plan(scenarios, durations)
    except (ManifestError, OSError, yaml.YAMLError, json.JSONDecodeError) as err:
        print(f"::error::{err}", file=sys.stderr)
        return 1

    missing = [s["workflow"] for s in scenarios if s["workflow"] not in durations]
    if missing:
        print(
            f"::notice::no measured duration for {', '.join(missing)}; packed as {DEFAULT_SECONDS}s",
            file=sys.stderr,
        )
    if args.summary:
        print(summary(groups))
    else:
        print(json.dumps({"include": groups}, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
