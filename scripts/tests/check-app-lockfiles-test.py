#!/usr/bin/env python3
"""check-app-lockfiles.py must keep every app lockfile current and refuse a rewrite nothing required.

Each case builds a throwaway repository of path-only crates (no network): a base commit,
then the PR's change in the working tree, and runs the check against that base.

    python3 scripts/tests/check-app-lockfiles-test.py
"""

import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-app-lockfiles.py"
APP = "apps/a"
NESTED = "apps/fixtures/b"


def crate(name, deps=(), extra=""):
    lines = [f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n{extra}\n[dependencies]']
    lines += [f'{d} = {{ path = "{path}" }}' for d, path in deps]
    return "\n".join(lines) + "\n"


def app(name, depth, deps=("c",), extra=""):
    up = "../" * depth
    return crate(name, [(d, f"{up}crates/{d}") for d in deps], extra)


def write(repo, files):
    for rel, text in files.items():
        path = repo / rel
        if text is None:
            path.unlink()
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        if rel.endswith("Cargo.toml"):
            (path.parent / "src").mkdir(exist_ok=True)
            (path.parent / "src" / "lib.rs").touch()


def lock(repo, app_dir):
    subprocess.run(["cargo", "update", "-q", "--workspace", "--manifest-path", f"{app_dir}/Cargo.toml"], cwd=repo, check=True)


def as_v3(repo, app_dir):
    """The same resolve in an older lockfile format: still valid, yet a rewrite."""
    path = repo / app_dir / "Cargo.lock"
    path.write_text(path.read_text().replace("version = 4", "version = 3"))


def git(repo, *args):
    return subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True, text=True).stdout.strip()


BASE = {
    "crates/c/Cargo.toml": crate("c"),
    "crates/d/Cargo.toml": crate("d"),
    "crates/e/Cargo.toml": crate("e"),
    f"{APP}/Cargo.toml": app("a", 2),
    f"{NESTED}/Cargo.toml": app("b", 3),
}
C_NEEDS_D = {"crates/c/Cargo.toml": crate("c", [("d", "../d")])}


# (label, (base files, apps relocked at base), (head files, apps relocked in the PR, apps rewritten), exit, substrings)
CASES = [
    ("current lockfiles with no change pass", ({}, []), ({}, [], []), 0, []),
    ("an app source change passes", ({}, []), ({f"{APP}/src/extra.rs": "\n"}, [], []), 0, []),
    (
        "a crate manifest change that leaves app lockfiles stale fails",
        ({}, []),
        (C_NEEDS_D, [], []),
        1,
        [f"{APP}/Cargo.lock", f"cargo update --workspace --manifest-path {APP}/Cargo.toml",
         f"{NESTED}/Cargo.lock", f"cargo update --workspace --manifest-path {NESTED}/Cargo.toml"],
    ),
    ("the same change with every app lockfile updated passes", ({}, []), (C_NEEDS_D, [APP, NESTED], []), 0, []),
    (
        "updating one app lockfile still names the other",
        ({}, []),
        (C_NEEDS_D, [APP], []),
        1,
        [f"cargo update --workspace --manifest-path {NESTED}/Cargo.toml"],
    ),
    (
        "an app's own manifest change with its lockfile passes",
        ({}, []),
        ({f"{APP}/Cargo.toml": app("a", 2, ("c", "e"))}, [APP], []),
        0,
        [],
    ),
    ("catching up lockfiles that master left stale passes", (C_NEEDS_D, []), ({}, [APP, NESTED], []), 0, []),
    (
        "a rewrite of a lockfile that was already current fails",
        ({}, []),
        ({}, [], [APP]),
        1,
        [f"{APP}/Cargo.lock", f"git checkout origin/master -- {APP}/Cargo.lock"],
    ),
    (
        "a manifest change that leaves the lockfile current does not excuse its rewrite",
        ({}, []),
        ({"crates/c/Cargo.toml": crate("c", extra='description = "c"')}, [], [APP]),
        1,
        [f"git checkout origin/master -- {APP}/Cargo.lock"],
    ),
    ("a new app with its lockfile passes", ({}, []), ({"apps/n/Cargo.toml": app("n", 2)}, ["apps/n"], []), 0, []),
    ("a removed app passes", ({}, []), ({f"{APP}/Cargo.toml": None, f"{APP}/Cargo.lock": None}, [], []), 0, []),
]


def run(label, base_edit, head_edit, want, expect):
    base_files, base_relock = base_edit
    head_files, head_relock, rewrite = head_edit
    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)
        git(repo, "init", "-q")
        write(repo, BASE)
        for d in (APP, NESTED):
            lock(repo, d)
        write(repo, base_files)
        for d in base_relock:
            lock(repo, d)
        git(repo, "add", "-A")
        git(repo, "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "commit", "-qm", "base")
        base = git(repo, "rev-parse", "HEAD")
        write(repo, head_files)
        for d in head_relock:
            lock(repo, d)
        for d in rewrite:
            as_v3(repo, d)
        before = {p: p.read_bytes() for p in repo.glob("apps/**/Cargo.lock")}
        done = subprocess.run(
            [sys.executable, str(SCRIPT), "--base", base], cwd=repo, capture_output=True, text=True, stdin=subprocess.DEVNULL
        )
        out = done.stdout + done.stderr
        untouched = before == {p: p.read_bytes() for p in repo.glob("apps/**/Cargo.lock")}
    ok = done.returncode == want and all(s in out for s in expect) and untouched
    print(f"  {'ok  ' if ok else 'FAIL'}  {label}")
    if not ok:
        print(f"     exit {done.returncode}, want {want}; lockfiles untouched: {untouched}; output:\n{out}")
    return ok


def main():
    results = [run(*c) for c in CASES]
    failed = results.count(False)
    print(f"{len(results) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
