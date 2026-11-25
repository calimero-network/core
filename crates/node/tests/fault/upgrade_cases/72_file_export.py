"""Deep cases: a sealed file's key exported to its reader (`POST /apps/{app}/files/{id}/export`, which
`meroctl file download` calls: the key is wrapped to a key made on the client, every byte checked)."""

from __future__ import annotations

import hashlib
import random

from upgrade_deep import DeepCase, check, eq, kv_wasm, people, scratch
from upgrade_path import case


def _data(seed, size):
    return random.Random(seed).randbytes(size)


@case
class SealedFileExport(DeepCase):
    name = "sealed-file-export"
    area = "Blobs and files"
    seeds = "two sealed files of an app (70 KB and 300 KB), downloaded with `meroctl file download` on the old release"
    verifies = ("every file downloaded through each node, old and new, byte for byte (the node sends ciphertext and "
                "the key wrapped to the client's, which opens it and checks size and SHA-256); on the new release a "
                "file uploaded there downloads the same way")

    def _upload(self, w, s, seed, size):
        fenn = people(w, "fenn")[0]
        data = _data(seed, size)
        up = fenn.a.upload_file(s["app"], data)
        eq(up["sha256"], hashlib.sha256(data).hexdigest(), "the upload's digest")
        s["files"].append({"id": up["id"], "seed": seed, "size": size})

    def populate(self, w):
        fenn = people(w, "fenn")[0]
        s = {"app": fenn.deploy(kv_wasm(w), "exports"), "files": []}
        self._upload(w, s, 1, 70_000)
        self._upload(w, s, 2, 300_000)
        self.check(w, s)
        return s

    def check(self, w, s):
        fenn = people(w, "fenn")[0]
        out = scratch(w, "export")
        for n in w.members():
            for f in s["files"]:
                path = out / f"{f['id'][:12]}-{n}.bin"
                path.unlink(missing_ok=True)
                fenn.cli("file", "download", s["app"], f["id"], "--out", str(path), node=n)
                check(path.read_bytes() == _data(f["seed"], f["size"]), f"file {f['id']} through node {n} differs")

    def exercise(self, w, s):
        self._upload(w, s, 3, 120_000)
        self.check(w, s)
