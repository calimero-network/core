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
- **One version per crate:** `cargo deny check bans` fails on a crate in two versions. Unify it (`cargo update -p <crate>`, or move our dependency to the version upstream uses); only when a dependency pins the other version, add a `skip` to `deny.toml` naming that dependency.

## Modules

Give each module a file named after it, and declare its children there: `crates/node/src/node.rs` holds `mod component;`, `crates/node/src/node/component.rs` holds `mod compute; mod files; mod uploads;`, and the children live in `crates/node/src/node/component/compute.rs`, `files.rs` and `uploads.rs`.
The one exception is a helper module that several integration-test binaries share, `tests/common/mod.rs` (the `apps/*/tests/common` helpers): Cargo builds every `tests/*.rs` as a test binary of its own, so a `tests/common.rs` would be one.
A helper that only one test binary uses is a module of that binary, beside its root (`crates/node/tests/it/common.rs`).
`scripts/check-naming.sh` fails on any other `mod.rs`.

Place a helper shared across a crate in a `commons.rs`-style module, and a type shared across crates in that area's `primitives` crate.

## Errors and panics

Return errors with `?`, mapping them into the crate's error type with `.map_err()`.
Crates use `eyre`, imported as `use eyre::Result as EyreResult;`.
`deny.toml` bans `anyhow`; the one exception is `calimero-tee-attestation`, whose collateral transports implement dcap-qvl's `HttpClient`, a trait that returns `anyhow::Result`.

