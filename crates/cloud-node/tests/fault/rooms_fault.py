"""Randomized fault test of Rooms on a 3-process merod cluster (acceptance criterion A4).

Adapted from the release-v2 review's tests/raft/jepsen.py (findings/area1-2-raft.md), with
Rooms (apps/rooms) as the workload instead of the nested-crdt-test app.

Workload, per iteration (room r<iteration>): workers book unique one-slot bookings, each with
a client idempotency key, retried with the SAME key on an unknown outcome; and read
`availability` strongly over a 24 h window. Nemesis (as in the review): SIGKILL the app
leader, SIGKILL a follower, SIGSTOP the leader, SIGSTOP a follower, or kill the leader then
freeze the new one.

Checks per iteration:
  - every acknowledged booking holds its slot, under its own id, exactly once at the end
  - no slot is held by two bookings (no duplicate slot in a read, no `Taken` answer: each slot
    is booked by one operation only, so `Taken` means that operation ran twice)
  - held slots are bounded: acked <= held <= acked + unknown, and every held slot was attempted
  - each slot, as a register (free / held), is linearizable over the strong reads (WGL)
  - the same key replayed at another node answers with the same booking id and index
Across all iterations: every acknowledged booking id is distinct (D52: `b<term>.<seq>.<n>`
must not repeat across leader kills).

Usage: python3 -I rooms_fault.py --iterations 200 --seed 1 [--duration 6] [--wasm PATH]
The Rooms component is the cached build of `cargo test --release -p calimero-cloud-node
--test rooms` (target/app-components/rooms-*.wasm) unless --wasm names one.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import shutil
import sys
import threading
import time
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from linearizable import check_register  # noqa: E402
from rv2 import ORIGIN, ROOT, Client, Cluster, HttpError  # noqa: E402

DEFINITE_FAIL = {400, 401, 403, 404, 409, 413, 422, 429}
SLOT_MS = 900_000
WINDOW = 96  # slots in availability's 24 h window
HELD = "held"


def classify(e):
    if isinstance(e, HttpError) and e.status in DEFINITE_FAIL:
        return "fail"
    return "info"  # 5xx, timeout, connection refused or reset: unknown outcome


def app_code(e):
    if isinstance(e, HttpError) and isinstance(e.body, dict):
        return (e.body.get("details") or {}).get("code")
    return None


def default_wasm():
    found = sorted((ROOT / "target" / "app-components").glob("rooms-*.wasm"),
                   key=lambda p: p.stat().st_mtime)
    if not found:
        raise SystemExit("no Rooms component: run cargo test --release -p calimero-cloud-node --test rooms")
    return str(found[-1])


def setup(cl: Cluster, wasm: str):
    """Owner signs up, creates an org, deploys Rooms in it, and mints an API key."""
    (cl.workdir / "cfg").mkdir(parents=True, exist_ok=True)
    pk = cl.workdir / "dev-passkey.json"
    cl.cli(1, "login", "--dev-passkey", str(pk), "--name", "owner", "--origin", ORIGIN)
    org = cl.cli(1, "namespace", "create", "Acme")["orgId"]
    app = cl.cli(1, "context", "create", "--name", "rooms", "--path", wasm, "--namespace", org)["appId"]
    key = cl.cli(1, "account", "api-key", "create", "--label", "rooms-fault")["key"]
    return app, key


class Iteration:
    def __init__(self, cl, client, app, it, seed, duration, workers, base):
        self.c = cl
        self.client = client
        self.app = app
        self.it = it
        self.rng = random.Random(seed * 100_003 + it)
        self.duration = duration
        self.workers = workers
        self.room = f"r{it}"
        self.base = base  # first slot start of this iteration's room
        self.next_slot = 0
        self.history = []
        self.lock = threading.Lock()
        self.t0 = time.time()
        self.stop = threading.Event()
        self.nemesis_log = []
        self.idem_probe = []
        self.frozen = False  # set once the checker takes the history: later records are dropped

    def now(self):
        return time.time() - self.t0

    def record(self, rec):
        with self.lock:
            if not self.frozen:
                self.history.append(rec)

    def probe(self, rec):
        with self.lock:
            if not self.frozen:
                self.idem_probe.append(rec)

    def take_slot(self):
        with self.lock:
            n = self.next_slot
            self.next_slot += 1
            return n

    def node(self, rng):
        return rng.choice(self.c.running() or self.c.ids)

    def start_of(self, n):
        return self.base + n * SLOT_MS

    def book_args(self, n):
        return {"roomId": self.room, "start": str(self.start_of(n)), "slotCount": 1, "title": f"s{n}"}

    def keyed_book(self, rng, n, retries=2):
        """`book` with a client idempotency key, retried with the same key on an unknown outcome."""
        idem = uuid.UUID(int=rng.getrandbits(128)).hex
        attempts = []
        for _ in range(retries + 1):
            node = self.node(rng)
            try:
                r = self.client.call(node, "book", self.book_args(n), idem=idem, timeout=20)
                attempts.append((node, "ok", r.get("index")))
                return "ok", r, attempts, idem, None
            except Exception as e:
                kind = classify(e)
                attempts.append((node, kind, str(e)[:200]))
                if kind == "fail":
                    return "fail", None, attempts, idem, app_code(e)
        return "info", None, attempts, idem, None

    def read_window(self, node, w, timeout=20):
        day = self.start_of(w * WINDOW)
        r = self.client.view(node, "availability", {"roomId": self.room, "dayStart": str(day)}, timeout=timeout)
        return r["result"], r.get("index")

    def worker(self, wid):
        rng = random.Random(self.rng.getrandbits(64))
        while not self.stop.is_set():
            t_inv = self.now()
            if rng.random() < 0.6:
                n = self.take_slot()
                st, res, att, idem, code = self.keyed_book(rng, n)
                self.record({"p": wid, "f": "book", "slot": n, "type": st, "code": code,
                             "id": res.get("result") if res else None,
                             "index": res.get("index") if res else None,
                             "invoke": t_inv, "complete": self.now(), "attempts": att, "idem": idem})
                # idempotency probe: replay the same key at another node; same answer expected
                if st == "ok" and rng.random() < 0.25:
                    others = [i for i in self.c.running() if i != att[-1][0]]
                    if others:
                        n2 = rng.choice(others)
                        try:
                            r2 = self.client.call(n2, "book", self.book_args(n), idem=idem, timeout=20)
                            first = [res.get("index"), res.get("result")]
                            replay = [r2.get("index"), r2.get("result")]
                            self.probe({"idem": idem, "first": first, "replay": replay,
                                        "node": n2, "same": first == replay})
                        except Exception as e:
                            self.probe({"idem": idem, "error": str(e)[:200], "code": app_code(e)})
            else:
                with self.lock:
                    used = self.next_slot
                if used == 0:
                    time.sleep(0.01)
                    continue
                w = rng.randrange(0, (used - 1) // WINDOW + 1)
                node = self.node(rng)
                try:
                    slots, index = self.read_window(node, w)
                    self.record({"p": wid, "f": "read", "window": w, "type": "ok", "slots": slots,
                                 "index": index, "node": node, "invoke": t_inv, "complete": self.now()})
                except Exception as e:
                    self.record({"p": wid, "f": "read", "window": w, "type": "fail",
                                 "invoke": t_inv, "complete": self.now(), "err": str(e)[:200]})

    def nemesis(self):
        rng = self.rng
        time.sleep(rng.uniform(0.3, 1.5))
        kind = rng.choice(["kill_leader", "kill_follower", "pause_leader", "pause_follower",
                           "kill_leader", "pause_leader", "kill_then_pause"])
        leader = self.c.app_leader(self.app)
        followers = [i for i in self.c.running() if i != leader]

        def ev(**k):
            k["t"] = round(self.now(), 3)
            self.nemesis_log.append(k)

        ev(kind=kind, leader=leader)
        if kind in ("kill_leader", "kill_then_pause") and leader:
            self.c.kill(leader); ev(op="kill", node=leader)
        elif kind == "kill_follower" and followers:
            n = rng.choice(followers); self.c.kill(n); ev(op="kill", node=n)
        elif kind == "pause_leader" and leader:
            self.c.pause(leader); ev(op="pause", node=leader)
        elif kind == "pause_follower" and followers:
            n = rng.choice(followers); self.c.pause(n); ev(op="pause", node=n)
        if kind == "kill_then_pause":
            time.sleep(rng.uniform(2.5, 4.0))
            for i in self.c.ids:
                if self.c.procs.get(i) is None:
                    self.c.start_node(i, wait=False); ev(op="start", node=i)
            time.sleep(1.0)
            nl = None
            deadline = time.time() + 5.0
            while nl is None and time.time() < deadline:
                nl = self.c.app_leader(self.app)
                if nl is None:
                    time.sleep(0.2)
            if nl is None:
                ev(op="no_leader", why="no new leader within 5 s; nothing frozen")
            if nl:
                self.c.pause(nl); ev(op="pause", node=nl, why="new leader")
        time.sleep(max(0.5, self.duration - self.now() - 1.0))
        for i in list(self.c.paused):
            self.c.resume(i); ev(op="resume", node=i)
        for i in self.c.ids:
            if self.c.procs.get(i) is None:
                self.c.start_node(i, wait=False); ev(op="start", node=i)

    def heal_wait(self, timeout=60):
        deadline = time.time() + timeout
        for i in self.c.ids:
            self.c.wait_healthy(i, timeout=30)
        while time.time() < deadline:
            if self.c.app_leader(self.app) and all(self.c.app_status(i, self.app) for i in self.c.ids):
                return True
            time.sleep(0.3)
        return False

    def add_room(self):
        for _ in range(60):
            try:
                self.client.call(1, "addRoom", {"id": self.room, "name": self.room, "capacity": 4},
                                 idem=f"room-{self.room}", timeout=20)
                return
            except HttpError as e:
                if app_code(e) == "Duplicate":
                    return
                time.sleep(0.5)
            except Exception:
                time.sleep(0.5)
        raise RuntimeError(f"could not add {self.room}")

    def final_read(self):
        """Every window of the room, strongly, retried until each answers: {window: slots}."""
        out = {}
        for w in range((max(self.next_slot, 1) - 1) // WINDOW + 1):
            for _ in range(40):
                t_inv = self.now()
                try:
                    slots, index = self.read_window(1, w)
                    out[w] = slots
                    self.record({"p": "final", "f": "read", "window": w, "type": "ok", "slots": slots,
                                 "index": index, "node": 1, "invoke": t_inv, "complete": self.now()})
                    break
                except Exception:
                    time.sleep(0.5)
        return out

    def run(self):
        self.add_room()
        ths = [threading.Thread(target=self.worker, args=(w,), daemon=True) for w in range(self.workers)]
        nem = threading.Thread(target=self.nemesis, daemon=True)
        for t in ths:
            t.start()
        nem.start()
        time.sleep(self.duration)
        nem.join()
        self.stop.set()
        for t in ths:
            t.join(timeout=120)
        stuck = [t.name for t in ths if t.is_alive()]
        healed = self.heal_wait()
        finals = self.final_read()
        with self.lock:
            # A worker still running must not append while the checker reads.
            self.frozen = True
        problems, stats, acked = self.check(finals, healed)
        if stuck:
            problems.append(f"worker threads still running after 120 s: {stuck}")
        return problems, stats, acked

    def check(self, finals, healed):
        problems = []
        books = [h for h in self.history if h["f"] == "book"]
        acked = [h for h in books if h["type"] == "ok"]
        unknown = [h for h in books if h["type"] == "info"]
        failed = [h for h in books if h["type"] == "fail"]
        windows = (max(self.next_slot, 1) - 1) // WINDOW + 1
        if len(finals) != windows:
            problems.append(f"final read incomplete: {len(finals)} of {windows} windows")

        # The final state: slot number -> booking id.
        held = {}
        for w, slots in finals.items():
            for s in slots:
                n = (int(s["start"]) - self.base) // SLOT_MS
                if n in held:
                    problems.append(f"slot {n} listed twice: {held[n]} and {s['bookingId']}")
                held[n] = s["bookingId"]
        attempted = {h["slot"] for h in books}
        phantom = sorted(set(held) - attempted)
        if phantom:
            problems.append(f"held slots nobody booked: {phantom[:10]}")
        for h in acked:
            if held.get(h["slot"]) != h["id"]:
                problems.append(f"acked booking LOST or replaced: slot {h['slot']} id {h['id']} -> {held.get(h['slot'])}")
        ids = list(held.values())
        if len(set(ids)) != len(ids):
            problems.append("one booking id holds two slots")
        if len(held) < len(acked):
            problems.append(f"held {len(held)} < acked {len(acked)}")
        if len(held) > len(acked) + len(unknown):
            problems.append(f"DUPLICATED: held {len(held)} > acked {len(acked)} + unknown {len(unknown)}")
        dup = [h for h in failed if h.get("code") == "Duplicate"]
        if dup:
            # A fresh slot's `book` fails `Duplicate` only when the booking id it minted exists:
            # `unique_id` reissued an id (D52).
            problems.append(f"booking id REISSUED (D52, Duplicate on a fresh slot): {len(dup)} books, "
                            f"first answer: {dup[0]['attempts'][-1][2][:160]}")
        other = [h for h in failed if h.get("code") not in ("Taken", "Duplicate")]
        if other:
            problems.append(f"unexpected refusals: {[h['attempts'][-1][2][:160] for h in other][:3]}")
        taken = [h for h in failed if h.get("code") == "Taken"]
        if taken:
            problems.append(f"Taken on a slot only one operation booked (ran twice): {[(h['slot'], h['attempts']) for h in taken][:5]}")
        acked_ids = [h["id"] for h in acked]
        if len(set(acked_ids)) != len(acked_ids):
            problems.append(f"duplicate booking ids among acked bookings: {sorted(acked_ids)}")

        # Each attempted slot as a register: written once (to held), read by every window read.
        regs = {}
        for h in books:
            if h["type"] == "fail":
                continue
            regs.setdefault(h["slot"], []).append({
                "f": "write", "value": HELD, "invoke": h["invoke"],
                "complete": h["complete"] if h["type"] == "ok" else math.inf})
        for h in self.history:
            if h["f"] != "read" or h["type"] != "ok":
                continue
            present = {(int(s["start"]) - self.base) // SLOT_MS for s in h["slots"]}
            lo, hi = h["window"] * WINDOW, (h["window"] + 1) * WINDOW
            for n in regs:
                if lo <= n < hi:
                    regs[n].append({"f": "read", "value": HELD if n in present else None,
                                    "invoke": h["invoke"], "complete": h["complete"]})
        not_lin = []
        for n, ops in regs.items():
            ok, info = check_register(ops)
            if ok is not True:
                not_lin.append((n, info))
        if not_lin:
            problems.append(f"slot registers not linearizable: {not_lin[:5]}")

        bad_probe = [p for p in self.idem_probe if p.get("same") is False]
        if bad_probe:
            problems.append(f"idempotent replay differed: {bad_probe}")
        probe_taken = [p for p in self.idem_probe if p.get("code") == "Taken"]
        if probe_taken:
            problems.append(f"replay of an acked key ran again (Taken): {probe_taken}")
        if not healed:
            problems.append("cluster did not heal within 60 s")
        reads = [h for h in self.history if h["f"] == "read" and h["p"] != "final"]
        stats = {
            "ops": len(self.history),
            "book_ok": len(acked), "book_info": len(unknown), "book_fail": len(failed),
            "retried_ok": sum(1 for h in acked if len(h["attempts"]) > 1),
            "reads_ok": sum(1 for h in reads if h["type"] == "ok"),
            "reads_fail": sum(1 for h in reads if h["type"] == "fail"),
            "held": len(held), "registers": len(regs),
            "idem_probes": len(self.idem_probe),
            "idem_probe_errors": sum(1 for p in self.idem_probe if "error" in p),
            "fail_answers": sorted({h["attempts"][-1][2][:120] for h in failed})[:5],
        }
        return problems, stats, [(h["id"], h["slot"], h["idem"]) for h in acked]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iterations", type=int, default=200)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--duration", type=float, default=6.0)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--wasm", default=None)
    ap.add_argument("--workdir", default=str(Path(__file__).resolve().parent / "work" / "rooms"))
    # 7600-7699 sits above every merobox workflow's base_port, so the two can run at
    # once; CLOUD_V2_PORT_OFFSET shifts it as it shifts the workflows (two checkouts).
    ap.add_argument("--base-port", type=int, default=7610)
    args = ap.parse_args()
    offset = int(os.environ.get("CLOUD_V2_PORT_OFFSET", "0"))
    if not (7600 <= args.base_port and args.base_port + 3 <= 7699):
        raise SystemExit("ports 7600-7699 only (before CLOUD_V2_PORT_OFFSET)")
    if args.base_port + offset + 3 > 65535:
        raise SystemExit(f"CLOUD_V2_PORT_OFFSET {offset} puts the nodes past port 65535")
    wasm = args.wasm or default_wasm()
    wd = Path(args.workdir)
    shutil.rmtree(wd, ignore_errors=True)
    wd.mkdir(parents=True)
    out = open(wd / "results.jsonl", "w")
    c = Cluster(wd, base_port=args.base_port + offset, cluster="roomsfault")
    t_start = time.time()
    total_fail = 0
    totals = {"ops": 0, "book_ok": 0, "book_info": 0, "book_fail": 0, "retried_ok": 0,
              "reads_ok": 0, "reads_fail": 0, "idem_probes": 0, "idem_probe_errors": 0}
    all_ids = {}  # acked booking id -> (iteration, slot, key)
    collisions = []
    # Far-future slots, so the no-show timer never releases a booking under test.
    base = (int(time.time() * 1000) // SLOT_MS + 10_000) * SLOT_MS
    try:
        c.start()
        app, key = setup(c, wasm)
        client = Client(c, app, key)
        time.sleep(2)
        for it in range(args.iterations):
            t_it = time.time()
            itr = Iteration(c, client, app, it, args.seed, args.duration, args.workers, base)
            problems, stats, acked = itr.run()
            for bid, slot, idem in acked:
                if bid in all_ids:
                    collisions.append({"id": bid, "first": all_ids[bid], "again": (it, slot, idem)})
                    problems.append(f"booking id {bid} REISSUED (D52): first {all_ids[bid]}, again {(it, slot, idem)}")
                else:
                    all_ids[bid] = (it, slot, idem)
            for k in totals:
                totals[k] += stats[k]
            rec = {"iteration": it, "seed": args.seed, "secs": round(time.time() - t_it, 2),
                   "nemesis": itr.nemesis_log, "problems": problems, "stats": stats}
            out.write(json.dumps(rec) + "\n")
            out.flush()
            if problems:
                total_fail += 1
                with open(wd / f"history-{it}.json", "w") as f:
                    json.dump({"history": itr.history, "idem": itr.idem_probe, "nemesis": itr.nemesis_log},
                              f, default=str)
            print(f"it {it} {rec['secs']}s nemesis={[e.get('kind') or e.get('op') for e in itr.nemesis_log]} "
                  f"ops={stats['ops']} book={stats['book_ok']}/{stats['book_info']}/{stats['book_fail']} "
                  f"held={stats['held']} reads={stats['reads_ok']}/{stats['reads_fail']} "
                  f"probes={stats['idem_probes']} problems={problems}", flush=True)
    finally:
        c.stop()
        print(f"TOTALS {json.dumps(totals)} distinct_acked_ids={len(all_ids)} id_collisions={len(collisions)}",
              flush=True)
        if collisions:
            print(f"COLLISIONS seed={args.seed} {json.dumps(collisions)}", flush=True)
        print(f"DONE iterations={args.iterations} failed_iterations={total_fail} "
              f"total_secs={round(time.time() - t_start, 1)}", flush=True)


if __name__ == "__main__":
    main()
