# apps/ - Example and Test Applications

Example and test apps on the Calimero component SDK. Each builds into a
`calimero:app` component; the node runs nothing else (SDK 0.12 apps, which ran
on wasmer, were removed with that engine).

## Package Identity

- **Target**: `wasm32-wasip2`, built into a `calimero:app` component
- **SDK**: `calimero-sdk` (by path); storage is `Table<Record>` and the
  collections `UnorderedMap`, `Set`, `Vector`, `Cell`, `Counter`, `PerUser`,
  `Frozen`, `Text` and `RichText`

## Commands

```bash
rustup target add wasm32-wasip2                       # once (rust-toolchain.toml lists it)
cargo test --manifest-path apps/<app>/Cargo.toml      # harness tests + app-level scenarios
cargo install --locked --path crates/meroctl          # once: puts meroctl on PATH
meroctl app build apps/<app> --out <app>.wasm
meroctl app pack --wasm <app>.wasm --package <id> --app-version 1.0.0 \
  --out dist/<id>-1.0.0.mpk                           # a signed single-service .mpk (dev key)
meroctl app inspect dist/<id>-1.0.0.mpk               # verify it as a node does
```

Each app's `Cargo.lock` must match its manifests and change only when they need it to (CI's "App lockfiles" check, `python3 scripts/check-app-lockfiles.py`).
After a dependency change in a `Cargo.toml` an app resolves (its own, the root's or a crate it reaches by path), run `cargo update --workspace --manifest-path apps/<app>/Cargo.toml`; restore any other rewrite with `git checkout origin/master -- <lockfile>`.

## Testing a compute method locally

A compute method (`#[app::compute]`) is tested alone, with no app component, cluster or job node.
`meroctl app build` writes the compute component as `compute.component.wasm` beside `--out`.

```bash
meroctl app build apps/<app> --out <app>.wasm
meroctl local compute run compute.component.wasm --method <name> [--args <json or @file>] [--input <file>]... --out <dir> [--seed <hex>] [--twice]
```

It runs on the job node's engine and run setup with the method's caps from the compute ABI, a fixed clock and seeded randomness.
Inputs are plain files and outputs are written to `<dir>/output-<i>` in the order the run finished them.
It prints progress as the run reports it, then the result, each output and the usage, and exits non-zero with the reason (`fuel`, `memory`, `trap`, `app-error`, or `lost` for a wall clock that ran out) when the run fails.
`--twice` runs again and fails, naming what differs, if the result or an output differs.

## Available Apps

| App                      | Purpose                | Good Example For       |
| ------------------------ | ---------------------- | ---------------------- |
| `kv-store`               | Simple key-value store | `UnorderedMap`, events, a typed error, ordered reads (range / prefix / page / first / last through the map's cursor); `TestApp` tests, `scenarios/*.json` and a migration from its shipped release |
| `wiki`                   | Team wiki | A seeded first state from `#[app::init]`, a `ToKeyPart` newtype key (`Slug`), author-owned pages with revisions, a sorted tag `Set` read by prefix / range / page, per-person drafts in `PerUser`, content-addressed attachments in `Frozen` (`sha2`) |
| `forum`                  | Forum | Tables with compound and optional-field indexes and maintained counts, a tag index as one row per tag; the author is the caller, write-once chat messages, moderator-removed announcements, a charter written only in `init`, moderators appointing moderators |
| `issue-tracker`          | Issue tracker | `Table` indexes, a label table and maintained counts: filter, count and page by field values without scanning |
| `team-space`             | Team space | Access control in layers: organization standing (`env::org_role` / `env::in_team`) for the pinned and team notes, then the app's own rows: an owned config with transfer, one writer-set mechanism (status line, settings, roles) and its rotation; `scenarios/cluster/` changes a team mid-run; also the node's org and writer-set fixture (`crates/node/tests`) |
| `name-registry`          | Usernames | Claim, verdict and admin tables: one owner per name, decided by the admins (the first claim applied wins); the app owner is the first admin, `setAdmins` hands the role on, a lost claim is renewed with `rebid` after a release |
| `rooms`                  | Room booking | The node's Rooms tests (`crates/node/tests/rooms.rs`) and the fault test |
| `calendar`               | Each caller's events | The app the units examples build on: a person's and an app's own events; `env::via()` keeps private events from an app acting for the person |
| `planner`                | Team planner | `uses` with `mode = "create"`: a calendar created with each instance, called as the planner through its generated client (`uses::calendar`) |
| `booking`                | Booking page | `uses` with `mode = "attach"` and `methods`: a calendar the owner attaches, called as the person with their grant; the calendar's own error comes back typed (`uses::calendar::Error`) |
| `compute-redact`         | CSV redaction | `#[app::compute]` runs: a preview then one run per part, a queue of at most 4 active runs, the compute component (`meroctl app build --compute-out`) that may import only the compute world; the policy is set by an admin and copied into each job; `share` publishes a done job's outputs (`compute::publish`) and `link` grants a download (`blobs.grant`) |
| `fixture-kv`             | The node's kv fixture | Calls, views, a `confirm` method (`clear`), files, blobs, xcalls, actions, timers; `crates/node/tests` (`harness::kv_wasm`), `meroctl`'s tests, the process suite and the release's `kv-store-test-fixture.mpk` |
| `fixture-compute`        | The compute fixture | Compute methods that stream files, report progress and break each cap; `crates/node/tests/job_runs.rs` and `crates/meroctl/compute/tests` |
| `fixture-notes-v1`, `fixture-notes-v2` | Two releases of one app | The consent page's upgrade diff (`an_upgrades_page_names_what_the_components_abis_change` in `crates/node/tests/consent_page.rs`) |
| `fixture-captcha`        | Captcha service | Synchronous xcalls (`env::xcall_call`) from other apps of the org, and a Turnstile check with `env::fetch`; `crates/node/tests/component_cluster.rs` |
| `fixture-files`          | The files fixture | Deterministic files kept by `BlobRef`, and the refusal to stream them back outside a compute run; `crates/node/tests` |

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

## Apps that use other apps

An app whose release declares `[package.metadata.calimero.uses]` (`planner`, `booking`)
calls each target through a typed client its `build.rs` generates
(`calimero_app_build::uses::build()` in `build.rs`, brought in with `app::uses!()`; a build-dependency on `crates/app-build`, which the
apps already reach as a dev-dependency, so their lockfiles do not change) from two committed
files: `calimero.lock` and `calimero/abi/<name>.json`. Refresh them after the target changes
its ABI, then commit both:

```bash
cd apps/planner && meroctl app uses update calendar --from ../calendar
meroctl app uses check
```

A build fails while the lock, the vendored ABI and `Cargo.toml` disagree. A call commits in the
target's log and is not rolled back if the run fails afterwards.

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
| `team-space/src/lib.rs`             | Auth patterns   |
| `fixture-kv/src/lib.rs`             | Every host feature the node tests |
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
  "name": "KV store",
  "cluster": { "nodes": 3 },
  "accounts": ["alice", "bob"],
  "steps": [
    { "type": "create_org", "account": "alice", "org_name": "Kv", "outputs": { "org": "orgId" } },
    { "type": "deploy", "account": "alice", "wasm": "{{env.CORE_WASM_DIR}}/kv_store.wasm",
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
