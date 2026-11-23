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

BUNDLE = {"name": f.ASSET}
RELEASES = [  # newest first, as the API lists them
    {"tag_name": "1.0.0-rc.35", "published_at": "2026-11-23T08:38:09Z", "assets": []},
    {"tag_name": "1.0.0-rc.34", "published_at": "2026-11-21T23:49:57Z", "assets": [BUNDLE]},
    {"tag_name": "1.0.0-rc.33", "published_at": "2026-11-20T17:59:35Z", "assets": [BUNDLE]},
]


def fake_api(releases):
    def get(url, **_):
        if "/releases/tags/" in url:
            return json.dumps(next(r for r in releases if r["tag_name"] == url.rsplit("/", 1)[-1])).encode()
        return json.dumps(releases).encode()

    return get


def run_main(releases, *argv):
    """main() against a fake API: (exit code, stderr)."""
    f.get = fake_api(releases)
    sys.argv = ["fetch-release-app.py", "--out", tempfile.mkdtemp(), *argv]
    err, code = io.StringIO(), 0
    with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
        try:
            f.main()
        except SystemExit as exit_:
            code = exit_.code
    return code, err.getvalue()


def skips_release_still_uploading():
    f.get = fake_api(RELEASES)
    return f.release(None)["tag_name"] == "1.0.0-rc.34"


def takes_newest_once_uploaded():
    done = [dict(RELEASES[0], assets=[BUNDLE])] + RELEASES[1:]
    f.get = fake_api(done)
    return f.release(None)["tag_name"] == "1.0.0-rc.35"


def main_uses_the_complete_release():
    code, err = run_main(RELEASES)  # the fake bundle has no digest: stops there, naming rc.34
    return code == 1 and "release 1.0.0-rc.34: GitHub records no SHA-256" in err


def named_release_without_bundle():
    code, err = run_main(RELEASES, "--version", "1.0.0-rc.35")
    return code == 1 and "release 1.0.0-rc.35 has no kv-store-test-fixture.mpk" in err


def no_release_has_a_bundle():
    code, err = run_main([dict(r, assets=[]) for r in RELEASES])
    return code == 1 and "no published release has kv-store-test-fixture.mpk" in err


CASES = [
    ("a release still uploading is skipped", skips_release_still_uploading),
    ("the newest release is taken once its bundle is up", takes_newest_once_uploaded),
    ("main uses the newest release that has the bundle", main_uses_the_complete_release),
    ("a named release without the bundle fails by name", named_release_without_bundle),
    ("no release with the bundle fails by name", no_release_has_a_bundle),
]


def main():
    failed = 0
    for label, case in CASES:
        ok = case()
        print(f"  {'ok  ' if ok else 'FAIL'}  {label}")
        failed += not ok
    print(f"{len(CASES) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
