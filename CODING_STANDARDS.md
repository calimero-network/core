# Coding Standards

How code in this repository is written.
`cargo fmt` and `cargo clippy -D warnings` enforce the mechanical rules (formatting, naming case); this file holds the ones they cannot check.

## Imports

Group imports in `StdExternalCrate` order, one blank line between groups:

```rust
// Standard library
use std::collections::HashMap;
use std::sync::Arc;

// External crates
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// Local crate and parent module
use crate::{common, Node};
use super::Shared;

// Local module declarations
mod config;
mod types;

// Symbols from local modules (optional)
use config::ContextConfig;
```

- **Module granularity:** one `use` per module path (`use core::pin::Pin;`, `use std::time::Duration;`), each crate's paths on separate lines rather than one nested tree.
- **Imported names:** bring a type in with `use std::fs::File;` and call `File::open(..)`, rather than spelling the full path at the call site.
- **Manifests:** list `Cargo.toml` dependencies alphabetically.

## Modules

Give each module a file named after it, and declare its children there: `crates/runtime/src/logic.rs` holds `mod host_functions;`, `crates/runtime/src/logic/host_functions.rs` holds `mod blobs; mod outbound;`, and the children live in `crates/runtime/src/logic/host_functions/blobs.rs` and `crates/runtime/src/logic/host_functions/outbound.rs`.
The existing `mod.rs` files predate this rule; a new module follows it.

Place a helper shared across a crate in a `commons.rs`-style module, and a type shared across crates in that area's `primitives` crate.

## Errors and panics

Return errors with `?`, mapping them into the crate's error type with `.map_err()`.
Crates use `eyre`, imported as `use eyre::Result as EyreResult;`.

A panic point (`.unwrap()`, `.expect()`, `assert!`, `panic!`) is reserved for an invariant that cannot fail, or a failure where aborting the node is the right response.
State that invariant beside it: `// SAFETY: guaranteed by X`.

## Control flow

Keep the main path at the left margin: bail early with the failing condition, then continue.

```rust
if !some_condition {
    return Err(YourError::Something);
}
// main path

let Ok(val) = thing else {
    return;
};
// main path using val
```

## Dead code

Every symbol a change adds is used in that same change: functions, variables, imports and types.
Delete commented-out code; add future code in the change that first uses it.
`#[allow(dead_code)]` carries a comment naming why (FFI, a test fixture).

- **Removing dead code:** run the **dead-code-cleanup** skill (`.cursor/skills/dead-code-cleanup/SKILL.md`); it proves there are no references before deleting.
- **Unreachable islands:** items that cite each other while the outermost edge points at a route, command or registration that does not exist are invisible to the compiler and clippy; find them with the **unreachable-subsystems** skill (`.cursor/skills/unreachable-subsystems/SKILL.md`).

## Security: trust boundaries

A PR touching a path in `TRUST_BOUNDARY_PATHS` ([`scripts/check-trust-boundary.py`](scripts/check-trust-boundary.py)) must answer the PR template's Trust boundary block; CI fails without it.
Fixing a finding: the **calimero-security-fix** skill ([`.cursor/skills/calimero-security-fix/SKILL.md`](.cursor/skills/calimero-security-fix/SKILL.md)).

- **Untrusted:** every peer, HTTP caller, registry download and app guest, and every field a signature or an attestation does not cover - [`clients_refuse_unattested_nodes_and_plain_requests`](crates/cloud-node/tests/cluster.rs).
- **Identity** comes from a verified credential or signature; a field in the request is a claim, not an identity - `principal` in [`crates/cloud-node/src/api.rs`](crates/cloud-node/src/api.rs), which derives the caller from the session token or API key alone.
- **Verify before acting:** the check that authorizes a store write, install or network call runs before it - [`an_uploaded_package_is_verified_and_reaches_the_other_nodes`](crates/cloud-node/tests/cluster.rs).
- **Limits:** every input from outside has a named constant bounding its size, count, time or concurrency, checked before the costly work - [`MAX_PENDING_CHALLENGES`](crates/cloud-node/src/api.rs), [`MAX_STATE_BYTES`](crates/cloud-node/src/presence.rs) and [`oversized_state_is_refused`](crates/cloud-node/src/presence.rs).
- **Ending membership:** every gated operation considers removed, departed, revoked, read-only and inherited actors, and actors from other organizations, and judges them on fresh system state - [`teams_work_like_v1_subgroups`](crates/cloud-node/tests/cluster.rs), `gate` in [`crates/cloud-node/src/api.rs`](crates/cloud-node/src/api.rs).
- **Closed input:** refuse unknown fields, variants and versions - the sealed envelope's `RequestHead` in [`crates/sealed/src/server.rs`](crates/sealed/src/server.rs), and `merod`'s `NodeArgs` ([`crates/merod/src/node_config.rs`](crates/merod/src/node_config.rs)).
- **One gate:** enforce a rule in the shared function every caller goes through - `attest::verify` in [`crates/cloud-client/src/attest.rs`](crates/cloud-client/src/attest.rs), which clients and peers both call.
- **Signed and wire format changes** bump the version in the same PR, check open PRs have not claimed that number, and name the paired SDK PR with `sdk-ref:` in the body: the sealed wire format ([`crates/sealed`](crates/sealed/src/lib.rs), byte-compatible with mero-js `src/sealed/`) and the attestation check ([`crates/cloud-client/src/attest.rs`](crates/cloud-client/src/attest.rs), mirrored in mero-js `src/cloud-v2/attestation.ts`) change in both or neither.
- **Workflows:** give every workflow explicit least-privilege `permissions`; a privileged workflow (`pull_request_target`, `workflow_run`) never checks out PR code or trusts an artifact a fork can upload - the "Check payload targets the triggering PR" step in [`comment.yml`](.github/workflows/comment.yml).
  Pass `github.*` and `steps.*` values to a `run:` script through `env:`, never `${{ }}`; `Workflow lint` runs `actionlint` and `zizmor` (`zizmor .github/workflows`) and its exceptions are in `.github/zizmor.yml`. `scripts/check-workflow-triggers.py` fails a privileged workflow whose job checks out the PR head without a same-repository `if`, or saves a cache.
- **Secrets** (tokens, keys, credentials) stay in the node's local config under `~/.calimero/<node>/`, outside the repository.

## Commands in docs and CI

Put a tool on PATH once, then call it by its plain name at every call site; `cargo mero` is the example:

```bash
PATH="$(scripts/setup-cargo-mero.sh):$PATH"   # in CI: ./.github/actions/setup-cargo-mero
cargo mero build --manifest-path apps/kv-store/Cargo.toml
```

Write `cargo mero build` bare where the working directory is already the app: a per-app README documents the command, not the path to itself.

## Commits

Commit subjects and PR titles follow Conventional Commits, `<type>(<scope>): <summary>`, an imperative summary in lower case with no trailing period; [`pr-title-lint.yml`](.github/workflows/pr-title-lint.yml) checks the title, which a squash merge makes the commit subject.
