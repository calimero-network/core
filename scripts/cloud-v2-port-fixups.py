#!/usr/bin/env python3
"""Fix workflows/cloud-v2/ ports where `merobox cloud port` mistranslates a
step, re-applied after each regeneration (scripts/port-workflows-to-cloud-v2.sh).
Each fix names the converter bug it works around; none weakens what the
scenario checks.

1. Files with `upgrade_group` or `get_migration_status` are re-ported with a
   Porter subclass that fixes two converter bugs:
   - cascade upgrades: v1's `upgrade_group` on a namespace (`cascade: true`)
     upgrades every context in the namespace's subgroups. The converter only
     looks for contexts created directly in the group named, so it drops the
     step ("no context to upgrade") and the port then reads the old release.
     Here each app deployed in the organization's teams is upgraded.
   - `get_migration_status`: v1's rollup counts members (each node migrated its
     own copy: `expected_members`, `total`, `migrated`, `reported_*`, ...). The
     converter keeps every v1 output although `migration_status` reports apps,
     so `equal({{mig_total}}, 2)` compares a member count with an app count, and
     `expected_members` is no field at all. Only `failed` means the same here
     (see MIGRATION_STATUS_SAME); the rest are dropped as the converter
     drops any v1-only output (the asserts on them go too, and the port is
     marked not_applicable when too few checks remain).
2. FIXUPS: small text edits to single ports, each with its reason.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "workflows/cloud-v2"
SOURCES = ROOT / "workflows"

#: Sources whose port the Porter below fixes.
NEEDS_FIX = re.compile(r"type: (upgrade_group|get_migration_status)\b")

#: (ported file relative to OUT, old text, new text, why)
FIXUPS: list[tuple[str, str, str, str]] = [
    # 00: v1 read the subgroup's upgrade record (get_group_upgrade_status:
    # status completed, toVersion 2.0.0). Its counterpart is the organization's
    # upgrade run (POST orgs/{org}/upgrade for a team, GET .../upgrades/{run}).
    (
        "app-migration/00-single-group-migration-baseline.yml",
        """- name: Upgrade with migrate_v1_to_v2
  type: upgrade_app
  app: '{{ctx_id}}'
  version: '{{app_v2_version}}'
  account: app-migration-baseline-1
""",
        """- name: Upgrade with migrate_v1_to_v2
  type: http
  method: post
  path: orgs/{{namespace_id}}/upgrade
  body:
    package: '{{app_v2}}'
    version: '{{app_v2_version}}'
    team: '{{group_id}}'
  account: app-migration-baseline-1
  outputs:
    up_run: runId
""",
        "converter bug: get_group_upgrade_status is unknown to it, so it drops the step but"
        " keeps the asserts on its outputs; the record is the org's upgrade run",
    ),
    (
        "app-migration/00-single-group-migration-baseline.yml",
        """- name: Assert the record names the v2 target
  type: json_assert
  statements:
  - equal("{{up_status}}", "completed")
  - equal("{{up_to_version}}", "2.0.0")
""",
        """- name: Read the subgroup's upgrade record
  type: http
  method: get
  path: orgs/{{namespace_id}}/upgrades/{{up_run}}
  account: app-migration-baseline-1
  outputs:
    up_record: .
    up_apps: apps
    up_done: done
- name: Assert the record names the v2 target
  type: json_assert
  statements:
  - 'json_subset({{up_record}}, {"version": "2.0.0", "finished": true, "failed": {}})'
  - json_equal({{up_done}}, {{up_apps}})
""",
        "same converter bug: v1's status completed + toVersion 2.0.0 is the run's finished,"
        " no failures, every covered app done, version 2.0.0",
    ),
    # 30: v1 ran the cascade by script (upgrade-group.sh), so the port never
    # upgraded and was marked not_applicable. The script's first check, the
    # trial refusing the lossy v2 because its migration_check fails, is the
    # release-v2 upgrade's own refusal; the port's later checks (the app
    # still serves v1, three items) then mean what they meant.
    (
        "app-migration/30-migration-check-fail-abort.yml",
        "- name: Wait for cascade op to gossip\n",
        """- name: Cascade to the lossy v2 is refused by the trial
  type: upgrade_app
  account: app-migration-check-fail-1
  app: '{{ctx_id}}'
  version: '{{app_v2_version}}'
  expected_failure: true
  outputs:
    trial_refusal: error
- name: The refusal is the migration check's
  type: assert
  statements:
  - contains("{{trial_refusal}}", "migration check refused")
- name: Wait for cascade op to gossip
""",
        "converter bug: an upgrade a script drove is dropped with the script",
    ),
    (
        "app-migration/30-migration-check-fail-abort.yml",
        "not_applicable: the port keeps 4 of v1's 14 checks; the rest checked what has no"
        " release-v2 counterpart\n",
        "# not_applicable lifted by scripts/cloud-v2-port-fixups.py: the trial refusal is restored\n",
        "the port tests its subject again once the scripted upgrade is restored",
    ),
    # 22: v1 checked the refusal's reason in the node log ("identity downgrade
    # forbidden") and that the migration never ran. Here the reason is in the
    # refusal itself, and the v1 entry still reading back means no migration
    # rewrote the state.
    (
        "app-migration/22-scenario-identity-downgrade.yml",
        """  account: app-migration-iddowngrade-1
  expected_failure: true
