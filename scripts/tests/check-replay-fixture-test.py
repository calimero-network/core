#!/usr/bin/env python3
"""check-replay-fixture.py must fail on a pinned release with no fixture, naming how to record it.

    python3 scripts/tests/check-replay-fixture-test.py
"""

import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-replay-fixture.py"
HEADER = "# the pinned release\n\n"


def run(pin, held=(), partial=(), data="datadir.tar.gz"):
    """Exit code and stderr for a pin file's text and the fixture folders present."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / "upgrade-from").write_text(pin)
        fixtures = tmp / "fixtures"
        for rel in held:
            (fixtures / rel).mkdir(parents=True)
            (fixtures / rel / "expected.json").write_text("{}")
            (fixtures / rel / data).write_text("")
        for rel in partial:
            (fixtures / rel).mkdir(parents=True)
            (fixtures / rel / "expected.json").write_text("{}")
        fixtures.mkdir(exist_ok=True)
        done = subprocess.run(
            [sys.executable, str(SCRIPT), "--pin", str(tmp / "upgrade-from"), "--fixtures", str(fixtures)],
            capture_output=True, text=True,
        )
        return done.returncode, done.stderr


def check(name, ok):
    print(("ok   " if ok else "FAIL ") + name)
    return ok


code, err = run(HEADER + "1.0.0-rc.35\n")
results = [
    check("a pin with no folder fails", code == 1),
    check("the error names the release", "pins 1.0.0-rc.35" in err),
    check("the error gives the fetch command", "scripts/fetch-release.sh 1.0.0-rc.35 target/upgrade-from" in err),
    check(
        "the error gives the record command",
        "python3 -I crates/node/tests/fault/upgrade_fixture.py --from-dir target/upgrade-from "
        "--out crates/node/tests/fixtures/upgrade/1.0.0-rc.35" in err,
    ),
    check("a pin whose folder is held passes", run(HEADER + "1.0.0-rc.35\n", held=["1.0.0-rc.35"])[0] == 0),
    check("a zstd data directory counts", run(HEADER + "1.0.0-rc.35\n", held=["1.0.0-rc.35"], data="datadir.tar.zst")[0] == 0),
    check("the error gives the deep fixtures command", "scripts/build-upgrade-deep-fixtures.sh target/upgrade-from" in err),
    check("another release's folder does not count", run(HEADER + "1.0.0-rc.35\n", held=["1.0.0-rc.34"])[0] == 1),
    check("a folder without its data directory does not count", run(HEADER + "1.0.0-rc.35\n", partial=["1.0.0-rc.35"])[0] == 1),
    check("a hard cutover pins no release", run(HEADER + "hard-cutover: 1 why\n")[0] == 0),
]
sys.exit(0 if all(results) else 1)
