# Process-level tests (`merod run --dev`, one process per node)

Black-box tests of a 3-node cluster whose nodes are separate `merod` processes, so
a test can SIGKILL, SIGSTOP, restart or upgrade one node. They replaced merobox's
cloud-v2 backend, which core no longer uses: its examples, its fuzzy load and
soak report, and the release-image smoke. CI runs them in `e2e.yml`'s
`process-suite` (nightly `--nightly`, and `release-process-suite` against the
latest release image's `merod`) and in `release.yml`'s `E2E on the merod image`
(`--smoke`).

| File | What it is |
| --- | --- |
| `suite.py` | The entry point: every test below, the PR / nightly / smoke sets |
| `rv2.py` | The cluster: ports, flags, kill/pause/resume/restart, per-node binaries, a package registry on 127.0.0.1, meroctl per account home |
| `soak.py` | Soak metrics and the report format (`{"version": 1, "runs": [...]}`), `soak.py report` renders it |
| `rooms_fault.py`, `linearizable.py` | Rooms under a randomized nemesis, checked for linearizability and idempotency (A4) |

## Tests

| Test | Sets | Replaces (merobox's, unless noted) | Checks |
| --- | --- | --- | --- |
| `kv-failover` | pr, smoke | merobox `workflow-examples/cloud-v2/kv-store-failover.yml` | kill node 1, read through the survivors, write with 2 of 3, restart it; freeze node 2, write through node 3, thaw; node 2 then node 1 catch up (stale reads at the write's index); indices increase |
| `frozen-leader` | pr, smoke | `frozen-leader.yml` | SIGSTOP the app's leader; a write through a follower lands within 10 s; the thawed leader reads it |
| `orgs-and-keys` | pr, smoke | `orgs-and-keys.yml` | an org, an invite redeemed; a member writes; an outsider gets `NOT_FOUND`; a confirm method answers `CONFIRM_REQUIRED`, then runs with the owner's passkey; a read-only app-scoped key reads (`len` = 0) and is `FORBIDDEN` to write; a removed member gets `NOT_FOUND` within 5 s |
| `packages` | pr, smoke | `packages.yml` | two signed kv-store releases published to a registry on 127.0.0.1 and installed by name; an app from the first; every node serves it from its own verified copy; upgraded by version, data kept; uninstall refused while an app runs it, allowed after the app is deleted; no packages left |
| `rolling-upgrade` | pr | `rolling-upgrade.yml` | the target build differs from the running one; each node restarted on it in turn while a writer keeps sending keyed writes; the app answers through the mixed cluster, each node rejoins the app's group, writes keep landing; writes on the new build; `ops/status` counts one app; no acknowledged write lost |
| `kv-soak` | pr, smoke | `workflows/fuzzy-tests/kv-store/fuzzy-test.yml` (merobox `fuzzy_test`, `MEROBOX_SOAK_REPORT`) | four org members, ten seeded keys, the workflow's weighted operations (`set_and_verify` 40, `write_burst` 25, `random_read` 15, `list_entries` 10, `cross_node_write_read` 3) for `--soak-seconds`; at least 99 % pass; the latency/throughput report |
| `rooms-fault` | nightly | (core's own) `rooms_fault.py` | runs it for `--rooms-iterations` with the suite's seed and ports |

`--nightly` runs every test three times, a 900 s soak and 50 rooms-fault
iterations. `--smoke` is the release-image set: no `rolling-upgrade` (one image,
one build) and no `rooms-fault`.

## Running

```bash
cargo build --release -p merod -p meroctl
cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release
for v in 1.0.0 1.1.0; do (cd apps/wasmer-fixtures/kv-store && cargo mero bundle --dev --no-icon --app-version "$v"); done
cargo test --release -p calimero-node --test rooms    # the Rooms component, for rooms-fault only

python3 -I crates/node/tests/fault/suite.py --list
python3 -I crates/node/tests/fault/suite.py                       # the PR set, 30 s soak
python3 -I crates/node/tests/fault/suite.py --nightly
python3 -I crates/node/tests/fault/suite.py --only frozen-leader --iterations 10
python3 -I crates/node/tests/fault/suite.py --smoke --merod /tmp/merod   # e.g. a release image's
python3 -I crates/node/tests/fault/suite.py --only kv-soak --soak-seconds 120 --soak-report soak.json
python3 -I crates/node/tests/fault/soak.py report soak.json --title "kv-store soak"
```

Inputs: `--merod`/`--meroctl` (else `RV2_MEROD`/`RV2_MEROCTL`, else
`RV2_BIN_DIR`, default `target/release`), `--kv-wasm` (`KV_STORE_WASM`),
`--kv-mpk`/`--kv-mpk-next` (`KV_STORE_MPK`/`KV_STORE_MPK_NEXT`, default
`dist/com.calimero.kv-store-{1.0.0,1.1.0}.mpk`), `--upgrade-merod` (default: a
copy of merod with one byte appended, CI's `merod-next`), `--rooms-wasm`,
`--soak-seconds` (`SOAK_SECONDS`, or the old `MEROBOX_FUZZY_SECONDS`), `--soak-report`
(`SOAK_REPORT`, or the old `MEROBOX_SOAK_REPORT`), `--json` (results), `--seed`,
`--workdir` (default `target/fault-suite`; `/dev/shm/...` keeps the disk free),
`--keep` (keep passing tests' directories too; a failed test's is always kept,
with `nodeN.log`).

Ports: test `T` uses its base (`suite.py --list` order: rooms-fault 7610,
kv-failover 7620, frozen-leader 7625, orgs-and-keys 7630, packages 7635,
rolling-upgrade 7640, kv-soak 7650) + node, plus `WORKFLOW_PORT_OFFSET`, so two
checkouts can run at once. `scripts/soak-flamegraph.sh` finds kv-soak's node by
that port.

Each account is a meroctl home with a file passkey and its own API key: calls
and views go over plain HTTP with the key (`--dev` allows it), to the account's
own node first (`index % 3 + 1`), skipping one that does not answer, as merobox
did; ceremonies (sign-up, confirmations, upgrades) go through meroctl.

## Why not `merod local`

`merod local` starts every node in one process (`calimero_node::local::start`),
so a test cannot crash, freeze or upgrade one node, and the image's entry point
is `merod run`, which is what the smoke should exercise. The suite starts
`merod run --dev` per node in every mode, smoke included.