- name: Wait for log flush
  type: wait
  seconds: 5.0
""",
        """  account: app-migration-iddowngrade-1
  expected_failure: true
  outputs:
    downgrade_refusal: error
- name: The refusal is the identity-downgrade gate's
  type: assert
  statements:
  - contains("{{downgrade_refusal}}", "identity downgrade forbidden")
- name: The migration never ran (the v1 entry still reads back)
  type: call
  app: '{{ctx_id}}'
  method: get
  args:
    key: hello
  expect: world
  account: app-migration-iddowngrade-1
""",
        "converter bug: v1's log checks are dropped, though the refusal carries the reason",
    ),
    (
        "app-migration/22-scenario-identity-downgrade.yml",
        "not_applicable: the port keeps 1 of v1's 5 checks; the rest checked what has no"
        " release-v2 counterpart\n",
        "# not_applicable lifted by scripts/cloud-v2-port-fixups.py: the reason and the\n"
        "# unchanged state are checked again\n",
        "the port tests its subject again once the dropped checks are restored",
    ),
    (
        "app-migration/45-frontend-two-releases.yml",
        "\nbackend: cloud-v2\n",
        "\nbackend: cloud-v2\n"
        "not_applicable: 'the port keeps only \"both namespaces exist\": what v1 checked (each"
        " release serves its own frontend) is a script step, dropped. release-v2 serves a"
        " package frontend from its own origin ({label}.{apps_domain}, routed by Host), which"
        " no runner step can address'\n",
        "converter bug: its hollow check counts the kept is_set statements and misses that the"
        " frontend check (a script) was the scenario's whole subject",
    ),
]


#: The v1 get_migration_status field that means the same for an app's one
#: replicated state: how many failed. The rest count or list v1 members (each
#: migrated its own copy), `target_version` is v1's u32 ABI state version (the
#: runner reports the package semver), and the runner fixes `residue_total`
#: at 0, so a check on it would check nothing.
MIGRATION_STATUS_SAME = {"failed"}


def fixed_porter():
    from merobox.cloud_v2.port import Porter

    class CascadePorter(Porter):
        def port_step(self, s: dict) -> list:
            group = self._var(s.get("group_id"))
            if (
                s.get("type") == "upgrade_group"
                and group in self.orgs
                and not self.group_contexts.get(group)
            ):
                ctxs = [
                    c
                    for team, org in self.teams.items()
                    if org == group
                    for c in self.group_contexts.get(team, [])
                ]
                if ctxs:
                    self.group_contexts[group] = ctxs
                    try:
                        return super().port_step(s)
                    finally:
                        del self.group_contexts[group]
            if s.get("type") == "get_migration_status":
                return self._migration_status(s)
            return super().port_step(s)

        def _migration_status(self, s: dict) -> list:
            outs = s.get("outputs") or {}
            kept = {v: p for v, p in outs.items() if p in MIGRATION_STATUS_SAME}
            gone = {v for v in outs if v not in kept}
            if gone:
                self.dropped["get_migration_status output (v1 per-member rollup)"] += len(gone)
                self.dropped_vars.update(gone)
            steps = super().port_step({**s, "outputs": kept} if outs else s)
            for st in steps:
                if not kept:
                    st.pop("outputs", None)
            return steps

    return CascadePorter


def report_fixed(wasm_map: dict) -> int:
    porter_cls = fixed_porter()
    changed = 0
    for port in sorted(OUT.rglob("*.yml")):
        rel = port.relative_to(OUT)
        src = SOURCES / rel
        if not src.exists() or not NEEDS_FIX.search(src.read_text()):
            continue
        base_port = yaml.safe_load(port.read_text())["cluster"]["base_port"]
        wf = yaml.safe_load(src.read_text())
        porter = porter_cls(wf, wasm_map, base_port)
        body = yaml.safe_dump(porter.port(), sort_keys=False, allow_unicode=True, width=100)
        header = porter.header(src.name) + (
            "# FIXED by scripts/cloud-v2-port-fixups.py (converter bugs): a cascade\n"
            "# upgrade_group upgrades every app in the namespace's teams; of\n"
            "# get_migration_status only the outputs that mean the same here are kept.\n"
        )
        port.write_text(header + body)
        changed += 1
    return changed


def main() -> int:
    raw = subprocess.check_output(
        [sys.executable, str(ROOT / "scripts/cloud-v2-wasm-map.py")], text=True
    )
    wasm_map = json.loads(raw)
    n = report_fixed(wasm_map)
    print(f"re-ported {n} files")
    bad = 0
    for rel, old, new, why in FIXUPS:
        p = OUT / rel
        text = p.read_text()
        if new in text:  # applied already (new may contain old)
            continue
        if text.count(old) != 1:
            print(f"fixup no longer applies to {rel}: {why}", file=sys.stderr)
            bad += 1
            continue
        p.write_text(text.replace(old, new))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
