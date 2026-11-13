#!/usr/bin/env python3
"""Fetch the kv app bundle a published core release shipped, for running it on this tree's node.

Every release uploads `kv-store-test-fixture.mpk` (release.yml's `Release App Bundle`): the kv
fixture built with that release's SDK and WIT and signed with its own tools (mero-sign, or meroctl). This downloads it,
checks it against the SHA-256 GitHub records for the asset, and checks the component inside
against the hash its own manifest names, then writes into OUT:

    kv-store-test-fixture.mpk   the package, as published
    kv.wasm                     its component (the manifest's `wasm.path`)
    release.txt                 the release it came from

The release is `--version`, else the first word of `.github/upgrade-from` (the release the
upgrade-path job pins) when that file exists, else the newest published release. A token in
GH_TOKEN or GITHUB_TOKEN is used for the API when set.

    python3 -I scripts/fetch-release-app.py --out target/previous-release
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import sys
import tarfile
import urllib.request
from pathlib import Path

REPO = "calimero-network/core"
ASSET = "kv-store-test-fixture.mpk"
API = f"https://api.github.com/repos/{REPO}"
ROOT = Path(__file__).resolve().parent.parent
PIN = ROOT / ".github" / "upgrade-from"
VERSION_RE = re.compile(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?")
MAX_BUNDLE_BYTES = 64 * 1024 * 1024  # the fixture is about 0.2 MB packed
MAX_MEMBER_BYTES = 64 * 1024 * 1024


def fail(message: str) -> None:
    print(f"fetch-release-app: {message}", file=sys.stderr)
    sys.exit(1)


def get(url: str, accept: str = "application/vnd.github+json", limit: int = 8 * 1024 * 1024) -> bytes:
    request = urllib.request.Request(url, headers={"Accept": accept, "User-Agent": "fetch-release-app"})
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token and url.startswith(API):
        request.add_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(request, timeout=60) as response:
        body = response.read(limit + 1)
    if len(body) > limit:
        fail(f"{url}: more than {limit} bytes")
    return body


def pinned() -> str | None:
    """The release `.github/upgrade-from` pins, if the file exists and names one."""
    if not PIN.exists():
        return None
    words = PIN.read_text().split()
    return words[0] if words and VERSION_RE.fullmatch(words[0]) else None


def release(version: str | None) -> dict:
    if version:
        return json.loads(get(f"{API}/releases/tags/{version}"))
    published = [r for r in json.loads(get(f"{API}/releases?per_page=20")) if not r.get("draft")]
    if not published:
        fail("no published release")
    return max(published, key=lambda r: r.get("published_at") or "")


def member(archive: tarfile.TarFile, name: str) -> bytes:
    """One regular file of the archive, by its path (a leading `./` ignored); never extracted."""
    for info in archive.getmembers():
        if info.name.removeprefix("./") == name and info.isfile():
            if info.size > MAX_MEMBER_BYTES:
                fail(f"{name}: {info.size} bytes")
            handle = archive.extractfile(info)
            if handle is None:
                break
            return handle.read()
    fail(f"the bundle has no file {name}")
    raise AssertionError  # unreachable: fail exits


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--version", help="the release tag (default: the pin, else the newest release)")
    args = parser.parse_args()

    rel = release(args.version or pinned())
    tag = rel["tag_name"]
    asset = next((a for a in rel.get("assets", []) if a.get("name") == ASSET), None)
    if asset is None:
        fail(f"release {tag} has no {ASSET}")
    digest = asset.get("digest") or ""
    if not digest.startswith("sha256:"):
        fail(f"release {tag}: GitHub records no SHA-256 for {ASSET}")
    mpk = get(asset["browser_download_url"], accept="application/octet-stream", limit=MAX_BUNDLE_BYTES)
    if hashlib.sha256(mpk).hexdigest() != digest.removeprefix("sha256:"):
        fail(f"release {tag}: {ASSET} does not match its recorded {digest}")

    with tarfile.open(fileobj=io.BytesIO(mpk), mode="r:gz") as archive:
        manifest = json.loads(member(archive, "manifest.json"))
        wasm_ref = manifest.get("wasm") or {}
        wasm = member(archive, str(wasm_ref.get("path", "")))
    if hashlib.sha256(wasm).hexdigest() != wasm_ref.get("hash"):
        fail(f"release {tag}: the component does not match the hash its manifest names")

    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / ASSET).write_bytes(mpk)
    (args.out / "kv.wasm").write_bytes(wasm)
    (args.out / "release.txt").write_text(f"{tag}\n")
    print(f"{ASSET} of {tag} ({manifest.get('package')} {manifest.get('appVersion')}) in {args.out}")


if __name__ == "__main__":
    main()
