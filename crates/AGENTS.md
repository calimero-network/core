# Crates Directory - AI Agent Guidance

Core library crates for Calimero infrastructure. Each crate is conceptually separate.

## Crate Categories

### Binary Crates (executables)

| Crate     | Binary         | Entry Point           | Purpose          |
| --------- | -------------- | --------------------- | ---------------- |
| `merod`   | `merod`        | `merod/src/main.rs`   | Node daemon      |
| `meroctl` | `meroctl`      | `meroctl/src/main.rs` | CLI tool         |
| `auth`    | `mero-auth`    | `auth/src/main.rs`    | Auth service     |

### Core Library Crates

| Crate              | Entry Point          | Purpose                   |
| ------------------ | -------------------- | ------------------------- |
| `calimero-node`    | `node/src/lib.rs`    | Node runtime coordination |
| `calimero-runtime` | `runtime/src/lib.rs` | WASM execution (wasmer)   |
| `calimero-storage` | `storage/src/lib.rs` | CRDT collections          |
| `calimero-network` | `network/src/lib.rs` | P2P networking (libp2p)   |
| `calimero-server`  | `server/src/lib.rs`  | HTTP/WS/SSE server        |
| `calimero-context` | `context/src/lib.rs` | Context lifecycle         |
| `calimero-dag`     | `dag/src/lib.rs`     | DAG causal ordering       |
| `calimero-store`   | `store/src/lib.rs`   | KV store (RocksDB)        |
| `calimero-sdk`     | `sdk/src/lib.rs`     | App development SDK       |
| `calimero-projection` | `projection/src/lib.rs` | Deterministic ScopeState projection of the op-log |
| `calimero-authz`   | `authz/src/lib.rs`   | Authorization decision over the unified causal log |
| `calimero-op-adapter` | `op-adapter/src/lib.rs` | Bridges per-plane ops onto the unified causal log |
| `calimero-governance-store` | `governance-store/src/lib.rs` | Local group-governance apply pipeline & broadcast |
| `calimero-tee-attestation` | `tee-attestation/src/lib.rs` | TEE (TDX) attestation generation & verification |
| `calimero-wasm-abi` | `wasm-abi/src/lib.rs` | WASM ABI schema: `AbiType`, validate, embed |

### Support Crates

| Crate                 | Purpose                                                         |
| --------------------- | --------------------------------------------------------------- |
| `calimero-primitives` | Shared types: `ContextId`, `ApplicationId`, `PublicKey`, `Hash` |
| `calimero-bundle`     | `.mpk` manifest types + signature canonicalization, shared by `calimero-node-primitives`, `cargo-mero`, `mero-sign` |
| `calimero-crypto`     | Cryptographic utilities                                         |
| `calimero-config`     | Configuration parsing                                           |
| `calimero-client`     | HTTP/WS client for nodes                                        |
| `calimero-account`    | Account/device identity primitive (ids, certs, key chains)      |
| `calimero-op`         | Unified op envelope types + id/root hashing                     |
| `calimero-governance-types` | Signed group-operation types (local governance)           |

## How the Crates Fit Together

Three views the tables above cannot give you: which crate may import which, which
crate owns each hop of a live request, and how the four unified-causal-log crates
relate to each other.

Everything else — the actor model, the host ABI boundary, the DAG fold, the
column-family layout, the scope tree — is already drawn as SVG on the docs site
under [`docs/src/components/diagrams/`](../docs/src/components/diagrams/), wired
into the [architecture orientation](../docs/src/content/docs/contribute/architecture.mdx)
and the [protocol reference](../docs/src/content/docs/protocol/overview.mdx).
Link to those rather than redrawing them here; two copies of a diagram drift.

### Dependency spine

The load-bearing edges only — the full graph is roughly 90. Arrows point *from*
the dependent *to* what it depends on. Colours match the six bands in the docs
site's `CrateStack` figure, which shows the same layering without the edges.

