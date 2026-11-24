#!/usr/bin/env python3
"""Fail when the release pinned in .github/upgrade-from has no replay fixture.

`upgrade_replay` replays every folder under crates/node/tests/fixtures/upgrade/, so a pin
moved to a release nobody recorded leaves that release's data unchecked. A pin that
declares a hard cutover names no release and needs no fixture.

    python3 scripts/check-replay-fixture.py
"""

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA_DIRS = ("datadir.tar.zst", "datadir.tar.gz")  # the data directory, as zstd now, gzip in the older fixtures
RECORD = "python3 -I crates/node/tests/fault/upgrade_fixture.py --from-dir target/upgrade-from --out crates/node/tests/fixtures/upgrade/{}"


def pinned(text):
    """The first line that is not a comment or blank, as upgrade-path.sh reads it."""
    for line in text.splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            return line
    return None


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--pin", type=Path, default=ROOT / ".github/upgrade-from")
    ap.add_argument("--fixtures", type=Path, default=ROOT / "crates/node/tests/fixtures/upgrade")
    args = ap.parse_args(argv)
    release = pinned(args.pin.read_text())
    if release is None or release.startswith("hard-cutover:"):
        return 0
    folder = args.fixtures / release
    if (folder / "expected.json").is_file() and any((folder / n).is_file() for n in DATA_DIRS):
        return 0
    print(
        f"::error::{args.pin.name} pins {release}, which has no replay fixture under crates/node/tests/fixtures/upgrade/.\n"
        f"Record it with that release's own binaries:\n"
        f"  scripts/fetch-release.sh {release} target/upgrade-from\n"
        f"  scripts/build-release-fixtures.sh {release} target/upgrade-from\n"
        f"  scripts/build-upgrade-deep-fixtures.sh target/upgrade-from\n"
        f"  {RECORD.format(release)}",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
