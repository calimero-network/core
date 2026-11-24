"""Unit tests of the upgrade-path runner itself, with no cluster:

    python3 -I crates/node/tests/fault/upgrade_runner_test.py
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import upgrade_path  # noqa: E402


class WaitForBuild(unittest.TestCase):
    def status(self, text=None):
        path = Path(tempfile.mkdtemp()) / "build.status"
        if text is not None:
            path.write_text(text)
        return path

    def test_a_build_that_exits_zero_lets_the_roll_go_on(self):
        upgrade_path.wait_for_build(self.status("0\n"), timeout=1)

    def test_a_failed_build_fails_the_run_with_its_exit_status(self):
        with self.assertRaisesRegex(upgrade_path.Failed, r"build failed \(exit 101\)"):
            upgrade_path.wait_for_build(self.status("101\n"), timeout=1)

    def test_a_build_that_never_ends_fails_the_run(self):
        with self.assertRaisesRegex(upgrade_path.Failed, "did not end within"):
            upgrade_path.wait_for_build(self.status(), timeout=0.1)


if __name__ == "__main__":
    unittest.main()
