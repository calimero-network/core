"""Deep cases: what a person does with a pipeline besides running it: estimate, retry, a compute run's logs.

Before `90_pipelines.py`: its seed stops the old job node, and these runs need one."""

from __future__ import annotations

import random

from upgrade_deep import DeepCase, Failed, check, eq, people, refused, until
from upgrade_path import case

PIPELINE = "fanout"


def _ended(view):
    if view["state"] == "running":
        raise Failed("the run is still going")
    return view


def _summary(view):
    return {"state": view["state"],
            "steps": {n: {k: s[k] for k in ("status", "runs", "cached")} | {"outputs": [o["id"] for o in s["outputs"]]}
                      for n, s in view["steps"].items()}}


def _get(p, app, run, node=None):
    return p.cli("cloud", "compute", "pipeline", "get", app, run, node=node)


@case
class PipelineControls(DeepCase):
    name = "pipeline-controls"
    area = "Pipelines"
    since = "1.0.0-rc.19"
    seeds = ("a pipeline instance with four sealed files: what a run would take (`pipeline estimate`, by file "
             "and by count), a run to completion, a retry of it answered from the step cache, and the compute "
             "run's logs")
    verifies = ("on every node: both runs' states, steps, runs, outputs and cache answers, the run list, the "
                "estimate and the logs answer, and a cancel of an ended run refused; on the new release a retry "
                "of the retry")

    @staticmethod
    def _estimate(p, app, node=None):
        return p.cli("cloud", "compute", "pipeline", "estimate", app, PIPELINE, "--count", "docs=4", node=node)

    def _record(self, p, s, run):
        view = until("the run ends", lambda: _ended(_get(p, s["app"], run)), 180)
        eq(view["state"], "succeeded", f"pipeline run {run}")
        s["runs"].append({"id": run, "summary": _summary(view)})

    def populate(self, w):
        p = people(w, "tavi")[0]
        package, version = w.registry.publish(w.fixtures["pipelines"])
        p.raw("post", "packages/install", {"package": package, "version": version})
        app = p.raw("post", "apps", {"name": "controls", "package": package, "version": version})["appId"]
        files = [p.a.upload_file(app, random.Random(7000 + i).randbytes(20_000))["id"] for i in range(4)]
        s = {"app": app, "files": files, "runs": []}
        by_file = p.cli("cloud", "compute", "pipeline", "estimate", app, PIPELINE, "--input", "docs=" + ",".join(files))
        s["estimate"] = self._estimate(p, app)
        eq(by_file, s["estimate"], "an estimate by file and by count")
        check(int(s["estimate"]["fuel"]) > 0, f"the estimate: {s['estimate']}")
        first = p.cli("cloud", "compute", "pipeline", "run", app, PIPELINE, "--input", "docs=" + ",".join(files))["run"]
        self._record(p, s, first)
        self._retry(p, s)
        s["logs"] = p.get(f"apps/{app}/runs/{first}.each.0/logs")
        return s

    def _retry(self, p, s):
        run = p.cli("cloud", "compute", "pipeline", "retry", s["app"], s["runs"][-1]["id"], "--wait")["id"]
        self._record(p, s, run)
        cached = [st["cached"] for st in s["runs"][-1]["summary"]["steps"].values()]
        check(all(cached), f"the retry was not answered from the step cache: {s['runs'][-1]['summary']}")

    def check(self, w, s):
        p = people(w, "tavi")[0]
        for n in w.members():
            for r in s["runs"]:
                eq(_summary(_get(p, s["app"], r["id"], n)), r["summary"], f"run {r['id']} on node {n}")
            listed = [r["id"] for r in p.cli("cloud", "compute", "pipeline", "runs", s["app"], node=n)["runs"]]
            eq(listed[:len(s["runs"])], [r["id"] for r in reversed(s["runs"])], f"the run list on node {n}")
            eq(self._estimate(p, s["app"], n), s["estimate"], f"the estimate on node {n}")
            eq(p.get(f"apps/{s['app']}/runs/{s['runs'][0]['id']}.each.0/logs", node=n), s["logs"],
               f"the compute run's logs on node {n}")
            refused("NOT_FOUND", lambda n=n: p.cli("cloud", "compute", "pipeline", "cancel", s["app"],
                                                    s["runs"][0]["id"], node=n), f"a cancel of an ended run on node {n}")

    def exercise(self, w, s):
        self._retry(people(w, "tavi")[0], s)
        self.check(w, s)
