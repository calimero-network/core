#!/usr/bin/env python3
"""Writes a kv package's component as several components of their own.

    python3 -I scripts/upgrade-dev-bundle.py kv-store-test-fixture.mpk OUT dev a b

For each name, `OUT/kv-<name>.wasm` is the package's `app.wasm` with one custom
section appended that names it, so each has a hash of its own and none is the
hash the package ships. The node shows a bundle hash that a package ships only to
whom may see that release (`SystemState::sees_bundle`), and takes the instances of
two packages that ship one component for the first package's: the deep upgrade
cases deploy `kv-dev.wasm` as every account's dev bundle and pack each of their
packages from a component of its own (scripts/build-upgrade-deep-fixtures.sh).
"""

import gzip
import io
import sys
import tarfile
from pathlib import Path


def leb128(n: int) -> bytes:
    out = bytearray()
    while True:
        b, n = n & 0x7F, n >> 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def main(mpk: str, out: str, *names: str) -> int:
    with tarfile.open(fileobj=io.BytesIO(gzip.decompress(Path(mpk).read_bytes()))) as tar:
        member = next(m for m in tar.getmembers() if m.name.lstrip("./") == "app.wasm")
        wasm = tar.extractfile(member).read()
    for name in names:
        section = b"calimero-upgrade-deep"
        body = leb128(len(section)) + section + name.encode()
        Path(out, f"kv-{name}.wasm").write_bytes(wasm + b"\x00" + leb128(len(body)) + body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(*sys.argv[1:]))