```mermaid
flowchart TD
  merod["merod"] --> node["calimero-node"]
  merod --> server["calimero-server"]
  merod --> store["calimero-store"]
  meroctl["meroctl"] --> client["calimero-client"]
  auth["mero-auth"] --> store
  client -. "HTTP / WS" .-> server
  server --> ctxc["calimero-context-client<br/><i>context/primitives</i>"]
  server --> ctx["calimero-context"]
  node --> ctx
  node --> net["calimero-network"]
  node --> rt["calimero-runtime"]
  node --> store
  ctx --> ctxc
  ctx --> rt
  ctx --> gov["calimero-governance-store"]
  ctx --> opad["calimero-op-adapter"]
  ctx --> dag["calimero-dag"]
  ctxc --> nodep["calimero-node-primitives"]
  gov --> govt["calimero-governance-types"]
  gov --> proj["calimero-projection"]
  opad --> op["calimero-op"]
  opad --> authz["calimero-authz"]
  proj --> op
  authz --> op
  dag --> storage["calimero-storage"]
  store --> storage
  rt --> sys["calimero-sys"]
  storage --> sdk["calimero-sdk"]
  sdk --> sys
  op --> acct["calimero-account"]
  acct --> prim["calimero-primitives"]
  storage --> prim
  net --> prim

  classDef bin  fill:#fdf3d9,stroke:#a37b12,color:#3a2a02
  classDef srv  fill:#f1fadd,stroke:#6f8f22,color:#26300a
  classDef core fill:#e3f7e6,stroke:#2f8f3e,color:#0f2a14
  classDef infra fill:#e4effb,stroke:#3f7ec0,color:#0d2440
  classDef log  fill:#ddf5f6,stroke:#2b9aa1,color:#062c2e
  classDef base fill:#efe7fb,stroke:#7b52c0,color:#1f0f38
  class merod,meroctl bin
  class server,client,auth,ctxc srv
  class node,ctx core
  class net,rt,store,storage infra
  class dag,op,opad,proj,authz,gov,govt log
  class sdk,sys,acct,prim,nodep base
```

What the shape tells you:

- Everything converges on `calimero-primitives`. If you are adding a type two
  crates both need, that is where it goes — see the Primitives Crates Pattern below.
- `calimero-network` never imports application-level types. It ferries opaque
  bytes and hands them up as `NetworkEvent`; decoding happens in the node layer.
- `calimero-context-client` does **not** depend on `calimero-context`. It holds a
  message `Recipient`, which is what keeps the two out of a dependency cycle.
- The unified-log crates (`op`, `op-adapter`, `projection`, `authz`) sit *below*
  `context` and `governance-store`, not beside them.

### Life of one write

Which crate owns each hop when a JSON-RPC `execute` arrives. Worth noting because
it surprises people: the server does **not** route through `NodeManager` on the
way in — it calls `ContextClient::execute` directly. The node layer is on the
*outbound* broadcast side.

```mermaid
sequenceDiagram
  autonumber
  participant CL as client / meroctl
  participant SV as server
  participant CC as context-client
  participant CX as context
  participant RT as runtime
  participant ST as storage + store
  participant ND as node
  participant NW as network
  CL->>SV: JSON-RPC execute
  SV->>CC: ContextClient::execute
  CC->>CX: ContextMessage::Execute
  CX->>CX: context.lock()
  CX->>RT: module.run_with_origin(method, args)
  RT->>ST: host functions: storage read / write
  ST-->>RT: CRDT entities
  RT-->>CX: Outcome {returns, logs, events, artifact}
  CX->>CX: sign_authorized_actions
  CX->>ST: persist, new root_hash
  CX->>ND: NodeClient::broadcast(artifact, delta_id, parent_ids, hlc)
  ND->>NW: publish on the context topic
  NW-->>ND: peers: NetworkEvent (the receive path, mirrored)
```

### Life of a read

Same entry point — there is no separate query RPC — but the path forks early and
never reaches the node or network layer at all. A method the app declared
`#[app::view]` is looked up in the ABI's read-only set, keyed by the executing
bytecode blob, and any miss falls back to the exclusive write lock.

