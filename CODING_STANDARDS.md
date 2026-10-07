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

Give each module a file named after it, and declare its children there: `crates/meroctl/src/cli/app.rs` holds `mod get; mod install;`, and the children live in `crates/meroctl/src/cli/app/get.rs` and `crates/meroctl/src/cli/app/install.rs`.
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

- **Untrusted:** every peer, gossip or stream payload, HTTP caller and app guest, and every field a signature does not cover - [`a_join_key_from_a_sender_the_invitation_does_not_vouch_for_is_not_installed`](crates/context/src/handlers/join_group.rs).
- **Identity** comes from the authenticated channel or a signature; a field in the message is a claim, not an identity - [`init_proof_rejects_wrong_peer_id`](crates/node/primitives/src/sync/wire.rs).
- **Verify before acting:** the check that authorizes a store write, install or network call runs before it - [`a_bundle_deriving_another_id_is_refused_before_anything_is_written`](crates/context/primitives/tests/ensure_application_bytecode.rs).
- **Limits:** every input from outside has a named constant bounding its size, count, time or concurrency, checked before the costly work - [`MAX_PRESENTED_HANDOFFS`](crates/account/src/root_key.rs), [`a_long_handoff_chain_is_refused_before_any_signature_is_checked`](crates/server/src/proof_auth.rs).
- **Ending membership:** every gated operation considers kicked, left, deny-listed, revoked, descoped and inherited actors, and actors from other namespaces - [`is_author_denied_for_context`](crates/governance-store/src/deny_list.rs).
- **Closed input:** refuse unknown fields, variants and versions - [`every_request_body_is_a_closed_set`](crates/server/primitives/tests/deny_unknown_fields.rs).
- **One gate:** enforce a rule in the shared function every caller goes through - [`ProofPolicy::admit`](crates/server/src/proof_auth.rs).
- **Signed format changes** bump the schema version in the same PR, check open PRs have not claimed that number, and name the paired SDK PR with `sdk-ref:` in the body - [`SIGNED_NAMESPACE_OP_SCHEMA_VERSION`](crates/governance-types/src/lib.rs), [`pre_flag_day_namespace_op_version_is_rejected`](crates/governance-types/src/tests.rs).
- **Workflows:** give every workflow explicit least-privilege `permissions`; a privileged workflow (`pull_request_target`, `workflow_run`) never checks out PR code or trusts an artifact a fork can upload - the "Check payload targets the triggering PR" step in [`comment.yml`](.github/workflows/comment.yml).
  Pass `github.*` and `steps.*` values to a `run:` script through `env:`, never `${{ }}`; `Workflow lint` runs `actionlint` and `zizmor` (`zizmor .github/workflows`) and its exceptions are in [`.github/zizmor.yml`](.github/zizmor.yml).
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
