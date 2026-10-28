# apps/ - Example WASM Applications

Example and test WebAssembly applications demonstrating Calimero SDK usage.

## Package Identity

- **Target**: `wasm32-unknown-unknown` (wasmer apps); `wasm32-wasip2` for the
  [component apps](#component-apps-calimero-app-sdk-wasm32-wasip2)
- **SDK**: `calimero-sdk` (wasmer apps); `calimero-app-sdk` (component apps)
- **Storage**: `calimero-storage` (CRDTs) for wasmer apps; tables for component apps

## Commands

```bash
# Add WASM target (one-time)
rustup target add wasm32-unknown-unknown

# Build one app (emits + embeds the ABI)
cargo mero build --manifest-path apps/wasmer-fixtures/kv-store/Cargo.toml

# Build all apps
./scripts/build-all-apps.sh
```

## Available Apps

| App                      | Purpose                | Good Example For       |
| ------------------------ | ---------------------- | ---------------------- |
| `kv-store`               | Simple key-value store (component SDK) | `Table` + `Counter`, events, a typed error, a `confirm` method; `TestApp` tests and `scenarios/*.json`. Its wasmer build, the node tests' fixture, is `wasmer-fixtures/kv-store` |
| `kv-store-init`          | KV with custom init (component SDK) | `#[app::init]` seeding the first state as the owner |
| `org-gated`              | Org notes (component SDK) | `env::org_role` / `env::in_team`: gate methods on the caller's organization role and teams; `scenarios/cluster/` changes a team mid-run |
| `wasmer-fixtures/org-gated` | Org notes (v1 SDK)  | The wasmer fixture `crates/node/tests/cluster.rs` loads (`org_gated.wasm`) for `env::caller_org()` |
| `kv-store-with-handlers` | KV with event handlers | Event handling         |
| `sorted-kv-store`        | Ordered KV store (component SDK) | A `Table` read in key order: range / prefix / page / first / last through its cursor |
| `sorted-set-store`       | Ordered set (component SDK) | A `Table` whose record is the element: range / prefix / page / min / max |
| `authored-sorted-kv-store` | Shared noticeboard (component SDK) | Per-note authors checked against `env::caller()`, topic slices through a `Table` index, and why the two go together |
| `indexed-issue-tracker`  | Issue tracker (component SDK) | `Table` indexes, a label table and maintained counts: filter, count and page by field values without scanning |
| `indexed-forum`          | Forum (component SDK)  | Tables with compound and optional-field indexes and maintained counts; a tag index as one row per tag; v1's `Moderated`/`Frozen`/`Authored` policies as explicit checks against `env::caller()` and a moderators table |
| `search-chat`            | Chat with full-text search | `#[derive(app::Searchable)]` + `app::search_indexes!`: the node indexes a map on its own; a view queries it with `Query` and gets entries read back from state. Example and E2E fixture for search |
| `permissions-showcase`   | Team space (component SDK) | One table per v1 write policy, each an explicit check: values written only in `init`, write-once messages, moderator-removed announcements, author-owned pages and revisions, evidence keyed by its SHA-256 |
| `name-registry`          | Usernames              | `Registry<K, V>`: one owner per name, decided by a TEE; claim, `#[app::tee]` resolve and timer sweep, release and re-claim |
| `name-registry-admin`    | Usernames (component SDK) | v1's `Registry<K, V, Admin>` as claim, verdict and admin tables: one owner per name, decided by the admins (the first claim applied wins); the app owner is the first admin, `setAdmins` hands the role on, a lost claim is renewed with `rebid` after a release |
| `migrations/migration-suite-v1..v5` | Migration chain (each `vN` migrates from `vN-1`) | additive, remove, rename, type-change |
| `migrations/scenario-*-v{1,2}` | Standalone v1+v2 fixture pairs (each pair self-contained) | new-method, new-enum-variant, pure-bugfix, crdt-native, struct-to-enum, field-split, field-remove-archive, invariant-reshuffle |
| `components-demo`        | Access-control patterns (component SDK) | An owned cell, a writer set and a role registry (v1's `Ownable`, `PermissionedStorage`, `AccessControl`), each a table plus a check against `env::caller()` |
| `kv-store-with-shared-storage` | Writer-set KV (component SDK) | A writer set as table rows plus an explicit check, and its rotation. The wasmer `SharedStorage` build is `wasmer-fixtures/kv-store-with-shared-storage` |
| `wasmer-fixtures/*`      | Wasmer builds of ported apps | Kept for the node's tests, the size gate and the release bundle; same crate and wasm names as before the port |
| `kv-store-with-user-and-frozen-storage` | Per-user + immutable KV (component SDK) | Rows keyed by the caller's account (v1's `UserStorage`), content-addressed rows hashed with `sha2` (v1's `FrozenStorage`) |
| `custom-key-store`       | Custom key type (component SDK) | A `ToKeyPart` newtype key (`Slug` over a `String`, normalized), and why a struct without it does not compile |
| `private_data`           | Node-local secrets     | `#[app::private]`; public hashes in an `AuthoredMap` under creator-named ids |
| `tee-dice` / `tee-cards` | TEE-decided games      | `TeeOnly`, `TeeSecret`, `#[app::tee]` handlers and timers |
| `blobs`                  | Blob storage demo      | Blob operations; file records as the uploader's own `AuthoredMap` entries |
| `collaborative-editor`   | Collaborative text     | Complex CRDTs          |
| `fugue-editor`           | Collaborative text (Fugue) | `FugueText` CRDT       |
| `fugue-collab`           | Collaborative text (Fugue), with deletes | E2E guest |
| `rich-collab`            | Collaborative rich text (blocks, marks) | `RichDocument` / `RichText` E2E guest |
| `team-metrics-macro`     | Team metrics (component SDK) | Per-team counts as a plain record: what v1's `#[derive(Mergeable)]` becomes when one log orders every call |
| `team-metrics-custom`    | Team metrics with badges (component SDK) | A bitmask field set by read-modify-write: what v1's custom `#[app::mergeable]` rule becomes |
| `xcall-example`          | Cross-context calls    | XCall pattern          |
| `outbound-probe`         | Outbound HTTPS         | `env::http_request` from each kind of run; E2E fixture for the outbound gates, against the test build's `.test` echo |

## Component apps (`calimero-app-sdk`, `wasm32-wasip2`)

Seventeen apps are on the component SDK: `rooms`, `authored-sorted-kv-store`,
`components-demo`, `custom-key-store`, `indexed-forum`, `indexed-issue-tracker`,
`kv-store`, `kv-store-init`, `kv-store-with-shared-storage`,
`kv-store-with-user-and-frozen-storage`, `name-registry-admin`, `org-gated`
(package `org-gated-notes`), `permissions-showcase`, `sorted-kv-store`,
`sorted-set-store`, `team-metrics-custom` and `team-metrics-macro`. The wasmer
builds of `kv-store`, `kv-store-with-shared-storage` and `org-gated` stay as
fixtures in `wasmer-fixtures/`. The other apps stay on the wasmer SDK until the
component SDK has what they use (blobs, xcall, search, event handlers, TEE
methods, outbound requests, migrations, CRDT types). Component apps are standalone
crates (`[workspace]` in their `Cargo.toml`, `calimero-app-sdk` by path), not
members of the core workspace, so `cargo test --manifest-path apps/<app>/Cargo.toml`
runs them (CI's "Cargo test (standalone v2 apps)" loop lists each). Storage is
`Table<Record>`, `UnorderedMap` and `Counter`; ownership and permission rules are
explicit checks against `env::caller()` / `env::org_role()`, stated in each app's
doc comment. Their end-to-end tests are scenario files:

- `scenarios/<name>.json`: app-level steps in the format of
  `crates/app-sdk/src/testing/scenario.rs`, run by the app's `tests/scenarios.rs`
  in the harness; `scenarios/<name>.org.json` is the org it needs
  (`{ "admins": [..], "members": [..], "guests": [..], "teams": {..} }`, the owner
  is always `owner`).
- `scenarios/cluster/<name>.json`: steps only a real cluster can run (teams
  changed mid-run, packages, upgrades).

```bash
cargo test --manifest-path apps/org-gated/Cargo.toml   # harness tests + scenarios
cargo run -p calimero-app-build -- apps/org-gated --out org-gated.wasm
```

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

## Patterns

### Basic App Pattern

- ✅ DO: Follow `wasmer-fixtures/kv-store/src/lib.rs` as reference (the component SDK's is `kv-store/src/lib.rs`)

```rust
// Simplified example based on apps/wasmer-fixtures/kv-store/src/lib.rs
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    items: UnorderedMap<String, LwwRegister<String>>,
}

#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
    Cleared,
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> KvStore {
        KvStore {
            items: UnorderedMap::new(),
        }
    }

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);

        if self.items.contains(&key)? {
            app::emit!(Event::Updated {
                key: &key,
                value: &value,
            });
        } else {
            app::emit!(Event::Inserted {
                key: &key,
                value: &value,
            });
        }

        self.items.insert(key, value.into())?;

        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);

        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }
}
```

**Key points:**

- Use `#[app::state(emits = for<'a> Event<'a>)]` to declare events
- Use nested CRDTs: `UnorderedMap<String, LwwRegister<String>>` for last-write-wins semantics
- Return `app::Result<T>` from methods (not plain `T` or `Option<T>`)
- Use `app::log!` for logging and `app::emit!` for events
- Define `Error` enum with `thiserror::Error` and `Serialize` for proper error handling
- Custom `init()` creates collections explicitly (not `Default`)

### Event Handling Pattern

- ✅ DO: Define events with `#[app::event]` and use `app::emit!` macro
- ✅ DO: See `kv-store-with-handlers/src/lib.rs` for event handlers

```rust
#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
}

// Emit events in methods
pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
    if self.items.contains(&key)? {
        app::emit!(Event::Updated { key: &key, value: &value });
    } else {
        app::emit!(Event::Inserted { key: &key, value: &value });
    }
    // ...
}

// Event handlers (optional, see kv-store-with-handlers). Peers run only
// methods marked #[app::handler].
#[app::handler]
pub fn insert_handler(&mut self, key: &str, value: &str) -> app::Result<()> {
    app::log!("Handler called for insert: {} = {}", key, value);
    Ok(())
}
```

### Error Handling Pattern

- ✅ DO: Use `thiserror::Error` with `Serialize` for app errors
- ✅ DO: Return `app::Result<T>` and use `app::bail!` for errors

```rust
use thiserror::Error;
use calimero_sdk::serde::Serialize;

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
    #[error("invalid value: {0}")]
    InvalidValue(&'a str),
}

// Use in methods
pub fn get_result(&self, key: &str) -> app::Result<String> {
    let Some(value) = self.get(key)? else {
        app::bail!(Error::NotFound(key));
    };
    Ok(value)
}
```

### Cargo.toml Pattern

```toml
[package]
name = "kv-store"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
calimero-sdk = { workspace = true }
calimero-storage = { workspace = true }
thiserror = { workspace = true }  # For error handling

# No [build-dependencies]: `cargo mero build` emits res/abi.json and
# res/state-schema.json from src/*.rs, so an app carries no build script.

[profile.release]
codegen-units = 1
opt-level = "z"
lto = true
debug = false
panic = "abort"
overflow-checks = true
```

**Note:** Workspace dependencies (`workspace = true`) are preferred in monorepo setups.

### Unit Testing Pattern (`TestHost`)

Exercise app logic as plain Rust - no WASM build, no node - with
the in-process harness `calimero_sdk::testing::TestHost`. It runs your methods
against an in-memory mock host that records events/logs and serves a
configurable executor identity, so you get millisecond `#[cfg(test)]`
assertions and real TDD.

The bridge that lets `TestHost` drive `Root` state needs
`calimero-storage`'s `testing` feature, enabled as a dev-dependency:

```toml
[dev-dependencies]
# Enables `register_crdt_merge` + the native mock host for `TestHost`.
calimero-storage = { workspace = true, features = ["testing"] }
```

```rust
#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;
    use super::*;

    #[test]
    fn set_get() {
        // `build` runs against a freshly-reset store, like `#[app::init]`.
        let mut app = TestHost::new(KvStore::init);

        app.call(|s| s.set("k".into(), "v".into())).unwrap();        // &mut self + commit
        assert_eq!(app.view(|s| s.get("k")).unwrap(), Some("v".to_owned())); // &self

        assert_eq!(app.events().len(), 1);                            // app::emit! captured
    }
}
```

- `call(|s| ...)` loads state, runs a `&mut self` method, commits.
- `view(|s| ...)` reads via `&self` without committing.
- `call_as(executor, |s| ...)` runs a mutation as a specific identity (multi-author CRDT tests).
- `events()` / `logs()` return what `app::emit!` / `app::log!` produced.
- Unsupported in-process: `env::xcall`, networked blobs, `ed25519_verify` (they panic if hit) - test those paths with cluster scenarios.

### Building an App

```bash
cargo mero build --manifest-path apps/<app>/Cargo.toml
```

## Key Reference Files

| File                                | Purpose         |
| ----------------------------------- | --------------- |
| `wasmer-fixtures/kv-store/src/lib.rs` | Basic CRDT app  |
| `kv-store/src/lib.rs`               | Basic component app |
| `components-demo/src/lib.rs`        | Auth patterns   |
| `kv-store-with-handlers/src/lib.rs` | Event handlers  |
| `blobs/src/lib.rs`                  | Blob operations |

## JIT Index

```bash
# Find all app entry points
rg -n "#\[app::state\]" */src/

# Find all public methods
rg -n "pub fn" */src/lib.rs

# Find CRDT usage
rg -n "UnorderedMap|Counter|LwwRegister|Vector" */src/

# Find event definitions
rg -n "#\[app::event\]" */src/

# Find event emissions
rg -n "app::emit!" */src/

# Find error definitions
rg -n "#\[derive.*Error" */src/

# Find error handling
rg -n "app::bail!" */src/
```

## Scenarios

An app's end-to-end tests are scenario files under `scenarios/`, run on an
in-process cluster (three nodes, sealed channels, the real WASM) by
`cargo test -p calimero-node --test scenarios`; each file gets a cluster of
its own. Two formats
([Testing strategy](../docs/src/content/docs/contribute/testing.mdx#scenarios)):

- **App-level**, `scenarios/<name>.json`: the step array of
  `calimero_app_sdk::testing::scenario` (`call`/`view`/`tick`/`advance`/
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
are built on first use when missing (`cargo mero build`;
`scripts/build-workflow-fixtures.py` for signed releases). A scenario that
cannot pass yet goes in `crates/node/tests/scenarios-skip.txt` with its reason.

## Building for Production

```bash
# app-release profile, wasm-opt, and the embedded ABI in one step
cargo mero build --manifest-path apps/<app-name>/Cargo.toml

# Output: apps/<app-name>/res/<app_name>.wasm (ABI embedded)
```

A bare `cargo build --target wasm32-unknown-unknown` compiles the same code but
emits no ABI and embeds nothing, so the node cannot introspect the result.

## Common Gotchas

- Must use `crate-type = ["cdylib"]` in Cargo.toml
- All state fields must be serializable (borsh)
- Use `#[borsh(crate = "calimero_sdk::borsh")]` for borsh derives
- Use `#[serde(crate = "calimero_sdk::serde")]` for serde derives
- Return `app::Result<T>` from methods, not plain `T` or `Option<T>`
- Use nested CRDTs (`UnorderedMap<String, LwwRegister<String>>`) for last-write-wins semantics
- Events must use lifetime parameters: `Event<'a>` with `emits = for<'a> Event<'a>`
- Errors must implement `Serialize` for proper JSON-RPC error responses
- Use `--release` or `--profile app-release` for deployment
- Panic behavior in WASM differs from native
- Test locally with `meroctl call` before deployment
