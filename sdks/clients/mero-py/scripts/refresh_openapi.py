#!/usr/bin/env python3
"""Vendor core's OpenAPI document into openapi/openapi.json.

The client's routes are generated from the document core publishes at
`docs/public/openapi.json` (core's `crates/node/src/api/openapi.rs` makes it,
from the registry its tests hold to the node's route table). This copies it,
byte for byte, from one core commit and records which in
openapi/source.json; `scripts/generate_openapi_client.py` then regenerates
the client from it.

    python scripts/refresh_openapi.py                       # core's master
    python scripts/refresh_openapi.py --ref v1.0.0          # a tag, branch or sha
    python scripts/refresh_openapi.py --commit <sha>        # that commit of master
    python scripts/refresh_openapi.py --core ../core        # a local checkout

With `--core`, the ref is fetched into that checkout from `origin` and read
with `git show`; otherwise the commit is resolved through GitHub's API and the
file downloaded from it (with `GITHUB_TOKEN`'s, if set, for the API's rate
limit). `--commit` reads that commit and records it as `--ref`'s, as
.github/workflows/openapi-sync.yml does when core announces a commit.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCUMENT = ROOT / "openapi" / "openapi.json"
SOURCE = ROOT / "openapi" / "source.json"
REPOSITORY = "calimero-network/core"
PATH = "docs/public/openapi.json"
DEFAULT_REF = "master"


def from_checkout(core: Path, ref: str):
    subprocess.run(
        ["git", "-C", str(core), "fetch", "--quiet", "origin", ref], check=True
    )
    commit = subprocess.run(
        ["git", "-C", str(core), "rev-parse", "FETCH_HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    data = subprocess.run(
        ["git", "-C", str(core), "show", f"{commit}:{PATH}"],
        check=True,
        capture_output=True,
    ).stdout
    return commit, data


def fetch(url: str, accept: str) -> bytes:
    headers = {"Accept": accept, "User-Agent": "calimero-client-py"}
    token = os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def from_github(ref: str):
    commit = fetch(
        f"https://api.github.com/repos/{REPOSITORY}/commits/{ref}",
        "application/vnd.github.sha",
    ).decode()
    data = fetch(
        f"https://raw.githubusercontent.com/{REPOSITORY}/{commit}/{PATH}",
        "application/json",
    )
    return commit, data


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--ref", default=DEFAULT_REF, help="core branch, tag or sha")
    parser.add_argument(
        "--commit",
        help="read this core commit (a full sha), recorded as --ref's",
    )
    parser.add_argument("--core", type=Path, help="a local core checkout to read")
    args = parser.parse_args()

    at = args.commit or args.ref
    if args.commit and len(args.commit) != 40:
        print("--commit takes a full 40-character sha", file=sys.stderr)
        return 1
    commit, data = from_checkout(args.core, at) if args.core else from_github(at)
    document = json.loads(data)
    if not str(document.get("openapi", "")).startswith("3."):
        print(f"{PATH} at {commit} is not an OpenAPI 3 document", file=sys.stderr)
        return 1

    DOCUMENT.write_bytes(data)
    SOURCE.write_text(
        json.dumps(
            {
                "repository": f"https://github.com/{REPOSITORY}",
                "ref": args.ref,
                "commit": commit,
                "path": PATH,
                "sha256": hashlib.sha256(data).hexdigest(),
            },
            indent=2,
        )
        + "\n"
    )
    print(f"openapi/openapi.json: {REPOSITORY}@{commit} ({args.ref})")
    print("now run: python scripts/generate_openapi_client.py")
    return 0


if __name__ == "__main__":
    sys.exit(main())
