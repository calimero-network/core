# Calimero Core - AI Agent Guidance

Peer-to-peer platform for building collaborative apps with automatic conflict-free (CRDT) sync, encrypted P2P networking, and group-based access control. Apps are written in Rust or JavaScript and compiled to WASM; every node runs the same logic over state that converges automatically. (See [`architecture/`](architecture/index.html) for the authoritative definition.)

- **Type**: Rust monorepo (Cargo workspace)
- **Stack**: Rust 1.88.0, wasmer (WASM), libp2p (P2P), RocksDB
- **Sub-package AGENTS.md**: See [crates/](crates/AGENTS.md), [apps/](apps/AGENTS.md), [tools/](tools/AGENTS.md)

## Two layers of docs: WHAT vs WHY

Read them in this order when you land in an unfamiliar area:

1. **`architecture/` - the WHY and how it all connects.** A static HTML reference
   site explaining the system as a whole: the philosophy, the end-to-end flows,
   and how the crates interconnect. Start at
   [`architecture/system-overview.html`](architecture/system-overview.html), then the
   [protocol reference](architecture/protocol/index.html): the
   [write path](architecture/protocol/write-path.html),
   [receive & apply path](architecture/protocol/receive-path.html),
   [operations & the causal DAG](architecture/protocol/operations.html),
   [state, projection & the root hash](architecture/protocol/projection.html),
   [sync & convergence](architecture/protocol/sync.html), and
   [governance](architecture/protocol/governance.html). The
   [dependency explorer](architecture/dependency-explorer.html) maps crate edges,
   and [`unified-causal-log-cutover-plan.html`](architecture/unified-causal-log-cutover-plan.html)
   is the live migration the `op`/`op-adapter`/`projection`/`authz` crates are
   building toward. These are plain HTML - read the source directly, or open in a
   browser. Treat `architecture/` as the source of truth for *intent*; a per-crate
   `AGENTS.md` explains one crate, `architecture/` explains how they add up.
2. **Per-directory `AGENTS.md` - the WHAT.** Each crate/tool/app dir has one
   describing that unit: its types, entry points, commands, and local gotchas.
   Every `AGENTS.md` has a `CLAUDE.md` symlink beside it so both this tool and
   AGENTS-aware tools auto-load the same guidance.

## Setup Commands

```bash
# Install dependencies & build
cargo build

# Build all (release)
cargo build --release

# Typecheck all
cargo check --workspace

# Test all
cargo test

# Format check
cargo fmt --check

# Lint
cargo clippy -- -A warnings
```

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

### No Dead Code

- **All code in PRs must be used** - no unused functions, variables, imports, or types
- Remove commented-out code blocks before submitting
- If code is for future use, don't include it yet - add it when needed
- Use `#[allow(dead_code)]` only with a comment explaining why (e.g., FFI, test fixtures)
- For detecting and removing dead code: use the **dead-code-cleanup** skill (`.cursor/skills/dead-code-cleanup/SKILL.md`) – it verifies no references before removal and produces a structured report

### Commit Format

```
<type>(<scope>): <short summary>
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, `build`, `ci`, `style`, `revert`

- Imperative present tense ("add" not "added")
- No period, no capitalization

### Filing Issues

When you (or an agent) open an engineering issue, follow this structure - it is
the [`technical_issue`](.github/ISSUE_TEMPLATE/technical_issue.md) template:

1. **Summary** - what is wrong and where (crate/module/flow); observed behavior, not a proposed fix.
2. **Impact** - who/what is affected and how badly: severity, blast radius, and a concrete real-world consequence.
3. **Steps to reproduce** - numbered, minimal steps, with actual vs expected result and any log / failing test / merobox scenario.
4. **Criteria for resolving** - an objective checklist that decides when it is fixed: the specific behavior that must hold, a regression test covering it, and `cargo fmt`/`clippy`/`test` passing.

Keep it scope-focused: one issue per defect, no side investigations. Operator/user
bug reports (system specs, install method) use the separate `bug_report` template.

## Security & Secrets

- **NEVER** commit tokens, keys, or credentials
- Secrets: `~/.calimero/node/config.toml` (local only)
- No `.env` files in repo

## JIT Index (what to open, not what to paste)

### Package Structure

Every crate has its own `AGENTS.md`; [crates/AGENTS.md](crates/AGENTS.md) is the full index. The most-opened directories:

| Directory            | Purpose                       | AGENTS.md                                                  |
| -------------------- | ----------------------------- | ---------------------------------------------------------- |
| `crates/`            | Core library crates (index)   | [crates/AGENTS.md](crates/AGENTS.md)                       |
| `crates/merod/`      | Node daemon binary            | [crates/merod/AGENTS.md](crates/merod/AGENTS.md)           |
| `crates/meroctl/`    | CLI tool                      | [crates/meroctl/AGENTS.md](crates/meroctl/AGENTS.md)       |
| `crates/node/`       | Node orchestration            | [crates/node/AGENTS.md](crates/node/AGENTS.md)             |
| `crates/context/`    | Context lifecycle & governance| [crates/context/AGENTS.md](crates/context/AGENTS.md)       |
| `crates/runtime/`    | WASM execution (wasmer)       | [crates/runtime/AGENTS.md](crates/runtime/AGENTS.md)       |
| `crates/storage/`    | CRDT collections              | [crates/storage/AGENTS.md](crates/storage/AGENTS.md)       |
| `crates/store/`      | RocksDB KV store (+enc, blobs)| [crates/store/AGENTS.md](crates/store/AGENTS.md)           |
| `crates/dag/`        | Causal delta DAG              | [crates/dag/AGENTS.md](crates/dag/AGENTS.md)               |
| `crates/sdk/`        | App development SDK           | [crates/sdk/AGENTS.md](crates/sdk/AGENTS.md)               |
| `crates/server/`     | HTTP/WS/SSE server            | [crates/server/AGENTS.md](crates/server/AGENTS.md)         |
| `crates/network/`    | P2P networking (libp2p)       | [crates/network/AGENTS.md](crates/network/AGENTS.md)       |
| `crates/primitives/` | Shared types (ids, keys, hash)| [crates/primitives/AGENTS.md](crates/primitives/AGENTS.md) |
| `crates/crypto/`     | ECDH shared-key encryption    | [crates/crypto/AGENTS.md](crates/crypto/AGENTS.md)         |
| `apps/`              | Example WASM apps             | [apps/AGENTS.md](apps/AGENTS.md)                           |
| `tools/`             | Dev tools (merodb, abi)       | [tools/AGENTS.md](tools/AGENTS.md)                         |

### Quick Find Commands

```bash
# Search for a function across crates
rg -n "fn function_name" crates/

