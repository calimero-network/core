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


def pack(tmp, *extra, with_app=True):
    app, out = Path(tmp) / "app.wasm", Path(tmp) / "out.mpk"
    app.write_bytes(COMPONENT + b"app")
    wasm = ["--wasm", str(app)] if with_app else []
    # `true` stands in for the signer: the script only reads the manifest back.
    subprocess.run([sys.executable, "-I", str(SCRIPT), *wasm, "--package", "p", "--app-version", "1.0.0",
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

    def test_compute_without_wasm_packs_a_compute_only_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            compute = Path(tmp) / "compute.component.wasm"
            compute.write_bytes(COMPONENT + b"compute")
            files = pack(tmp, "--compute", str(compute), with_app=False)
        self.assertEqual(sorted(files), ["compute.wasm", "manifest.json"])
        manifest = json.loads(files["manifest.json"])
        self.assertNotIn("wasm", manifest)
        self.assertEqual(manifest["compute"]["size"], len(files["compute.wasm"]))

    def test_each_pipeline_is_declared_and_shipped_under_pipelines(self):
        with tempfile.TemporaryDirectory() as tmp:
            compute = Path(tmp) / "compute.component.wasm"
            compute.write_bytes(COMPONENT + b"compute")
            first, second = Path(tmp) / "first.yaml", Path(tmp) / "second.yaml"
            first.write_text("name: first\n")
            second.write_text("name: second\n")
            files = pack(tmp, "--compute", str(compute), "--pipeline", str(first), "--pipeline", str(second),
                         with_app=False)
        self.assertEqual(sorted(files), ["compute.wasm", "manifest.json", "pipelines/first.yaml",
                                         "pipelines/second.yaml"])
        declared = json.loads(files["manifest.json"])["pipelines"]
        self.assertEqual([d["path"] for d in declared], ["pipelines/first.yaml", "pipelines/second.yaml"])
        self.assertEqual(declared[0]["size"], len(files["pipelines/first.yaml"]))

    def test_without_pipelines_the_manifest_has_no_pipelines_field(self):
        with tempfile.TemporaryDirectory() as tmp:
            files = pack(tmp)
        self.assertNotIn("pipelines", json.loads(files["manifest.json"]))

    def test_two_pipeline_files_with_one_name_are_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            one, two = Path(tmp) / "a" / "p.yaml", Path(tmp) / "b" / "p.yaml"
            for f in (one, two):
                f.parent.mkdir()
                f.write_text("name: p\n")
            with self.assertRaises(subprocess.CalledProcessError):
                pack(tmp, "--pipeline", str(one), "--pipeline", str(two))

    def test_neither_wasm_nor_compute_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(subprocess.CalledProcessError):
                pack(tmp, with_app=False)

    def test_a_compute_file_that_is_not_a_component_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "bad.wasm"
            bad.write_bytes(b"not wasm at all")
            with self.assertRaises(subprocess.CalledProcessError):
                pack(tmp, "--compute", str(bad))


if __name__ == "__main__":
    unittest.main()
