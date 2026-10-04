#!/usr/bin/env python3
"""Tests for needs-windows-ci.py, against a throwaway git repo.

Both of the script's wrong answers are silent: `true` too often costs Windows
minutes, `false` too often lands a Windows-only break unseen until the nightly.

    python3 scripts/tests/needs-windows-ci-test.py
"""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "needs-windows-ci.py"


def git(repo, *args):
    subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)


def write(repo, path, text):
    target = Path(repo) / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text)


def verdict(repo, paths, base="base"):
    args = [sys.executable, str(SCRIPT)] + (["--base", base] if base else [])
    out = subprocess.run(
        args, cwd=repo, input="\n".join(paths) + "\n", capture_output=True, text=True, check=True
    )
    return out.stdout.strip()


def main():
    failures = []

    def expect(name, want, got):
        if want != got:
            failures.append(f"{name}: want {want}, got {got}")
        else:
            print(f"ok   {name}")

    with tempfile.TemporaryDirectory() as repo:
        env = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
        os.environ.update(env)
        git(repo, "init", "-q")
        write(repo, "crates/a/src/plain.rs", "fn a() {}\n")
        write(repo, "crates/a/src/os.rs", '#[cfg(windows)]\nfn w() {}\n')
        write(repo, "crates/a/src/gone.rs", 'use std::os::unix::fs::PermissionsExt;\n')
        write(repo, "crates/a/src/attr.rs", '#[cfg_attr(target_os = "windows", allow(dead_code))]\nfn x() {}\n')
        write(repo, "crates/a/src/macro.rs", 'fn y() -> bool { cfg!(unix) }\n')
        write(repo, "crates/a/src/was_os.rs", '#[cfg(unix)]\nfn u() {}\n')
        git(repo, "add", "-A")
        git(repo, "commit", "-qm", "base")
        git(repo, "tag", "base")

        # Head: os.rs keeps its cfg, gone.rs is deleted, plain.rs gains nothing OS-specific,
        # was_os.rs loses its cfg (the base side still counts), new.rs is added with one.
        write(repo, "crates/a/src/plain.rs", "fn a() { let _ = 1; }\n")
        Path(repo, "crates/a/src/gone.rs").unlink()
        write(repo, "crates/a/src/new.rs", "#[cfg(not(unix))]\nfn n() {}\n")
        write(repo, "crates/a/src/was_os.rs", "fn u() {}\n")
        git(repo, "add", "-A")
        git(repo, "commit", "-qm", "head")

        expect("plain Rust change skips Windows", "false", verdict(repo, ["crates/a/src/plain.rs"]))
        expect("docs-only change skips Windows", "false", verdict(repo, ["docs/x.md", "README.md"]))
        expect("cfg(windows) file runs Windows", "true", verdict(repo, ["crates/a/src/os.rs"]))
        expect("cfg_attr target_os runs Windows", "true", verdict(repo, ["crates/a/src/attr.rs"]))
        expect("cfg! macro runs Windows", "true", verdict(repo, ["crates/a/src/macro.rs"]))
        expect("new file with cfg(not(unix)) runs Windows", "true", verdict(repo, ["crates/a/src/new.rs"]))
        expect("removing the only cfg still runs Windows", "true", verdict(repo, ["crates/a/src/was_os.rs"]))
        expect("deleted file read from the base runs Windows", "true", verdict(repo, ["crates/a/src/gone.rs"]))
        expect("lockfile runs Windows", "true", verdict(repo, ["Cargo.lock"]))
        expect("nested manifest runs Windows", "true", verdict(repo, ["crates/a/Cargo.toml"]))
        expect("nested build script runs Windows", "true", verdict(repo, ["crates/server/build.rs"]))
        expect("its own workflow runs Windows", "true", verdict(repo, [".github/workflows/platforms.yml"]))
        expect("the scenario it runs runs Windows", "true", verdict(repo, ["apps/kv-store/workflows/workflow-example.yml"]))
        expect("one hit among many runs Windows", "true", verdict(repo, ["docs/x.md", "crates/a/src/plain.rs", "crates/a/src/os.rs"]))
        expect("unreadable Rust path fails closed", "true", verdict(repo, ["crates/a/src/never-existed.rs"]))
        expect("empty input fails closed", "true", verdict(repo, []))
        expect("unknown base fails closed", "true", verdict(repo, ["crates/a/src/plain.rs"], base="deadbeef"))

    if failures:
        print("\n".join(f"FAIL {f}" for f in failures))
        return 1
    print("all needs-windows-ci tests passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
