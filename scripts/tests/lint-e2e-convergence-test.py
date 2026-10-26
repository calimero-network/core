#!/usr/bin/env python3
"""R3 (weak barrier) fires on a bare wait_for_sync and stays quiet with `expect`."""

import importlib.util
import pathlib
import tempfile

spec = importlib.util.spec_from_file_location(
    "lint", pathlib.Path(__file__).parent.parent / "lint-e2e-convergence.py"
)
lint = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lint)

SCENARIO = """
steps:
  - {{name: Write on node-1, type: call, node: n1, method: set}}
  - {{name: Settle, type: wait_for_sync, nodes: [n1, n2]{expect}}}
  - {{name: Read on node-2, type: call, node: n2, method: get}}
"""


def findings(expect: str) -> list[str]:
    with tempfile.NamedTemporaryFile("w", suffix=".yml") as f:
        f.write(SCENARIO.format(expect=expect))
        f.flush()
        return lint.lint(f.name)


bare = findings("")
assert len(bare) == 1 and "R3" in bare[0], bare
assert findings(", expect: {n2: x}") == []
print("ok")
