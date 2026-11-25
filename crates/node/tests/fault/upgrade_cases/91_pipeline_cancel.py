"""Deep cases: a pipeline run cancelled.

After `90_pipelines.py`, whose seed stops the old job node: a run started now waits unplaced, so the cancel
reaches it before it ends, whatever the timing."""

from __future__ import annotations

import random

from upgrade_deep import DeepCase, eq, people, until
from upgrade_path import case

PIPELINE = "fanout"


def _get(p, app, run, node=None):
    return p.cli("cloud", "compute", "pipeline", "get", app, run, node=node)


def _cancelled(view):
    eq(view["state"], "cancelled", "the run's state")
    return {"state": view["state"], "steps": {n: s["status"] for n, s in view["steps"].items()}}


@case
class PipelineCancel(DeepCase):
    name = "pipeline-cancel"
    area = "Pipelines"
    since = "1.0.0-rc.19"
    seeds = "a pipeline run no job node answers for (the old one is stopped), cancelled (`pipeline cancel`)"
    verifies = ("the run `cancelled` with its steps' states on every node, across the roll that brings a job node "
                "back and after the restart")

    def populate(self, w):
        p = people(w, "tavo")[0]
        package, version = w.registry.publish(w.fixtures["pipelines"])
        p.raw("post", "packages/install", {"package": package, "version": version})
        app = p.raw("post", "apps", {"name": "cancelled", "package": package, "version": version})["appId"]
        files = [p.a.upload_file(app, random.Random(8000 + i).randbytes(20_000))["id"] for i in range(2)]
        run = p.cli("cloud", "compute", "pipeline", "run", app, PIPELINE, "--input", "docs=" + ",".join(files))["run"]
        p.cli("cloud", "compute", "pipeline", "cancel", app, run)
        view = until("the run is cancelled", lambda: (_cancelled(_get(p, app, run)), _get(p, app, run))[1], 60)
        return {"app": app, "run": run, "steps": _cancelled(view)["steps"]}

    def check(self, w, s):
        p = people(w, "tavo")[0]
        for n in w.members():
            eq(_cancelled(_get(p, s["app"], s["run"], n))["steps"], s["steps"], f"the cancelled run on node {n}")