# Find a struct definition
rg -n "pub struct StructName" crates/

# Find trait implementations
rg -n "impl.*TraitName.*for" crates/

# Find tests for a module
rg -n "#\[test\]" crates/module_name/

# Find all entry points (main.rs)
rg -l "fn main" crates/*/src/

# Find host functions (WASM imports)
rg -n "fn " crates/runtime/src/logic/imports.rs
# Or find implementations:
rg -n "pub fn " crates/runtime/src/logic/host_functions/
```

## Definition of Done

Before creating a PR:

1. `cargo fmt --check` passes
2. `cargo clippy -- -A warnings` passes
3. `cargo test` passes
4. `cargo deny check licenses sources` passes (if modifying dependencies)
5. **Update relevant documentation** at the end of changes – README, AGENTS.md, crate docs, or API docs as needed; docs must be updated no later than one day after merge

## Data Flow Overview

```
Client Request → JSON-RPC Server → WASM Runtime → Storage (CRDTs)
                                        ↓
                             State Delta → DAG → Network (Gossipsub)
                                        ↓
                             Other Nodes receive & apply delta
```

## Core Concepts (Summary)

Grounded in [`architecture/concepts.html`](architecture/concepts.html); read it for the full model.

- **Namespace**: A root group (a group with no parent). The application-instance boundary and identity scope for a node - each namespace has its own Ed25519 keypair, and all its subgroups and contexts share that identity. All groups in a namespace share one governance DAG.
- **Group**: A governance boundary within a namespace. Has members, an inherited application, and one or more contexts. Membership, access control, and upgrades happen here via signed governance ops that propagate over P2P gossip; every group has at least one Admin.
- **Context**: A running instance of a WASM application with its own isolated state, kept in sync across context members via CRDT replication. Belongs to exactly one group (32-byte `ContextId`).
- **CRDTs**: Automatic conflict resolution - `GCounter`, `PnCounter`, `LwwRegister<T>`, `UnorderedMap<K,V>`, `UnorderedSet<T>`, `Vector<T>`, `ReplicatedGrowableArray` (see [crates/storage/AGENTS.md](crates/storage/AGENTS.md)).
- **DAG**: Causal ordering of governance ops and state deltas via parent references. Governance ops are either cleartext `RootOp`s (group creation, member join, key delivery) or encrypted `GroupOp`s (membership, capabilities).
- **Gossipsub**: libp2p P2P broadcast; governance ops and deltas propagate per namespace/context topic.

## Running Local Nodes

```bash
# Initialize and run first node
merod --node node1 init --server-port 2428 --swarm-port 2528
merod --node node1 run

# Second node connecting to first
merod --node node2 init --server-port 2429 --swarm-port 2529
merod --node node2 config --swarm-addrs /ip4/127.0.0.1/tcp/2528
merod --node node2 run

# Debug logging
RUST_LOG=debug merod --node node1 run
```

## Building WASM Apps

```bash
# Add WASM target
rustup target add wasm32-unknown-unknown

# Build specific app
cargo build -p kv-store --target wasm32-unknown-unknown --release

# Build all apps
./scripts/build-all-apps.sh
```
