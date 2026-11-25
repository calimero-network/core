"""Deep cases: the operator's price table. It is set before the pipeline case stops the old job node
(`90_pipelines.py`): a price table needs every node of the cluster to answer."""

from __future__ import annotations

import json

from upgrade_deep import DeepCase, check, decision, eq, ops
from upgrade_path import case


@case
class PriceTable(DeepCase):
    name = "price-table"
    area = "Ops"
    seeds = "the operator's price table (`ops set-prices`)"
    verifies = ("`GET /ops/prices` on every node as set; a price table that is not the current version plus one "
                "is refused; on the new release the next version replaces it")

    @staticmethod
    def _table(version, scale):
        return {"version": version, "fuelPerVcpuSecond": 3 * scale, "vcpuSecond": 2 * scale, "gibSecond": 2 * scale,
                "connectorGib": scale, "storageGibMonth": scale, "egressGib": scale}

    def populate(self, w):
        table = self._table((ops(w, "GET", "ops/prices").get("version") or 0) + 1, 1)
        w.fleet.operate("set-prices", json.dumps(table))
        return {"table": table}

    def check(self, w, s):
        for n in w.members():
            eq(ops(w, "GET", "ops/prices", node=n), s["table"], f"the price table on node {n}")
        stale = self._table(s["table"]["version"], 9)
        got = decision(lambda: w.fleet.operate("set-prices", json.dumps(stale)))
        check(got != "ok", "a price table of the current version replaced the current one")

    def exercise(self, w, s):
        s["table"] = self._table(s["table"]["version"] + 1, 2)
        w.fleet.operate("set-prices", json.dumps(s["table"]))
        self.check(w, s)
