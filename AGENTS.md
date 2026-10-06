# Calimero Core - AI Agent Guidance

Peer-to-peer platform for building collaborative apps with automatic conflict-free (CRDT) sync, encrypted P2P networking, and group-based access control. Apps are written in Rust or JavaScript and compiled to WASM; every node runs the same logic over state that converges automatically.

## Where to read

`docs/` explains why the system works as it does and how the crates add up; a directory's `AGENTS.md` explains that one unit.

- **Intent**: before changing a flow that spans crates, read the [protocol overview](docs/src/content/docs/protocol/overview.mdx) and the chapters it names, all under `docs/src/content/docs/protocol/`.
- **Directory**: before working in a crate, app or tool, read the `AGENTS.md` in its directory (the `CLAUDE.md` beside it is a symlink); [crates/AGENTS.md](crates/AGENTS.md) indexes the crates and [architecture](docs/src/content/docs/contribute/architecture.mdx) shows how they connect.
- **Glossary**: when a protocol term (namespace, group, context, scope, operation) is unclear, read the [glossary](docs/src/content/docs/protocol/glossary.mdx).
- **Nodes**: before running `merod` by hand, read [Run local nodes](docs/src/content/docs/contribute/development.mdx#run-local-nodes).
- **Issues**: before filing an engineering issue, fill in the [`technical_issue`](.github/ISSUE_TEMPLATE/technical_issue.md) template.

## Setup Commands

A pre-commit hook (`cargo fmt --check` on staged Rust files) installs itself on
any `cargo build`/`cargo test` via the `calimero-git-hooks` build script - no
husky/pnpm needed, and it works from git worktrees. Sources live in `.githooks/`.

## Universal Conventions

### Import Organization (StdExternalCrate Pattern)

```rust
// 1. Standard library
use std::collections::HashMap;
use std::sync::Arc;

// 2. External crates
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// 3. Local crate & parent module
use crate::{common, Node};
use super::Shared;

// 4. Local modules
mod config;
mod types;
```

### Module Organization

Do NOT use `mod.rs`. Export modules from named files:

```
crates/meroctl/src/cli/app.rs       # Contains: mod get; mod install;
crates/meroctl/src/cli/app/get.rs
crates/meroctl/src/cli/app/install.rs
```

**Exceptions:** Rare exceptions exist for specific technical reasons (e.g., `crates/node/src/sync/mod.rs` - see [crates/node/AGENTS.md](crates/node/AGENTS.md)). New `mod.rs` files should only be created with explicit justification and documentation of the exception.

### Error Handling

- Use `eyre` crate: `use eyre::Result as EyreResult;`
- Avoid `.unwrap()` / `.expect()` - use `.map_err()` or `?`
- Comment if unwrap is safe: `// SAFETY: guaranteed by X`

### Invoking cargo mero

Put the tool on PATH once, then call it like any cargo subcommand. Never spell out
`cargo run -q -p cargo-mero -- mero ...` at a call site.

```bash
PATH="$(scripts/setup-cargo-mero.sh):$PATH"   # in CI: ./.github/actions/setup-cargo-mero
cargo mero build --manifest-path apps/kv-store/Cargo.toml
```

Drop `--manifest-path` where the working directory is already the app - a per-app
README documents `cargo mero build`, not the path to itself.

### No Dead Code

- **All code in PRs must be used** - no unused functions, variables, imports, or types
- If code is for future use, don't include it yet - add it when needed
- Use `#[allow(dead_code)]` only with a comment explaining why (e.g., FFI, test fixtures)
- For detecting and removing dead code: use the **dead-code-cleanup** skill (`.cursor/skills/dead-code-cleanup/SKILL.md`) – it verifies no references before removal and produces a structured report
- For code that is *referenced* but unreachable – a dead island whose items cite
  each other while the outermost edge points at a route, command or registration
  that does not exist: use the **unreachable-subsystems** skill
  (`.cursor/skills/unreachable-subsystems/SKILL.md`). The compiler and clippy
  cannot see this class, because every item genuinely has a caller

### Commit Format

```
<type>(<scope>): <short summary>
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, `build`, `ci`, `style`, `revert`

- No period, no capitalization

## Security: trust boundaries

A PR touching a path in `TRUST_BOUNDARY_PATHS` ([`scripts/check-trust-boundary.py`](scripts/check-trust-boundary.py)) must answer the PR template's Trust boundary block; CI fails without it.
Fixing a finding: the **calimero-security-fix** skill ([`.cursor/skills/calimero-security-fix/SKILL.md`](.cursor/skills/calimero-security-fix/SKILL.md)).

- **Untrusted:** every peer, gossip or stream payload, HTTP caller and app guest, and every field a signature does not cover - [`a_join_key_from_a_sender_the_invitation_does_not_vouch_for_is_not_installed`](crates/context/src/handlers/join_group.rs).
- **Identity** comes from the authenticated channel or a signature, never from a field in the message - [`init_proof_rejects_wrong_peer_id`](crates/node/primitives/src/sync/wire.rs).
- **Verify before acting:** no store write, install or network call before the check that authorizes it - [`a_bundle_deriving_another_id_is_refused_before_anything_is_written`](crates/context/primitives/tests/ensure_application_bytecode.rs).
- **Limits:** every input from outside has a named constant bounding its size, count, time or concurrency, checked before the costly work - [`MAX_PRESENTED_HANDOFFS`](crates/account/src/root_key.rs), [`a_long_handoff_chain_is_refused_before_any_signature_is_checked`](crates/server/src/proof_auth.rs).
- **Ending membership:** every gated operation considers kicked, left, deny-listed, revoked, descoped and inherited actors, and actors from other namespaces - [`is_author_denied_for_context`](crates/governance-store/src/deny_list.rs).
- **Closed input:** refuse unknown fields, variants and versions instead of defaulting them - [`every_request_body_is_a_closed_set`](crates/server/primitives/tests/deny_unknown_fields.rs).
- **One gate:** enforce a rule in the shared function every caller goes through, not in the one path a report names - [`ProofPolicy::admit`](crates/server/src/proof_auth.rs).
- **Signed format changes** bump the schema version in the same PR, check open PRs have not claimed that number, and name the paired SDK PR with `sdk-ref:` in the body - [`SIGNED_NAMESPACE_OP_SCHEMA_VERSION`](crates/governance-types/src/lib.rs), [`pre_flag_day_namespace_op_version_is_rejected`](crates/governance-types/src/tests.rs).
- **Workflows:** never check out PR code in a privileged workflow (`pull_request_target`, `workflow_run`), never trust an artifact a fork can upload, and give every workflow explicit least-privilege `permissions` - the "Check payload targets the triggering PR" step in [`comment.yml`](.github/workflows/comment.yml).

### Secrets

- Secrets: `~/.calimero/node/config.toml` (local only)

## Testing

- **Reproduce first**: write the failing test at the layer a user hits the bug and watch it go red before changing code.
- **Blind spot**: merobox E2E runs one build on fresh state, so it never tests backward compatibility; an on-disk or wire format change needs its own migration test.
- **Test guide**: before writing a test or merobox scenario, reading node logs, or debugging a CI failure, read [Testing strategy](docs/src/content/docs/contribute/testing.mdx).

## Definition of Done

**Run `./scripts/check-like-ci.py` rather than the list below by hand.**
It reads `.github/workflows/ci-checks.yml` and runs the steps of every job the required `Rust` check waits on, continuing past a failure the way CI's `if: !cancelled()` does.
A hand-kept copy of the list drifts toward running less than CI.

Before creating a PR:

1. `cargo clippy --workspace --all-targets --features calimero-storage/testing -- -D warnings` passes.
   Run it with `-D`, the way CI does: `-A warnings` allows every lint, so it can only ever pass.
   `merod` also gets a second pass under `--features mock-attestation`, which CI runs separately.
2. `cargo nextest run --workspace` and `cargo test --workspace --doc` pass (CI runs the tests under nextest)
3. `cargo deny check licenses sources` passes (if modifying dependencies)
4. **Update relevant documentation** at the end of changes – README, AGENTS.md, crate docs, or API docs as needed; docs must be updated no later than one day after merge
5. **Prove it works.** For a bug fix, the PR description must show the fix works: the reproduction (command / test / merobox scenario), and before→after evidence (the failing log line or test output before, the passing result after). A fix with no reproduction and no regression test is not done.

### Review & merge gate

A PR is mergeable only when all of these hold - this is the closed loop:

- **CI all green** (merobox E2E, sync-regression where triggered, SDK e2e, lint, wire-contract gate).
- **Automated review addressed**: meroreviewer and Cursor Bugbot comments are either fixed or explicitly answered, and their threads resolved. A clean/LGTM pass with no open threads is the signal.
- **Human review** approved where required by branch protection.

Working in Claude Code, drive this loop with the skills instead of by hand:

- `systematic-debugging` - reproduce-first before proposing a fix.
- `resolve-bot-review-comments` - triage/fix/resolve Bugbot / meroreviewer / CodeRabbit threads (filters real findings from nits, then resolves).
- `babysit` - watch the PR and CI until green and all actionable review is resolved.
- `security-review` / `code-review` - self-review the diff before requesting review.
