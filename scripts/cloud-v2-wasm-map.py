#!/usr/bin/env python3
"""Map each v1 bundle path core's workflows install to what it holds.

`merobox cloud port --wasm-map` reads this to turn `install_application` into
the release-v2 step that installs the same thing:

- the migration fixtures (`workflows/app-migration/build-wasms.sh`: each pair
  or chain shares one `--package` and gets `--app-version N.0.0`, plus the
  no-ABI bundle and the frontend-release pair) carry `_mpk`, the signed `.mpk`:
  the port installs it by name from the cluster's registry, deploys a release
  of it and upgrades by version, as v1 upgraded a group to an application;
- the other apps (`kv-store`, `kv-store-with-handlers`, `scaffolding-e2e`) map
  to their WASM only, uploaded as a bundle.

`""` is each entry's WASM, `_package` v1's application identity (the same
across versions). Paths use `{{env.CORE_WASM_DIR}}` (`target/cloud-v2-wasm`,
from `scripts/build-cloud-v2-fixtures.py`).
"""

import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DIR = "{{env.CORE_WASM_DIR}}"
BUILD_WASMS = ROOT / "workflows/app-migration/build-wasms.sh"

#: Apps the workflows install as `dist/<package>-0.0.0.mpk`, uploaded as WASM.
PLAIN_APPS = ["kv-store", "kv-store-with-handlers", "scaffolding-e2e"]


def crate_of(app_dir: Path) -> str:
    return tomllib.loads((app_dir / "Cargo.toml").read_text())["package"]["name"]


def wasm(crate: str) -> str:
    return f"{DIR}/{crate.replace('-', '_')}.wasm"


def fixtures() -> list[dict]:
    """Every bundle build-wasms.sh makes: package, version, crate, dist name."""
    out = []
    suites = re.findall(r'^\s*"(apps/migrations/[^"]+)"', BUILD_WASMS.read_text(), re.M)
    for suite in suites:
        name = Path(suite).name
        base = re.sub(r"-v[0-9].*$", "", name)
        version = re.sub(r".*-v([0-9]+).*", r"\1", name)
        out.append(
            {
                "dir": suite,
                "crate": crate_of(ROOT / suite),
                "package": f"com.calimero.{base}",
                "version": f"{version}.0.0",
            }
        )
    out.append(
        {
            "dir": "apps/migrations/migration-suite-v2-add-field",
            "crate": "migration-suite-noabi",
            "package": "com.calimero.migration-suite-noabi",
            "version": "2.0.0",
            "no_abi": True,
        }
    )
    for v in (1, 2):
        out.append(
            {
                "dir": "apps/migrations/migration-suite-v1",
                "crate": f"frontend-release-v{v}",
                "package": "com.calimero.frontend-release",
                "version": f"{v}.0.0",
                "frontend_dir": f"apps/migrations/frontend-release/v{v}",
            }
        )
    for f in out:
        f["mpk"] = f"dist/{f['package']}-{f['version']}.mpk"
    return out


def main() -> None:
    out: dict[str, dict[str, str]] = {}
    for f in fixtures():
        out[f["mpk"]] = {
            "": wasm(f["crate"]),
            "_package": f["package"],
            "_mpk": f"{DIR}/{f['mpk']}",
        }
    for app in PLAIN_APPS:
        crate = crate_of(ROOT / "apps" / app)
        out[f"dist/com.calimero.{app}-0.0.0.mpk"] = {"": wasm(crate)}
    json.dump(out, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