```mermaid
sequenceDiagram
  autonumber
  participant CL as client / meroctl
  participant SV as server
  participant CC as context-client
  participant CX as context
  participant RT as runtime
  participant ST as storage + store
  CL->>SV: JSON-RPC execute of a view method
  SV->>CC: ContextClient::execute
  CC->>CX: ContextMessage::Execute
  CX->>CX: read_only_methods lookup, keyed by executing blob
  Note over CX: a miss falls back to the write lock (fail-safe)
  CX->>CX: context.lock_read() — shared, reads do not serialise
  CX->>RT: module.run_with_origin(.., ReadOnlyContextStorage)
  RT->>ST: storage reads, write host-calls silenced
  ST-->>RT: CRDT entities
  RT-->>CX: Outcome {returns, logs, events}
  CX->>CX: assert no mutation, else discard + warn (ABI mismatch)
  CX-->>SV: returns
  SV-->>CL: JSON-RPC result
```

Three defences stack here, and it is worth knowing all three exist: the shared
`lock_read()` lets concurrent reads run without serialising; `ReadOnlyContextStorage`
silences write host-calls at the boundary; and if a mutation nonetheless comes
back, `context` discards it rather than committing. The absence of the last four
steps of the write path — sign, persist, broadcast — is the whole difference.

### Receiving a delta

What happens on the other side of that broadcast. Note the shape: authorization
happens *before* decryption, and a delta that arrives before the governance state
that authorizes it is buffered rather than dropped.

```mermaid
flowchart TD
  NW["network<br/>gossipsub: BroadcastMessage::StateDelta"] --> NE["node<br/>handlers/network_event.rs"]
  NE --> RO{"sender still a writer?"}
  NE -. "state-delta mailbox full: drop;<br/>peer heartbeat rebroadcast retries" .-> DROP(["dropped"])
  RO -->|"no"| REJ(["reject"])
  RO -->|"yes"| DRAIN["drain the governance-pending buffer"]
  DRAIN --> AUTH{"authorize_delta_at_edge_projected<br/>at the sender's governance edge"}
  AUTH -->|"governance state<br/>behind the edge"| BUF["buffer as governance-pending"]
  AUTH -->|"unauthorized"| REJ
  AUTH -->|"authorized"| SIG["verify_delta_signature"]
  SIG --> KEY["look up the group key<br/>waits, within a bounded window"]
  KEY --> DEC["decrypt_delta_actions<br/>artifact + nonce to actions,<br/>expected root hash, events"]
  DEC --> DAG{"dag.add_delta_with_events<br/>parents present?"}
  DAG -->|"no"| PEND["pend in the DAG,<br/>request_missing_deltas"]
  DAG -->|"yes"| APPLY["ContextStorageApplier::apply"]
  APPLY --> EXEC["context.execute, method __calimero_sync_next"]
  EXEC --> STO["storage merge, new root hash"]
  STO --> EV["event handlers, WebSocket re-emit"]
  BUF -. "later governance ops<br/>drain the buffer" .-> DRAIN

  classDef net  fill:#e4effb,stroke:#3f7ec0,color:#0d2440
  classDef ok   fill:#e3f7e6,stroke:#2f8f3e,color:#0f2a14
  classDef hold fill:#fdf3d9,stroke:#a37b12,color:#3a2a02
  classDef bad  fill:#fbe6e6,stroke:#b34d4d,color:#3d0f0f
  class NW,NE net
  class SIG,KEY,DEC,APPLY,EXEC,STO,EV ok
  class BUF,PEND hold
  class REJ,DROP bad
```

The last hop is the one to internalise: **applying a peer's delta re-enters the
same execute path as a local write**, with the method name `__calimero_sync_next`.
That is why `crates/context/src/handlers/execute/mod.rs` branches on `is_state_op`
throughout — same code, two callers — and why an applied delta does not
re-broadcast.

### When sync kicks in

Gossip is best-effort, so deltas get dropped, arrive out of order, or miss a peer
that was offline. `SyncManager` (`crates/node/src/sync/`, a long-lived task rather
than an actor) is the backstop that reconciles whatever gossip missed.

