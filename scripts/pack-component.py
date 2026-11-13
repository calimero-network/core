#!/usr/bin/env python3
"""Deprecated: `meroctl app pack` writes and signs the `.mpk` now.

    meroctl app pack --wasm kv.wasm --package com.calimero.kv-store \
        --app-version 1.0.0 --out dist/com.calimero.kv-store-1.0.0.mpk

This script is kept for one release and runs that command with the same flags.
`--mero-sign` is ignored (meroctl signs itself); `MEROCTL` names the binary
(default: `cargo run -q -p meroctl --`). Without `--key` the development key
signs, which a public registry refuses.
"""

from __future__ import annotations

import os
import shlex
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main(argv: list[str]) -> int:
    args, skip = [], False
    for i, a in enumerate(argv):
        if skip:
            skip = False
            continue
        if a == "--mero-sign":
            skip = True
            continue
        if a.startswith("--mero-sign="):
            continue
        args.append(a)
    meroctl = shlex.split(os.environ.get("MEROCTL", "")) or ["cargo", "run", "-q", "-p", "meroctl", "--"]
    print("pack-component.py is deprecated: use `meroctl app pack`", file=sys.stderr)
    # Paths are resolved from the caller's directory; `cargo run` needs the workspace.
    cwd = None if os.environ.get("MEROCTL") else ROOT
    args = [str(Path(a).resolve()) if i > 0 and args[i - 1] in PATH_FLAGS else a for i, a in enumerate(args)]
    return subprocess.run(meroctl + ["app", "pack", *args], cwd=cwd).returncode


PATH_FLAGS = {"--wasm", "--compute", "--pipeline", "--frontend", "--out", "--key", "--previous"}

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
