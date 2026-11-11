# apps/ - Example and Test Applications

Example and test apps on the Calimero component SDK. Each builds into a
`calimero:app` component; the node runs nothing else (SDK 0.12 apps, which ran
on wasmer, were removed with that engine).

## Package Identity

- **Target**: `wasm32-wasip2`, built into a `calimero:app` component
- **SDK**: `calimero-sdk` (by path); storage is `Table<Record>`,
  `UnorderedMap`, `Cell` and `Counter`

## Commands

```bash
rustup target add wasm32-wasip2                       # once (rust-toolchain.toml lists it)
cargo test --manifest-path apps/<app>/Cargo.toml      # harness tests + app-level scenarios
scripts/install-mero-app-build.sh   # once: puts mero-app-build on PATH
mero-app-build apps/<app> --out <app>.wasm
python3 -I scripts/pack-component.py --wasm <app>.wasm --package <id> --app-version 1.0.0 \
  --out dist/<id>-1.0.0.mpk                           # a signed single-service .mpk (dev key)
```

## Testing a compute method locally

A compute method (`#[app::compute]`) is tested alone, with no app component, cluster or job node.
Put `mero-app-build` and `mero-compute` on PATH once with `scripts/install-mero-app-build.sh` and `scripts/install-mero-compute.sh`.
`mero-app-build` writes the compute component as `compute.component.wasm` beside `--out`.

```bash
mero-app-build apps/<app> --out <app>.wasm
mero-compute run compute.component.wasm --method <name> [--args <json or @file>] [--input <file>]... --out <dir> [--seed <hex>] [--twice]
```

It runs on the job node's engine and run setup with the method's caps from the compute ABI, a fixed clock and seeded randomness.
Inputs are plain files and outputs are written to `<dir>/output-<i>` in the order the run finished them.
It prints progress as the run reports it, then the result, each output and the usage, and exits non-zero with the reason (`fuel`, `memory`, `time`, `trap`, `app-error`) when the run fails.
`--twice` runs again and fails, naming what differs, if the result or an output differs.

## Available Apps

| App                      | Purpose                | Good Example For       |
| ------------------------ | ---------------------- | ---------------------- |
| `kv-store`               | Simple key-value store | `Table` + `Counter`, events, a typed error; `TestApp` tests and `scenarios/*.json` |
| `kv-store-init`          | KV with custom init | `#[app::init]` seeding the first state as the owner |
| `org-gated`              | Org notes | `env::org_role` / `env::in_team`: gate methods on the caller's organization role and teams; `scenarios/cluster/` changes a team mid-run |
| `sorted-kv-store`        | Ordered KV store | A `Table` read in key order: range / prefix / page / first / last through its cursor |
| `sorted-set-store`       | Ordered set | A `Table` whose record is the element: range / prefix / page / min / max |
| `authored-sorted-kv-store` | Shared noticeboard | Per-note authors checked against `env::caller()`, topic slices through a `Table` index, and why the two go together |
| `indexed-issue-tracker`  | Issue tracker | `Table` indexes, a label table and maintained counts: filter, count and page by field values without scanning |
| `indexed-forum`          | Forum  | Tables with compound and optional-field indexes and maintained counts; a tag index as one row per tag; moderation as explicit checks against `env::caller()` and a moderators table |
| `permissions-showcase`   | Team space | One table per write policy, each an explicit check: values written only in `init`, write-once messages, moderator-removed announcements, author-owned pages and revisions, evidence keyed by its SHA-256 |
| `name-registry-admin`    | Usernames | Claim, verdict and admin tables: one owner per name, decided by the admins (the first claim applied wins); the app owner is the first admin, `setAdmins` hands the role on, a lost claim is renewed with `rebid` after a release |
| `components-demo`        | Access-control patterns | An owned cell, a writer set and a role registry, each a table plus a check against `env::caller()` |
| `kv-store-with-shared-storage` | Writer-set KV | A writer set as table rows plus an explicit check, and its rotation; `crates/node/tests/shared_storage.rs` runs it on a cluster |
| `kv-store-with-user-and-frozen-storage` | Per-user + immutable KV | Rows keyed by the caller's account, content-addressed rows hashed with `sha2` |
| `custom-key-store`       | Custom key type | A `ToKeyPart` newtype key (`Slug` over a `String`, normalized), and why a struct without it does not compile |
| `team-metrics`           | Team metrics with badges | Per-team counts and a bitmask as a plain record |
| `rooms`                  | Room booking | The node's Rooms tests (`crates/node/tests/rooms.rs`) and the fault test |
| `compute-redact`         | CSV redaction | `#[app::compute]` runs: a preview then one run per part, a queue of at most 4 active runs, the compute component (`mero-app-build --compute-out`) that may import only the compute world; the policy is set by an admin and copied into each job; `share` publishes a done job's outputs (`compute::publish`) and `link` grants a download (`blobs.grant`) |
| `v2-fixtures/kv`         | The node's kv fixture | Calls, views, a `confirm` method (`clear`), files, blobs, xcalls, actions, timers; `crates/node/tests` (`harness::kv_wasm`), `meroctl`'s tests, the process suite and the release's `kv-store-test-fixture.mpk` |
| `v2-fixtures/compute` | The compute fixture | Compute methods that stream files, report progress and break each cap; `crates/node/tests/job_runs.rs` and `tools/mero-compute/tests` |
| `v2-fixtures/org-gated`, `captcha`, `files` | Node test fixtures | What `crates/node/tests` needs that the example apps do not show |