```mermaid
flowchart TD
  T1["periodic sync interval"] --> DISP
  T2["HashHeartbeat divergence<br/>DAG heads or root hash differ"] --> DISP
  T3["namespace / open-subgroup join"] --> DISP
  DISP{"SessionTracker::dispatch_decision<br/>backoff, wedge-watchdog,<br/>at most one sync per context per cycle"}
  DISP -->|"skip"| SKIP(["skip this cycle"])
  DISP -->|"dispatch"| SESS["sync session over an encrypted libp2p stream"]
  SESS --> HS["handshake: exchange root hash + DAG heads"]
  HS --> SEL{"select_protocol<br/>from the divergence metrics"}
  SEL -->|"roots match"| DONE(["converged, no-op"])
  SEL -->|"DeltaSync"| DS["request DAG heads,<br/>pull the missing deltas"]
  SEL -->|"HashComparison"| HC["Merkle DFS<br/>hash_comparison_protocol.rs"]
  SEL -->|"LevelWise"| LW["level-wise BFS, for wide trees<br/>level_sync.rs"]
  SEL -->|"Snapshot"| SN["snapshot transfer<br/>snapshot.rs"]
  HC -. "on failure" .-> DS
  LW -. "on failure" .-> DS
  DS -. "still divergent" .-> SN
  DS --> AP["entities land through the same apply path"]
  HC --> AP
  LW --> AP
  SN --> AP

  classDef trig fill:#f1fadd,stroke:#6f8f22,color:#26300a
  classDef prot fill:#ddf5f6,stroke:#2b9aa1,color:#062c2e
  classDef done fill:#e3f7e6,stroke:#2f8f3e,color:#0f2a14
  class T1,T2,T3 trig
  class DS,HC,LW,SN prot
  class DONE,AP done
```

The dotted edges are a fallback chain, not alternatives: `HashComparison` and
`LevelWise` fall back to DAG-heads sync on failure, and that falls back to a full
snapshot. Snapshot is correct but expensive, so a system that keeps reaching for
it is telling you something. `BloomFilter` and `SubtreePrefetch` appear in the
selector but are not implemented — they fall through to snapshot.

Missing *parents* are a separate, cheaper mechanism: the receive path requests
them directly via `request_missing_deltas` without opening a sync session.

### The unified causal log

Why there are four crates here and not one. `op-adapter` is explicitly
transitional: it exists to map the older per-plane operation types onto the
unified payload, and goes away once everything speaks `Op` natively.

```mermaid
flowchart LR
  A["app write<br/>storage action"] --> AD
  G["governance op<br/>governance-types"] --> AD
  AD["op-adapter<br/><i>per-plane to unified</i>"] --> OP["op<br/>Op envelope,<br/>id + root hashing"]
  OP --> DG["dag<br/>causal order, heads"]
  DG --> PR["projection<br/>ScopeState fold"]
  PR --> RH(["root hash"])
  PR -. "ACL view at the causal cut" .-> AZ["authz<br/>authorize()"]
  AZ -. "accept / reject" .-> AD

  classDef in  fill:#f1fadd,stroke:#6f8f22,color:#26300a
  classDef log fill:#ddf5f6,stroke:#2b9aa1,color:#062c2e
  classDef out fill:#fdf3d9,stroke:#a37b12,color:#3a2a02
  class A,G in
  class AD,OP,DG,PR,AZ log
  class RH out
```

The dotted back-edge is the part to internalise: **authorization reads the
projection, and the projection is built from the log it authorizes into.** That
loop is why the ordering rules are subtle, and why `authorize` resolves its ACL
view at the op's causal cut rather than at the current head.

## Patterns & Conventions

### Primitives Crates Pattern

Shared types go in `*-primitives` crates to avoid circular dependencies:

```
context/primitives/  → calimero-context-client
node/primitives/     → calimero-node-primitives
network/primitives/  → calimero-network-primitives
server/primitives/   → calimero-server-primitives
```

### Config Crates Pattern

Configuration types often in separate `*-config` crates:

```
context/config/      → calimero-context-config
```

### Actix Actors

Node components use actix actor framework for async coordination:

- ✅ See pattern: `node/src/handlers/network_event.rs`
- ✅ Actor definitions: `node/src/lib.rs`

### File Organization

```rust
// src/lib.rs - exports and top-level types
pub mod handlers;
pub mod sync;
mod constants;
mod utils;

// Each handler in separate file
// src/handlers/network_event.rs
// src/handlers/state_delta.rs
```

## Common Dependencies

