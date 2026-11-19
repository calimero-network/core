"""Deep cases: deployments (a deployment's secrets for the apps its offered releases use)."""

from __future__ import annotations

import gzip
import hashlib
import io
import json
import tarfile
from pathlib import Path

from upgrade_deep import DeepCase, check, eq, kv_wasm, people, person, refused, scratch, until
from upgrade_path import case

USED = "com.calimero.upgrade.used-secrets"
NOTES = "com.calimero.upgrade.uses-secrets"
HOST = "api.example.com"


def _leb128(n: int) -> bytes:
    out = bytearray()
    while True:
        b, n = n & 0x7F, n >> 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def _component(w, name: str) -> bytes:
    """The kv dev component with a custom section naming it: a hash of its own, as
    scripts/upgrade-dev-bundle.py makes one per package."""
    section = b"calimero-upgrade-deep"
    body = _leb128(len(section)) + section + name.encode()
    return kv_wasm(w).read_bytes() + b"\x00" + _leb128(len(body)) + body


def _pack(p, w, package: str, version: str, wasm: bytes, extra: dict) -> Path:
    """A `.mpk` of `wasm` whose manifest adds `extra` (`secrets`, `uses`), signed with the
    development key by the meroctl the case runs with (this tree's, after the roll)."""
    d = scratch(w, f"{package}-{version}")
    manifest = {
        "version": "1.0", "package": package, "appVersion": version, "minRuntimeVersion": "0.1.0",
        "wasm": {"path": "app.wasm", "hash": hashlib.sha256(wasm).hexdigest(), "size": len(wasm)},
        **extra,
    }
    (d / "manifest.json").write_text(json.dumps(manifest))
    p.cli("app", "sign", str(d / "manifest.json"), "--dev")
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name, data in (("manifest.json", (d / "manifest.json").read_bytes()), ("app.wasm", wasm)):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    out = d / f"{package}-{version}.mpk"
    out.write_bytes(gzip.compress(buf.getvalue()))
    return out


@case
class DeploymentUsedSecrets(DeepCase):
    name = "deployment-used-secrets"
    area = "Deployments"
    since = "1.0.0-rc.32"
    seeds = ("a deployment offering a release that uses (`uses`) another package, whose release declares a "
             "secret, and the deployment's value of that secret; the release's `uses` and the value wait for "
             "the cluster version that brings them, so they are made after the roll")
    verifies = ("the deployment's secrets as its manager reads them: the value's fingerprint, the release that "
                "declares it (`declaredBy`) and the offered app that uses it; an outsider is refused the list, "
                "and a name nothing it reaches declares is refused; a new value set on the new release")

    def populate(self, w):
        dex = person(w, "dex")
        used = _pack(dex, w, USED, "1.0.0", _component(w, "used-secrets"), {
            "outbound": [f"https://{HOST}/"],
            "secrets": {"NEAR_AI_API_KEY": {"hosts": [HOST], "required": True}},
        })
        notes = _pack(dex, w, NOTES, "1.0.0", _component(w, "uses-secrets"), {
            "uses": {"inference": {"package": USED, "range": "^1", "version": "1.0.0"}},
        })
        for mpk in (used, notes):
            package, version = w.registry.publish(mpk)
            dex.raw("post", "packages/install", {"package": package, "version": version})
        d = dex.raw("post", "deployments", {"name": "dex-labs", "owner": {"account": dex.id}})["id"]
        dex.raw("post", f"deployments/{d}/apps", {"app": "notes", "package": NOTES, "version": "1.0.0"})

        # A release installed before every node keeps `uses` records none: installing it
        # again once they do fills them in, and then the secret it reaches is taken.
        def set_secret():
            dex.raw("post", "packages/install", {"package": NOTES, "version": "1.0.0"})
            return dex.raw("put", f"deployments/{d}/secrets/NEAR_AI_API_KEY", {"value": "dex-key-1"})

        set_ = until("the cluster version takes a secret an offered release uses", set_secret, timeout=120)
        check("dex-key-1" not in json.dumps(set_), f"the answer carries the value: {set_}")
        return {"deployment": d, "fingerprint": set_["fingerprint"]}

    def check(self, w, s):
        dex, dot = people(w, "dex", "dot")
        d = s["deployment"]
        listed = dex.get(f"deployments/{d}/secrets")
        row = listed.get("secrets", {}).get("NEAR_AI_API_KEY", {})
        eq(row.get("set"), True, "the deployment holds the secret")
        eq(row.get("fingerprint"), s["fingerprint"], "the secret's fingerprint")
        eq(row.get("apps"), ["notes"], "the offered app that uses it")
        eq(row.get("declaredBy"), [{"package": USED, "version": "1.0.0"}], "the release that declares it")
        check("dex-key" not in json.dumps(listed), f"the list carries a value: {listed}")
        refused(("FORBIDDEN", "NOT_FOUND"), lambda: dot.get(f"deployments/{d}/secrets"),
                "an outsider reads the deployment's secrets")
        refused(("UNDECLARED_SECRET",), lambda: dex.raw("put", f"deployments/{d}/secrets/OTHER", {"value": "x"}),
                "a name nothing the deployment reaches declares")

    def exercise(self, w, s):
        dex = person(w, "dex")
        set_ = dex.raw("put", f"deployments/{s['deployment']}/secrets/NEAR_AI_API_KEY", {"value": "dex-key-2"})
        check(set_["fingerprint"] != s["fingerprint"], f"a new value has a new fingerprint: {set_}")
        s["fingerprint"] = set_["fingerprint"]
        self.check(w, s)