## Component apps

Component apps are standalone crates (`[workspace]` in their `Cargo.toml`,
`calimero-sdk` by path), not members of the core workspace, so
`cargo test --manifest-path apps/<app>/Cargo.toml` runs them (CI's "Cargo test
(standalone v2 apps)" loop lists each). Ownership and permission rules are
explicit checks against `env::caller()` / `env::org_role()`, stated in each app's
doc comment. Their end-to-end tests are scenario files:

- `scenarios/<name>.json`: app-level steps in the format of
  `crates/sdk/src/testing/scenario.rs`, run by the app's `tests/scenarios.rs`
  in the harness; `scenarios/<name>.org.json` is the org it needs
  (`{ "admins": [..], "members": [..], "guests": [..], "teams": {..} }`, the owner
  is always `owner`).
- `scenarios/cluster/<name>.json`: steps only a real cluster can run (teams
  changed mid-run, packages, upgrades).
- `migrations/<version>.json`: a scenario with one `{ "op": "upgrade" }` step,
  run by the app's `tests/migrations.rs` (`run_migration`) from the component in
  `releases/<version>.mpk`, the signed bundle that release shipped, to the
  crate's build: the new code must read what every shipped release stored
  (`apps/kv-store`; `build/testing.mdx`).

## App Structure

Each app follows this structure:

```
app-name/
├── Cargo.toml                # Crate config
├── README.md                 # App documentation
├── src/
│   └── lib.rs                # App implementation
└── scenarios/
    ├── <name>.json           # App-level scenario (harness and cluster)
    ├── <name>.org.json       # Its organization (optional)
    └── cluster/*.json        # Cluster scenarios (node-level steps)
```

## Key Reference Files

| File                                | Purpose         |
| ----------------------------------- | --------------- |
| `kv-store/src/lib.rs`               | Basic component app |
| `components-demo/src/lib.rs`        | Auth patterns   |
| `v2-fixtures/kv/src/lib.rs`         | Every host feature the node tests |
| `crates/sdk/AGENTS.md`          | The SDK itself |

## Scenarios

An app's end-to-end tests are scenario files under `scenarios/`, run on an
in-process cluster (three nodes, sealed channels, the real component) by
`cargo test -p calimero-node --test scenarios`; each file gets a cluster of
its own. Two formats
([Testing strategy](../docs/src/content/docs/contribute/testing.mdx#scenarios)):

- **App-level**, `scenarios/<name>.json`: the step array of
  `calimero_sdk::testing::scenario` (`call`/`view`/`tick`/`advance`/
  `removeMember`, `as`, `expect`, `save`, `$var`, `@account`, `events`). The
  app's own `tests/scenarios.rs` runs it in the harness; the cluster runner
  builds the app, creates the organization of `<name>.org.json`
  (`{ "admins": [], "members": [], "guests": [], "teams": { "t": ["acct"] } }`,
  the owner is `owner`) with invites and teams, deploys the app in it and runs
  the same steps, `as` each account's client, step `i` on node `i % 3`, the app
  clock starting at `1_700_000_100_000`.
- **Cluster**, `scenarios/cluster/**/*.json`: node-level steps a harness cannot
  run (orgs, invites, teams, packages, upgrades, blobs, raw HTTP). An object in
  merobox's cloud-v2 workflow format as JSON; the former `workflows/*.yml` were
  converted by `scripts/workflow-to-scenario.py`, and the converter stays for
  any workflow still in YAML.

```json
{
  "name": "Sorted KV store",
  "cluster": { "nodes": 3 },
  "accounts": ["alice", "bob"],
  "steps": [
    { "type": "create_org", "account": "alice", "org_name": "Sorted", "outputs": { "org": "orgId" } },
    { "type": "deploy", "account": "alice", "wasm": "{{env.CORE_WASM_DIR}}/sorted_kv_store.wasm",
      "org": "{{org}}", "outputs": { "app": "appId" } },
    { "type": "call", "account": "alice", "app": "{{app}}", "method": "set", "args": { "key": "a", "value": "1" } },
    { "type": "view", "account": "alice", "app": "{{app}}", "method": "get", "args": { "key": "a" }, "expect": "1" }
  ]
}
```

```bash
cargo test -p calimero-node --test scenarios                       # every scenario
SCENARIO_FILTER=apps/<app>/ cargo test -p calimero-node --test scenarios
SCENARIO_SHARD=2/4 cargo test -p calimero-node --test scenarios -- --test-threads 2
cargo test -p calimero-node --test scenarios -- --list
```

`{{env.CORE_WASM_DIR}}/<crate>.wasm` and `.../dist/<package>-<version>.mpk`
are built on first use when missing (the app's component; no scenario installs a
signed release now). A scenario that
cannot pass yet goes in `crates/node/tests/scenarios-skip.txt` with its reason.