```toml
# Error handling
eyre = "0.6"

# Async runtime
tokio = "1.47"
actix = "0.13"

# Serialization
borsh = "1.3"      # Binary (storage)
serde = "1.0"      # JSON (API)

# Networking
libp2p = "0.56"

# WASM
wasmer = "6.1"

# Storage
rocksdb = "0.24"
```

## Commands

```bash
# Build specific crate
cargo build -p calimero-node

# Test specific crate
cargo test -p calimero-node

# Test with output
cargo test -p calimero-dag test_dag_out_of_order -- --nocapture

# Run SDK macro tests (compile-time)
cargo test -p calimero-sdk-macros
```

## JIT Index

### Find Functions

```bash
# Find host functions
rg -n "pub fn " runtime/src/logic/host_functions/

# Find handlers
rg -n "pub async fn handle" node/src/handlers/

# Find API endpoints
rg -n "pub async fn " server/src/admin/
```

### Find Types

```bash
# Find struct definitions
rg -n "pub struct" primitives/src/

# Find enums
rg -n "pub enum" -A5 context/primitives/src/

# Find trait definitions
rg -n "pub trait" storage/src/
```

## Sub-Package AGENTS.md

Every crate has its own `AGENTS.md`. Binaries and services:

- [merod/AGENTS.md](merod/AGENTS.md) - Node daemon
- [meroctl/AGENTS.md](meroctl/AGENTS.md) - CLI tool
- [auth/AGENTS.md](auth/AGENTS.md) - `mero-auth` auth service

Core libraries:

- [node/AGENTS.md](node/AGENTS.md) - Node orchestration
- [runtime/AGENTS.md](runtime/AGENTS.md) - WASM runtime
- [storage/AGENTS.md](storage/AGENTS.md) - CRDT collections
- [store/AGENTS.md](store/AGENTS.md) - RocksDB KV store (+ encryption, blobs)
- [sdk/AGENTS.md](sdk/AGENTS.md) - App SDK
- [server/AGENTS.md](server/AGENTS.md) - HTTP/WS server
- [network/AGENTS.md](network/AGENTS.md) - P2P networking
- [context/AGENTS.md](context/AGENTS.md) - Context lifecycle & local governance
- [client/AGENTS.md](client/AGENTS.md) - HTTP/WS client for nodes
- [dag/AGENTS.md](dag/AGENTS.md) - DAG causal ordering

Unified causal log & governance:

- [account/AGENTS.md](account/AGENTS.md) - Account/device identity primitive
- [op/AGENTS.md](op/AGENTS.md) - Unified op envelope + id/root hashing
- [op-adapter/AGENTS.md](op-adapter/AGENTS.md) - Per-plane ops onto the unified log
- [projection/AGENTS.md](projection/AGENTS.md) - Deterministic ScopeState projection
- [authz/AGENTS.md](authz/AGENTS.md) - Authorization over the causal log
- [governance-types/AGENTS.md](governance-types/AGENTS.md) - Signed group-op types
- [governance-store/AGENTS.md](governance-store/AGENTS.md) - Local governance apply pipeline

Foundations & support:

- [primitives/AGENTS.md](primitives/AGENTS.md) - Shared types (`ContextId`, `PublicKey`, `Hash`)
- [bundle/AGENTS.md](bundle/AGENTS.md) - `.mpk` manifest types & signature canonicalization
- [crypto/AGENTS.md](crypto/AGENTS.md) - ECDH shared-key encryption
- [config/AGENTS.md](config/AGENTS.md) - Node configuration parsing
- [sys/AGENTS.md](sys/AGENTS.md) - WASM host ABI types
- [wasm-abi/AGENTS.md](wasm-abi/AGENTS.md) - WASM ABI schema emit/validate/embed
- [tee-attestation/AGENTS.md](tee-attestation/AGENTS.md) - TEE (TDX) attestation
- [prelude/AGENTS.md](prelude/AGENTS.md) - Shared root-storage-key prelude
- [storage-macros/AGENTS.md](storage-macros/AGENTS.md) - Storage derive macros
- [build-utils/AGENTS.md](build-utils/AGENTS.md) - build.rs version/git helpers
- [git-hooks/AGENTS.md](git-hooks/AGENTS.md) - Self-installing pre-commit hook
- [utils/AGENTS.md](utils/AGENTS.md) - `calimero-utils-actix` actor helpers
