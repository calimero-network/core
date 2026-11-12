"""Deep cases: pipeline runs of a compute-only package (fan-out, collect, receipts, the step cache)."""

from __future__ import annotations

import hashlib
import random

from upgrade_deep import DeepCase, Failed, check, eq, person, refused, scratch, until
from upgrade_path import case

PIPELINE = "fanout"
SIZES = (300_000, 120_000, 70_000)
REFUSED = ("FORBIDDEN", "NOT_FOUND")


def _data(seed, size):
    return random.Random(seed).randbytes(size)


def _ended(view):
    if view["state"] == "running":
        raise Failed("the run is still going")
    return view


def _summary(view):
    return {"state": view["state"],
            "steps": {n: {k: s[k] for k in ("status", "runs", "cached")} | {"outputs": [o["id"] for o in s["outputs"]]}
                      for n, s in view["steps"].items()}}


@case
class Pipelines(DeepCase):
    name = "pipeline-runs"
    area = "Pipelines"
    since = "1.0.0-rc.19"
    seeds = ("a compute-only package registering a fan-out (batch 2) and collect pipeline, installed as an app; "
             "three sealed input files; a run to completion and a re-run of the same inputs")
    verifies = ("the finished run's state, step states, runs and outputs; its receipt verifies against its outputs; "
                "its outputs download byte for byte; the re-run's steps came from the step cache; a stranger is "
                "refused starting a run, the receipt and the events; on the new release a run over a new file "
                "completes and a re-run is still answered from the cache")

    def _start(self, p, s, files):
        started = p.cli("pipeline", "run", s["app"], PIPELINE, "--input", "docs=" + ",".join(s["files"][i]["id"] for i in files))
        view = until("the pipeline run ends", lambda: _ended(p.cli("pipeline", "get", s["app"], started["run"])), 180)
        eq(view["state"], "succeeded", f"pipeline run {started['run']}")
        s["runs"].append({"id": started["run"], "files": files, "summary": _summary(view)})

    def populate(self, w):
        pat = person(w, "pat")
        package, version = w.registry.publish(w.fixtures["pipelines"])
        pat.raw("post", "packages/install", {"package": package, "version": version})
        app = pat.raw("post", "apps", {"name": "pipes", "package": package, "version": version})["appId"]
        s = {"app": app, "files": [], "runs": [], "out": {}}
        for i, size in enumerate(SIZES):
            up = pat.a.upload_file(app, _data(i, size))
            s["files"].append({"id": up["id"], "size": size, "sha256": up["sha256"]})
        self._start(pat, s, [0, 1, 2])
        self._start(pat, s, [0, 1, 2])
        s["out"] = self._outputs(w, pat, s, s["runs"][0])
        return s

    def _outputs(self, w, pat, s, run):
        """The run's downloaded outputs by file name, each judged against the inputs; the collect output's digest."""
        out = scratch(w, "pipeline-" + run["id"])
        pat.cli("pipeline", "download", s["app"], run["id"], "--out", str(out))
        want = {"each-0.bin": _data(0, SIZES[0]) + _data(1, SIZES[1]), "each-1.bin": _data(2, SIZES[2])}
        for name, data in want.items():
            check((out / name).read_bytes() == data, f"{name} of run {run['id']} is not its inputs' bytes")
        collected = (out / "all-0.bin").read_bytes()
        eq(len(collected), sum(SIZES), f"all-0.bin of run {run['id']}")
        return hashlib.sha256(collected).hexdigest()

    def check(self, w, s):
        pat, zed = person(w, "pat"), person(w, "zed")
        for r in s["runs"]:
            eq(_summary(pat.cli("pipeline", "get", s["app"], r["id"])), r["summary"], f"pipeline run {r['id']}")
        first, rerun = s["runs"][0], s["runs"][1]
        check(not any(st["cached"] for st in first["summary"]["steps"].values()), "the first run ran its steps")
        check(all(st["cached"] for st in rerun["summary"]["steps"].values()), f"the re-run answered from the cache: {rerun['summary']}")
        if w.phase == "mixed":  # the app's attested nodes are changing; the receipt is only fetched
            check(pat.cli("pipeline", "receipt", s["app"], first["id"]).get("signature"), "a signed receipt")
        else:
            got = pat.cli("pipeline", "receipt", s["app"], first["id"], "--verify")
            check(got["verified"] is True and len(got["outputs"]) == 3 and all(o["matches"] for o in got["outputs"]),
                  f"the receipt of {first['id']}: {got}")
        eq(self._outputs(w, pat, s, first), s["out"], "the collect step's output")
        ids = [f["id"] for f in s["files"][:3]]
        refused(REFUSED, lambda: zed.http("POST", f"apps/{s['app']}/pipelines/{PIPELINE}/runs",
                                          {"inputs": {"docs": ids}, "params": {}}), "a stranger's start")
        refused(REFUSED, lambda: zed.http("GET", f"apps/{s['app']}/pipeline-runs/{first['id']}/receipt"),
                "a stranger's receipt")
        refused(REFUSED, lambda: zed.http("GET", f"apps/{s['app']}/pipeline-runs/{first['id']}/events"),
                "a stranger's events")

    def exercise(self, w, s):
        pat = person(w, "pat")
        size = 90_000
        up = pat.a.upload_file(s["app"], _data(3, size))
        s["files"].append({"id": up["id"], "size": size, "sha256": up["sha256"]})
        self._start(pat, s, [0, 1, 2])
        self._start(pat, s, [3])
        check(all(st["cached"] for st in s["runs"][2]["summary"]["steps"].values()), "the cache survived the roll")
        check(not any(st["cached"] for st in s["runs"][3]["summary"]["steps"].values()), "a new file ran its steps")
        got = pat.cli("pipeline", "receipt", s["app"], s["runs"][3]["id"], "--verify")
        check(got["verified"] is True, f"the receipt of the run on the new release: {got}")
