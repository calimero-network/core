"""Deep cases: pipeline runs of a compute-only package (fan-out, collect, receipts, the step cache)."""

from __future__ import annotations

import hashlib
import random

from upgrade_deep import DeepCase, Failed, check, eq, person, refused, scratch, until
from upgrade_path import OLD_JOB, case

PIPELINE = "fanout"
SIZES = (300_000, 120_000, 70_000)
LIVE_SIZES = (50_000, 40_000, 30_000)  # new content, so the step cache cannot answer the in-flight run
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
             "three sealed input files; a run to completion and a re-run of the same inputs; the cluster's job "
             "node stopped, then a run over three new files that waits unplaced across the roll")
    verifies = ("the finished run's state, step states, runs and outputs; its receipt verifies against its outputs; "
                "its outputs download byte for byte; the re-run's steps came from the step cache; a stranger is "
                "refused starting a run, the receipt and the events; the run left in flight is still running "
                "mid-roll, then ends succeeded on the new release with each item run once, its outputs byte for "
                "byte and its receipt verifying; on the new release a run over a new file completes and a re-run "
                "is still answered from the cache")

    def _begin(self, p, s, files):
        return p.cli("pipeline", "run", s["app"], PIPELINE, "--input", "docs=" + ",".join(s["files"][i]["id"] for i in files))["run"]

    def _upload(self, p, s, seed, size):
        up = p.a.upload_file(s["app"], _data(seed, size))
        s["files"].append({"id": up["id"], "size": size, "sha256": up["sha256"], "seed": seed})

    def _start(self, p, s, files):
        started = {"run": self._begin(p, s, files)}
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
            self._upload(pat, s, i, size)
        self._start(pat, s, [0, 1, 2])
        self._start(pat, s, [0, 1, 2])
        s["out"] = self._outputs(w, pat, s, s["runs"][0])
        for i, size in enumerate(LIVE_SIZES):
            self._upload(pat, s, 10 + i, size)
        # A run no job node answers for waits unplaced, spending no attempt: it is recorded on this release
        # and finished by the next once the new job node serves. The node stays registered (the pool counts it).
        w.fleet.kill(OLD_JOB)
        s["live"] = {"id": self._begin(pat, s, [3, 4, 5]), "files": [3, 4, 5]}
        return s

    def _outputs(self, w, pat, s, run):
        """The run's downloaded outputs by file name, each judged against the inputs; the collect output's digest."""
        out = scratch(w, "pipeline-" + run["id"])
        pat.cli("pipeline", "download", s["app"], run["id"], "--out", str(out))
        data = [_data(s["files"][i]["seed"], s["files"][i]["size"]) for i in run["files"]]
        want = {"each-0.bin": data[0] + data[1], "each-1.bin": data[2]}
        for name, bytes_ in want.items():
            check((out / name).read_bytes() == bytes_, f"{name} of run {run['id']} is not its inputs' bytes")
        collected = (out / "all-0.bin").read_bytes()
        eq(len(collected), sum(len(d) for d in data), f"all-0.bin of run {run['id']}")
        return hashlib.sha256(collected).hexdigest()

    def _check_live(self, w, pat, s):
        """No job node serves until the roll is done, so until then the run is still going; after it the
        new release finishes what the old one began: each item run once, outputs and receipt intact."""
        live = s["live"]
        if w.phase in ("seed", "mixed"):
            eq(pat.cli("pipeline", "get", s["app"], live["id"])["state"], "running", f"in-flight run {live['id']}")
            return
        if "summary" not in live:
            view = until("the in-flight run ends", lambda: _ended(pat.cli("pipeline", "get", s["app"], live["id"])), 240)
            eq(view["state"], "succeeded", f"in-flight run {live['id']}")
            each = view["steps"]["each"]["runs"]
            eq(len(each), 2, "the in-flight run's item runs")
            eq(len(set(each)), 2, "the in-flight run's item runs are distinct")
            live["summary"] = _summary(view)
            live["out"] = self._outputs(w, pat, s, live)
            got = pat.cli("pipeline", "receipt", s["app"], live["id"], "--verify")
            check(got["verified"] is True and len(got["outputs"]) == 3 and all(o["matches"] for o in got["outputs"]),
                  f"the receipt of the in-flight run {live['id']}: {got}")
        eq(_summary(pat.cli("pipeline", "get", s["app"], live["id"])), live["summary"], f"in-flight run {live['id']}")
        eq(self._outputs(w, pat, s, live), live["out"], "the in-flight run's collect output")

    def check(self, w, s):
        pat, zed = person(w, "pat"), person(w, "zed")
        self._check_live(w, pat, s)
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
        self._upload(pat, s, 20, 90_000)
        self._start(pat, s, [0, 1, 2])
        self._start(pat, s, [6])
        check(all(st["cached"] for st in s["runs"][2]["summary"]["steps"].values()), "the cache survived the roll")
        check(not any(st["cached"] for st in s["runs"][3]["summary"]["steps"].values()), "a new file ran its steps")
        got = pat.cli("pipeline", "receipt", s["app"], s["runs"][3]["id"], "--verify")
        check(got["verified"] is True, f"the receipt of the run on the new release: {got}")
