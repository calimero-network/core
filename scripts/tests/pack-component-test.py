#!/usr/bin/env python3
"""pack-component.py declares and ships the compute component only when given one.

    python3 scripts/tests/pack-component-test.py
"""

import io
import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "pack-component.py"
COMPONENT = b"\0asm\x0d\0\x01\0"


def pack(tmp, *extra):
    app, out = Path(tmp) / "app.wasm", Path(tmp) / "out.mpk"
    app.write_bytes(COMPONENT + b"app")
    # `true` stands in for the signer: the script only reads the manifest back.
    subprocess.run([sys.executable, "-I", str(SCRIPT), "--wasm", str(app), "--package", "p", "--app-version", "1.0.0",
                    "--mero-sign", "true", "--out", str(out), *extra], check=True, capture_output=True)
    with tarfile.open(fileobj=io.BytesIO(out.read_bytes())) as tar:
        return {m.name: tar.extractfile(m).read() for m in tar.getmembers()}


class PackComponent(unittest.TestCase):
    def test_without_compute_the_package_has_no_compute_part(self):
        with tempfile.TemporaryDirectory() as tmp:
            files = pack(tmp)
        self.assertEqual(sorted(files), ["app.wasm", "manifest.json"])
        self.assertNotIn("compute", json.loads(files["manifest.json"]))

    def test_with_compute_the_manifest_declares_it_and_the_archive_carries_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            compute = Path(tmp) / "compute.component.wasm"
            compute.write_bytes(COMPONENT + b"compute")
            files = pack(tmp, "--compute", str(compute))
        self.assertEqual(sorted(files), ["app.wasm", "compute.wasm", "manifest.json"])
        declared = json.loads(files["manifest.json"])["compute"]
        self.assertEqual(declared["path"], "compute.wasm")
        self.assertEqual(declared["size"], len(files["compute.wasm"]))

    def test_a_compute_file_that_is_not_a_component_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "bad.wasm"
            bad.write_bytes(b"not wasm at all")
            with self.assertRaises(subprocess.CalledProcessError):
                pack(tmp, "--compute", str(bad))

    def test_previous_reaches_the_signer_with_the_component_beside_the_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            seen = Path(tmp) / "seen.json"
            signer = Path(tmp) / "signer.py"
            # Records its arguments and what sits beside the manifest it was given.
            signer.write_text(
                "import json, sys, pathlib\n"
                "manifest = pathlib.Path(sys.argv[2])\n"
                f"pathlib.Path({str(seen)!r}).write_text(json.dumps("
                "{'argv': sys.argv[1:], 'beside': sorted(p.name for p in manifest.parent.iterdir())}))\n"
            )
            previous = Path(tmp) / "old.mpk"
            previous.write_bytes(b"old")
            pack(tmp, "--mero-sign", f"{sys.executable} -I {signer}", "--previous", str(previous))
            got = json.loads(seen.read_text())
        self.assertEqual(got["argv"][0], "sign")
        self.assertEqual(got["argv"][-2:], ["--previous", str(previous.resolve())])
        self.assertEqual(got["beside"], ["app.wasm", "manifest.json"])


if __name__ == "__main__":
    unittest.main()
