#!/usr/bin/env python3
"""Tests for fetch-release-app.py: which release it picks, and that it fails loudly.

A release is published before its assets finish uploading, so the pick must not follow
"newest" onto one that has no bundle yet. Each case pins one rule.

    python3 scripts/tests/fetch-release-app-test.py
"""

import contextlib
import importlib.util
import io
import json
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "fetch-release-app.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
spec = importlib.util.spec_from_file_location("fetch_release_app", SCRIPT)
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)

HEADER = "# The core release every cluster must be able to roll forward from.\n#\n# Keep it at the latest.\n"
BUNDLE = {"name": f.ASSET}
RELEASES = [  # newest first, as the API lists them
    {"tag_name": "1.0.0-rc.35", "published_at": "2026-11-23T08:38:09Z", "assets": []},
    {"tag_name": "1.0.0-rc.34", "published_at": "2026-11-21T23:49:57Z", "assets": [BUNDLE]},
    {"tag_name": "1.0.0-rc.33", "published_at": "2026-11-20T17:59:35Z", "assets": [BUNDLE]},
]


def pin_file(text):
    path = Path(tempfile.mkdtemp()) / "upgrade-from"
    path.write_text(text)
    return path


def fake_api(releases):
    def get(url, **_):
        if "/releases/tags/" in url:
            tag = url.rsplit("/", 1)[-1]
            return json.dumps(next(r for r in releases if r["tag_name"] == tag)).encode()
        return json.dumps(releases).encode()

    return get


def run_main(pin, releases):
    """main() against a fake API and pin file: (exit code, stderr, stdout)."""
    f.get, f.PIN = fake_api(releases), pin
    sys.argv = ["fetch-release-app.py", "--out", tempfile.mkdtemp()]
    err, out = io.StringIO(), io.StringIO()
    code = 0
    with contextlib.redirect_stderr(err), contextlib.redirect_stdout(out):
        try:
            f.main()
        except SystemExit as exit_:
            code = exit_.code
    return code, err.getvalue(), out.getvalue()


def picks(pin_text, releases=RELEASES):
    f.PIN, f.get = pin_file(pin_text), fake_api(releases)
    return f.release(f.pinned())["tag_name"]


CASES = [
    ("a pin under a comment header is read", lambda: picks(HEADER + "1.0.0-rc.34\n") == "1.0.0-rc.34"),
    ("a pin with a trailing comment word is read", lambda: picks("1.0.0-rc.34 the release\n") == "1.0.0-rc.34"),
    (
        "a release still uploading is skipped when nothing is pinned",
        lambda: picks("hard-cutover: 1 why\n") == "1.0.0-rc.34",
    ),
    ("a hard cutover pin is no pin", lambda: (f.PIN.write_text(HEADER + "hard-cutover: 1 why\n"), f.pinned())[1] is None),
]


def pinned_release_without_bundle():
    code, err, _ = run_main(pin_file(HEADER + "1.0.0-rc.35\n"), RELEASES)
    return code == 1 and "release 1.0.0-rc.35 has no kv-store-test-fixture.mpk" in err


def no_release_has_a_bundle():
    bare = [dict(r, assets=[]) for r in RELEASES]
    code, err, _ = run_main(pin_file("hard-cutover: 1 why\n"), bare)
    return code == 1 and "no published release has kv-store-test-fixture.mpk" in err


def pin_beats_newer_release():
    code, err, _ = run_main(pin_file(HEADER + "1.0.0-rc.34\n"), RELEASES)
    return code == 1 and "release 1.0.0-rc.34: GitHub records no SHA-256" in err  # stops past the pick


CASES += [
    ("a pinned release without the bundle fails by name", pinned_release_without_bundle),
    ("no release with the bundle fails by name", no_release_has_a_bundle),
    ("the pin is used though a newer release exists", pin_beats_newer_release),
]


def main():
    failed = 0
    for label, case in CASES:
        try:
            ok = case()
        except Exception as exc:
            ok = False
            print(f"     {type(exc).__name__}: {exc}")
        print(f"  {'ok  ' if ok else 'FAIL'}  {label}")
        failed += not ok
    print(f"{len(CASES) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
