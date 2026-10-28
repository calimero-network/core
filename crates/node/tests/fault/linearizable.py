"""Wing & Gong / Lowe linearizability check for a single register (Porcupine-style).

An op is a dict: {"f": "write"|"read", "value": v, "invoke": t0, "complete": t1}
complete = inf for an indeterminate write (it may have taken effect any time after invoke).
Reads that failed and writes that definitely failed must be removed by the caller.
"""

from __future__ import annotations

import math


def check_register(ops, init=None, max_steps=5_000_000):
    """Returns (ok: bool|None, info). None means the search hit max_steps."""
    if not ops:
        return True, "empty"
    # events: (time, kind, id); kind 0 = call, 1 = return. calls before returns on ties
    # is the permissive choice; we put returns AFTER calls only when times are equal.
    events = []
    for i, op in enumerate(ops):
        events.append((op["invoke"], 0, i))
        events.append((op["complete"], 1, i))
    events.sort(key=lambda e: (e[0], e[1]))

    # doubly linked list of events
    n = len(events)
    prev = list(range(-1, n - 1))
    nxt = list(range(1, n + 1))
    nxt[n - 1] = -1
    head = 0
    match = {}
    for idx, (_t, kind, i) in enumerate(events):
        if kind == 1:
            match[i] = idx
    call_idx = {}
    for idx, (_t, kind, i) in enumerate(events):
        if kind == 0:
            call_idx[i] = idx

    def lift(idx):
        nonlocal head
        # remove call event idx and its return
        for e in (idx, match[events[idx][2]]):
            p, q = prev[e], nxt[e]
            if p == -1:
                head = q
            else:
                nxt[p] = q
            if q != -1:
                prev[q] = p

    def unlift(idx):
        nonlocal head
        for e in (match[events[idx][2]], idx):
            p, q = prev[e], nxt[e]
            if p == -1:
                head = e
            else:
                nxt[p] = e
            if q != -1:
                prev[q] = e

    def step(state, op):
        if op["f"] == "write":
            return True, op["value"]
        return op["value"] == state, state

    linearized = 0  # bitset of op ids
    state = init
    cache = set()
    stack = []  # (call event idx, previous state)
    entry = head
    steps = 0
    best = 0
    while head != -1:
        steps += 1
        if steps > max_steps:
            return None, f"gave up after {max_steps} steps"
        if entry == -1:
            # no candidate: backtrack
            if not stack:
                return False, f"no linearization (deepest prefix {best} of {len(ops)} ops)"
            cidx, prev_state = stack.pop()
            i = events[cidx][2]
            linearized &= ~(1 << i)
            state = prev_state
            unlift(cidx)
            entry = nxt[cidx]
            continue
        t, kind, i = events[entry]
        if kind == 0:
            op = ops[i]
            ok, new_state = step(state, op)
            if ok:
                new_lin = linearized | (1 << i)
                key = (new_lin, new_state)
                if key not in cache:
                    cache.add(key)
                    stack.append((entry, state))
                    state = new_state
                    linearized = new_lin
                    lift(entry)
                    best = max(best, len(stack))
                    entry = head
                    continue
            entry = nxt[entry]
        else:
            # a return event: the op that returned here was not linearized in time
            if not stack:
                return False, f"no linearization (deepest prefix {best} of {len(ops)} ops)"
            cidx, prev_state = stack.pop()
            ii = events[cidx][2]
            linearized &= ~(1 << ii)
            state = prev_state
            unlift(cidx)
            entry = nxt[cidx]
    return True, "ok"


if __name__ == "__main__":
    inf = math.inf
    # self tests
    good = [
        {"f": "write", "value": 1, "invoke": 0, "complete": 2},
        {"f": "read", "value": 1, "invoke": 1, "complete": 3},
        {"f": "write", "value": 2, "invoke": 4, "complete": 5},
        {"f": "read", "value": 2, "invoke": 6, "complete": 7},
    ]
    assert check_register(good)[0] is True
    stale = good + [{"f": "read", "value": 1, "invoke": 8, "complete": 9}]
    assert check_register(stale)[0] is False
    lost = [
        {"f": "write", "value": 1, "invoke": 0, "complete": 1},
        {"f": "read", "value": None, "invoke": 2, "complete": 3},
    ]
    assert check_register(lost)[0] is False
    info = [
        {"f": "write", "value": 1, "invoke": 0, "complete": inf},
        {"f": "read", "value": None, "invoke": 2, "complete": 3},
        {"f": "read", "value": 1, "invoke": 4, "complete": 5},
    ]
    assert check_register(info)[0] is True
    flip = [
        {"f": "write", "value": 1, "invoke": 0, "complete": 1},
        {"f": "write", "value": 2, "invoke": 0.5, "complete": 3},
        {"f": "read", "value": 2, "invoke": 3.5, "complete": 4},
        {"f": "read", "value": 1, "invoke": 5, "complete": 6},
    ]
    assert check_register(flip)[0] is False
    conc = [
        {"f": "write", "value": 1, "invoke": 0, "complete": 10},
        {"f": "write", "value": 2, "invoke": 0, "complete": 10},
        {"f": "read", "value": 2, "invoke": 1, "complete": 2},
        {"f": "read", "value": 1, "invoke": 3, "complete": 4},
    ]
    assert check_register(conc)[0] is True
    conc2 = conc + [{"f": "read", "value": 2, "invoke": 5, "complete": 6}]
    assert check_register(conc2)[0] is False
    print("checker self-tests pass")
