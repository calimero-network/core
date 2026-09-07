# Team Metrics — app-defined merge

**Example: `#[app::mergeable]`, for a field the storage layer cannot converge on its own.**

The sibling of `apps/team-metrics-macro`, which uses `#[derive(Mergeable)]`. Both
apps track the same team statistics; the difference shows up in exactly one
field, and it is worth being precise about which.

## What the two apps actually differ on

```rust
#[app::mergeable]
#[derive(Debug, Default, BorshSerialize, BorshDeserialize, AbiType)]
pub struct TeamStats {
    pub wins: Counter,     // converges WITHOUT any of this
    pub losses: Counter,
    pub draws: Counter,
    pub badges: u64,       // converges ONLY because of the rule below
}

impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        self.draws.merge(&other.draws)?;

        self.badges |= other.badges;   // ← the reason this app exists
        Ok(())
    }
}
```

**The counters need nothing from you.** Each is stored as its own child entity,
and the storage layer merges it by summing per-writer contributions. Delete
`#[app::mergeable]` and the whole `Mergeable` impl, and `wins` still converges.

**`badges` is a plain `u64`.** It lives in the value blob, and a blob has no
merge semantics of its own — without a declared rule it resolves
last-write-wins, so one node's badges survive and the other node's are silently
discarded. `#[app::mergeable]` is what makes bitwise-OR the rule instead.

That is the entire distinction. If your struct holds only CRDT collections, you
do not need this app's approach — use the derive.

## When to reach for it

Use `#[app::mergeable]` when a field has no CRDT of its own and last-write-wins
is the wrong answer for it. Use `#[derive(Mergeable)]` otherwise: dispatch costs
a WASM call per conflicting entry, and structural convergence reaches the same
result without one.

## The contract

Dispatch hands merge authority to your code, so `merge` must be:

- **deterministic** — same inputs, same output, always
- **commutative** — `merge(a, b) == merge(b, a)`
- **associative** and **idempotent** — `merge(a, a) == a`
- **total** — never `Err` on well-formed input

The last one is the trap. Returning `Err` is not validation; it is a refusal to
converge. The entity stays divergent and sync repair retries it indefinitely.
Validate on the write path — `award_badge` rejects an out-of-range badge — never
in `merge`.

Two related things are **not** possible, despite being easy to assume:

- **Skipping a field conditionally.** A field that is its own child entity (a
  `Counter`) converges whether or not your `merge` touches it. Omitting it does
  not exclude it.
- **Depending on wall-clock order.** A rule that consults timestamps is not
  commutative, and the two replicas will disagree.

## Tests

| Level | What it proves |
| ----- | -------------- |
| `src/lib.rs` unit tests | app logic, single node |
| `tests/converge.rs` | the counters converge and sum across replicas. It does **not** cover the custom rule — see the comment in that file for why the harness cannot |
| `workflows/team-metrics-custom.yml` | two real nodes each award their own badge and both end up holding both. This is the assertion that proves the merge function ran |

The badge assertion is the load-bearing one: a counter converges either way, so
a counter test cannot tell you whether your `merge` was ever called.

## Build & test

```bash
cargo mero build
```

Workflows run in CI via `merobox-workflows.yml`. Locally:

```bash
cargo mero build
merobox bootstrap run workflows/team-metrics-custom.yml \
  --no-docker \
  --binary-path ../../target/debug/merod \
  --e2e-mode
```