A panic point (`.unwrap()`, `.expect()`, `assert!`, `panic!`) is reserved for an invariant that cannot fail, or a failure where aborting the node is the right response.
State that invariant beside it: `// SAFETY: guaranteed by X`.
Write it as `.expect("...")`: clippy refuses `.unwrap()` outside tests (`unwrap_used`, in the root `Cargo.toml`'s `[workspace.lints]`, which every member takes with `[lints] workspace = true`; `clippy.toml` exempts `#[test]` functions, and an integration test or bench allows it at its crate root).

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

- **Untrusted:** every peer, HTTP caller, registry download and app guest, and every field a signature or an attestation does not cover - [`clients_refuse_unattested_nodes_and_plain_requests`](crates/node/tests/it/component_cluster.rs).
- **Identity** comes from a verified credential or signature; a field in the request is a claim, not an identity - `principal` in [`crates/node/src/api.rs`](crates/node/src/api.rs), which derives the caller from the session token or API key alone.
- **Verify before acting:** the check that authorizes a store write, install or network call runs before it - [`an_uploaded_package_is_verified_and_reaches_the_other_nodes`](crates/node/tests/it/cluster.rs).
- **Limits:** every input from outside has a named constant bounding its size, count, time or concurrency, checked before the costly work - [`MAX_PENDING_CHALLENGES`](crates/node/src/api.rs), [`MAX_STATE_BYTES`](crates/node/src/presence.rs) and [`oversized_state_is_refused`](crates/node/src/presence.rs).
- **Ending membership:** every gated operation considers removed, departed, revoked, read-only and inherited actors, and actors from other organizations, and judges them on fresh system state - [`teams_work_like_v1_subgroups`](crates/node/tests/it/component_cluster.rs), `gate` in [`crates/node/src/api.rs`](crates/node/src/api.rs).
- **Closed input:** refuse unknown fields, variants and versions - the sealed envelope's `RequestHead` in [`crates/sealed/src/server.rs`](crates/sealed/src/server.rs), and `merod`'s `NodeArgs` ([`crates/merod/src/node_config.rs`](crates/merod/src/node_config.rs)).
- **One gate:** enforce a rule in the shared function every caller goes through - `attest::verify` in [`crates/client/src/attest.rs`](crates/client/src/attest.rs), which clients and peers both call.
- **Log entries:** a new command or field ships with a feature gate. A new system or app log command, or a new field that changes what an existing one does, is a [`Feature`](crates/node/src/system/cluster_version.rs) with the release it ships in (`FEATURES`, `Feature::since`), named in `SystemCmd::features` (exhaustive: a new command does not compile until it says), and is never proposed below the cluster version: `Node::sys_write` refuses it `409 FEATURE_NOT_ACTIVE`, and a route that asks for a passkey checks `Node::require_features` first. A node on an older release skips what it cannot decode, so an ungated entry splits the replicas' state - [`a_feature_is_never_proposed_while_an_older_node_holds_the_log`](crates/node/src/system/cluster_version.rs).
  A field that only records which rule an entry was judged by (serde default `false`, set by the API) decodes on older releases and is not a feature, but says so beside it.
- **Signed and wire format changes** bump the version in the same PR, check open PRs have not claimed that number, and change the matching client in `sdks/clients/` in the same PR: the sealed wire format ([`crates/sealed`](crates/sealed/src/lib.rs), byte-compatible with mero-js `src/sealed/`) and the attestation check ([`crates/client/src/attest.rs`](crates/client/src/attest.rs), mirrored in mero-js `src/runtime/attestation.ts`) change in both or neither.
- **Workflows:** give every workflow explicit least-privilege `permissions`; a privileged workflow (`pull_request_target`, `workflow_run`) never checks out PR code or trusts an artifact a fork can upload - the "Check payload targets the triggering PR" step in [`comment.yml`](.github/workflows/comment.yml).
  Pass `github.*` and `steps.*` values to a `run:` script through `env:`, never `${{ }}`; `Workflow lint` runs `actionlint` and `zizmor` (`zizmor .github/workflows`) and its exceptions are in `.github/zizmor.yml`. `scripts/check-workflow-triggers.py` fails a privileged workflow whose job checks out the PR head without a same-repository `if`, or saves a cache.
- **Secrets** (tokens, keys, credentials) stay in the node's local config under `~/.calimero/<node>/`, outside the repository.

## Documentation in code

Rustdoc, comments and `AGENTS.md` files say what the code does and how; the [docs site](https://calimero-network.github.io/core/protocol/overview/) says why.
A reader must be able to follow every reference to its source in one click.

- **Crate docs:** every `lib.rs` and `main.rs` opens with `//!`: what the crate is, a `Spec:` link to the docs chapter(s) it implements, and its `AGENTS.md`.
- **Module docs:** a module that implements a protocol chapter links that chapter in its `//!`.
- **Item docs:** a normative item links the exact heading that states its rule: a format version (`STORE_FORMAT`, `ENTRY_FORMAT`, `SNAPSHOT_FORMAT`), a wire or stored type, a `Feature`, an error code, an invariant. Other items get rustdoc without a link.
- **Link the site in full:** write the literal URL with its heading, so it opens from the editor, rust-analyzer's hover and rustdoc, and grep finds it. Never a macro or a URL constant, never an `.mdx` path, never a path the site only redirects (`redirects` in `docs/astro.config.mjs`).

  ```rust
  //! Spec: [Replication](https://calimero-network.github.io/core/protocol/replication/)
  /// Raised per [Changing a stored format](https://calimero-network.github.io/core/operate/compatibility/#changing-a-stored-format).
  // Spec: https://calimero-network.github.io/core/protocol/upgrades/#the-cluster-version
  ```

- **Link, do not copy:** a comment does not repeat a page's prose. When a comment needs more than a sentence of why, the page is missing it: add it there and link it.
- **No bare ids:** `(D20)`, `R8`, `C10`, `review finding F6`, `Ruling 39` and `Bugbot on #5210` lead nowhere. State the rule in words and link the page that states it; link the spec row in `docs/specs/` for the reasoning behind a decision; drop a review finding's id, since the regression test's name says what it guards and git blame leads to the PR.
- **No issue numbers:** a bare `#4723` is not a link. Drop it where it records history; write the full issue URL for open work (`https://github.com/calimero-network/core/issues/4736`).
- **Present tense:** a comment states what holds now. "It used to", "previously" and "was renamed" belong in the PR description. A regression test's doc says what must hold, not the bug it caught.
- **Paths that exist:** write a repository path from the root (`crates/node/tests/it/consent_page.rs`). Moving or deleting a file, grep for its path and fix every comment that names it; renaming a heading in `docs/`, grep for its anchor.
- **Enforced** by [`scripts/check-code-refs.py`](scripts/check-code-refs.py) (the `Code references` workflow): site links and paths must resolve (`scripts/check-code-refs.allow` lists the few that are right as written), and no file may gain a bare id or issue number (`scripts/check-code-refs.baseline` holds today's counts per file; they only go down). Editing a comment that carries one, rewrite it, then run `python3 scripts/check-code-refs.py --update-baseline`.

## Commands in docs and CI

Put a tool on PATH once, then call it by its plain name at every call site; `meroctl` is the example:

```bash
cargo build --release -p meroctl && PATH="$PWD/target/release:$PATH"
meroctl app sign manifest.json --dev
```

Write a command bare where the working directory is already the app: a per-app README documents the command, not the path to itself.

## Commits

Commit subjects and PR titles follow Conventional Commits, `<type>(<scope>): <summary>`, an imperative summary in lower case with no trailing period; [`pr-title-lint.yml`](.github/workflows/pr-title-lint.yml) checks the title, which a squash merge makes the commit subject.
