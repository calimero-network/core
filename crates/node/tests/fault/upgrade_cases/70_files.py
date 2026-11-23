"""Deep cases: sealed files an app wrote itself, an upload session left half done across the
roll, and a deleted blob (the representative set covers uploaded blobs and files)."""

from __future__ import annotations

import hashlib
import json
import random
import urllib.error
import urllib.parse
import urllib.request

from upgrade_deep import DeepCase, check, eq, fixture, kv_wasm, new_org, person, refused, scratch, until
from upgrade_path import case


def _data(u):
    return random.Random(u["seed"]).randbytes(u["size"])


def _put_part(w, p, app, u, k):
    """Part `k` of the upload session `u`, raw bytes, over `p`'s passkey session."""
    data = _data(u)[k * u["partBytes"]:(k + 1) * u["partBytes"]]
    node = p.order()[0]
    req = urllib.request.Request(w.fleet.url(node) + f"apps/{app}/files/uploads/{u['id']}/parts/{k}", data=data,
                                 method="PUT")
    req.add_header("authorization", f"Bearer {p.key}")
    req.add_header("content-type", "application/octet-stream")
    with urllib.request.urlopen(req, timeout=120) as r:
        r.read()


def _leader(w, app):
    leader = w.fleet.app_leader(app)
    check(leader, f"no leader of {app} yet")
    return leader


@case
class AppFiles(DeepCase):
    name = "sealed-files-sessions"
    area = "Blobs and files"
    seeds = ("files an app wrote itself, an upload session begun with one of its parts sent, a plain blob "
             "deleted (the representative set uploads files of several segments)")
    verifies = ("each app-written file's size and SHA-256 as the app reads it; the half-done session expects "
                "its next part while its leader (an old node) leads and answers `UPLOAD_UNKNOWN` once the app moved "
                "(documented: the client begins again); a session begun on the new release completes with "
                "the right digest; the deleted blob stays gone")

    def populate(self, w):
        fay = person(w, "fay")
        fapp = fay.deploy(fixture(w, "files.wasm"), "app-files")
        s = {"fapp": fapp, "written": {}}
        for label, size, seed in (("small", 70_000, 1), ("large", 2 * 1024 * 1024 + 5, 2)):
            out = fay.call(fapp, "putFile", {"len": str(size), "seed": str(seed)}, timeout=180)["result"]
            s["written"][label] = {"id": out["id"], "sha256": out["sha256"], "size": str(size)}
        kv = fay.deploy(kv_wasm(w), "uploads")
        s["kv"] = kv
        begun = fay.http("POST", f"apps/{kv}/files/uploads", {"size": 2 * 16 * 1024 * 1024 + 11})
        s["upload"] = {"id": begun["uploadId"], "partBytes": int(begun["partBytes"]), "parts": int(begun["parts"]),
                       "size": 2 * 16 * 1024 * 1024 + 11, "seed": 7, "done": [],
                       "leader": until("the app's leader", lambda: _leader(w, kv))}
        check(s["upload"]["parts"] >= 2, f"an upload session of several parts: {begun}")
        _put_part(w, fay, kv, s["upload"], 0)
        blob = scratch(w, "gone") / "gone.bin"
        blob.write_bytes(random.Random(3).randbytes(5000))
        up = fay.cli("blob", "upload", "--instance", kv, str(blob))
        s["gone"] = up.get("blobId") or up.get("id")
        fay.raw("delete", f"apps/{kv}/blobs/{s['gone']}")
        return s

    def check(self, w, s):
        fay = person(w, "fay")
        for label, f in s["written"].items():
            got = fay.result(s["fapp"], "blobInfo", {"id": f["id"]})
            eq((got or {}).get("size"), f["size"], f"the app's {label} file's size")
            eq(got["sha256"], f["sha256"], f"the app's {label} file's digest")
        # An upload session lives in its app leader's memory: a session whose leader
        # changed answers `404 UPLOAD_UNKNOWN` and the client begins again
        # (protocol/blobs). The leader that took the first part is an old node, so once
        # the app moved the session is gone; nothing of it was committed.
        u = s["upload"]
        status = lambda: fay.http("GET", f"apps/{s['kv']}/files/uploads/{u['id']}")  # noqa: E731
        if until("the app's leader", lambda: _leader(w, s["kv"])) == u["leader"]:
            eq(int(status()["next"]), 1, "the half-done upload's next part, under the leader that took it")
        else:
            refused("UPLOAD_UNKNOWN", status, "a half-done upload whose leader changed")
        for f in u["done"]:
            info = fay.http("GET", f"apps/{s['kv']}/files/{f['id']}")
            eq((int(info["size"]), info["sha256"]), (f["size"], f["sha256"]), "a file uploaded on the new release")
        refused("NOT_FOUND", lambda: fay.http("GET", f"apps/{s['kv']}/blobs/{s['gone']}/info"), "a deleted blob")

    def exercise(self, w, s):
        fay = person(w, "fay")
        old = s["upload"]
        begun = fay.http("POST", f"apps/{s['kv']}/files/uploads", {"size": old["size"]})
        u = {**old, "id": begun["uploadId"], "partBytes": int(begun["partBytes"]), "parts": int(begun["parts"])}
        for k in range(u["parts"]):
            _put_part(w, fay, s["kv"], u, k)
        done = fay.http("POST", f"apps/{s['kv']}/files/uploads/{u['id']}/complete", {})
        eq(done["sha256"], hashlib.sha256(_data(u)).hexdigest(), "an upload session on the new release")
        old["done"].append({"id": done["id"], "sha256": done["sha256"], "size": u["size"]})
        out = fay.call(s["fapp"], "putFile", {"len": str(1024 * 1024 + 1), "seed": "9"}, timeout=180)["result"]
        s["written"]["new"] = {"id": out["id"], "sha256": out["sha256"], "size": str(1024 * 1024 + 1)}
        self.check(w, s)


