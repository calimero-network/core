"""The CLI is deprecated: every run must say so on stderr and name the replacement."""

import subprocess
import sys
from pathlib import Path

EXAMPLE_ABI = Path(__file__).parent.parent / "examples" / "schemas" / "example_abi.json"


def test_cli_warns_on_stderr_and_names_replacement(tmp_path):
    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "calimero_abi_codegen.cli",
            "-i",
            str(EXAMPLE_ABI),
            "-o",
            str(tmp_path / "out"),
        ],
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert run.stderr.count("DeprecationWarning") == 1
    assert "meroctl app codegen --lang python" in run.stderr