GRANT_TTL = 7 * 24 * 3600  # seconds: outlives the roll


def _download(w, app, blob, grant, node):
    """The status of `blob` fetched on `node` with `grant` and no session, and its bytes when served."""
    url = w.fleet.url(node) + f"apps/{app}/blobs/{blob}?grant={urllib.parse.quote(grant)}"
    try:
        with urllib.request.urlopen(url, timeout=60) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, b""


def _ref(v):
    return v if isinstance(v, str) else v["id"]


@case
class BlobGrants(DeepCase):
    name = "published-blob-grants"
    area = "Blobs and files"
    since = "1.0.0-rc.31"
    seeds = ("a compute output published as a plain blob; a download grant of it, the owner's revoke, and a grant "
             "made after the revoke")
    verifies = ("on every node the grant from before the revoke is refused `404` and the one after it serves the "
                "blob's bytes; on the new release a new grant serves, a second revoke ends every earlier grant "
                "and a grant made after it serves")

    def _grant(self, rey, s):
        return _ref(rey.call(s["app"], "share", {"id": s["blob"], "ttl": GRANT_TTL})["result"])

    def _judge(self, w, s):
        for node in w.members():
            for grant in s["revoked"]:
                eq(_download(w, s["app"], s["blob"], grant, node)[0], 404, f"a revoked grant on node {node}")
            status, data = _download(w, s["app"], s["blob"], s["live"], node)
            eq(status, 200, f"the live grant on node {node}")
            eq(hashlib.sha256(data).hexdigest(), s["sha256"], f"the blob through node {node}")

    def _revoke_and_grant(self, rey, s):
        rey.raw("post", f"apps/{s['app']}/blobs/{s['blob']}/revoke")
        s["revoked"].append(s["live"])
        s["live"] = self._grant(rey, s)

    def populate(self, w):
        rey = person(w, "rey")
        package, version = w.registry.publish(w.fixtures["compute_from"])
        rey.raw("post", "packages/install", {"package": package, "version": version})
        org = new_org(rey, "Grant Org")
        app = rey.raw("post", "apps", {"name": "grants", "package": package, "version": version, "org": org})["appId"]
        data = random.Random(11).randbytes(100_000)
        file_id = rey.a.upload_file(app, data)["id"]
        run = rey.call(app, "startDigest", {"inputs": [file_id]}, timeout=60)["result"]

        def finished():
            outcome = rey.result(app, "outcome", {"run": run})
            check(outcome, "the run has no outcome yet")
            return json.loads(outcome)

        outcome = until("the compute run ends", finished, 240)
        check(outcome["status"] == "done" and outcome["outputs"] == 1, f"the run: {outcome}")
        output = rey.get(f"apps/{app}/runs/{run}")["final"]["outputs"][0]
        blob = _ref(rey.call(app, "publish", {"output": output["id"]})["result"])
        s = {"app": app, "blob": blob, "revoked": []}
        s["live"] = self._grant(rey, s)
        status, bytes_ = _download(w, app, blob, s["live"], w.members()[0])
        eq(status, 200, "the first grant before any revoke")
        s["sha256"] = hashlib.sha256(bytes_).hexdigest()
        self._revoke_and_grant(rey, s)
        self._judge(w, s)
        return s

    def check(self, w, s):
        until("the grants hold on every node", lambda: self._judge(w, s), 30)

    def exercise(self, w, s):
        rey = person(w, "rey")
        s["live"] = self._grant(rey, s)
        self._judge(w, s)
        self._revoke_and_grant(rey, s)
        self._judge(w, s)
