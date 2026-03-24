# Local group governance (Proposal 1) — Phase 0 design

This document fixes **decisions and scope** before implementation. It describes **signed group operations** replicated over the existing **per-group gossip topic** (`group/<group_id_hex>`), with **local materialization** into node storage (`group_store`), as an alternative to **NEAR**-backed group state.

**Branch:** `feat/local-group-governance-ops`  
**Related:** [GROUP-FEATURE-OVERVIEW.md](./GROUP-FEATURE-OVERVIEW.md) (product behavior; chain-oriented sections will gain a “local governance” counterpart).

---

## 1. Problem statement

Today, group metadata and permissions are **synchronized from on-chain** contracts (`sync_group`, external client). We want a **node-only mode** where:

- **Authoritative mutations** are **signed operations** agreed by peers.
- **Replication** uses **gossip** on the **group topic** (same channel already used for `GroupMutationNotification`).
- **Local state** remains compatible with existing **storage keys** and **admin / meroctl** flows where possible.

**Non-goals (Phase 0):** Delete blockchain integration from the tree (that is a **later phase** — see §11); change context **application** `StateDelta` encryption; implement full invitation commit/reveal off-chain.

---

## 2. Governance modes

| Mode | Behavior |
|------|----------|
| **`external`** (default today) | Group state **canonical source** = chain queries / sync; gossip notifications remain **hints** for refresh. |
| **`local`** | Group state **canonical source** = **ordered application** of **verified signed ops** received from gossip and/or applied locally; **no** chain required for group policy. |

**Configuration (names TBD in implementation):** e.g. `group.governance = "external" | "local"` in node config, set at init or via explicit migration.

**Compatibility:** Existing deployments keep **`external`** until operators opt in.

---

## 3. Wire format: `SignedGroupOp`

**Versioning:** First byte `SCHEMA_VERSION = 1` for forward compatibility.

**Payload (conceptual):**

```text
SignedGroupOp {
  version: u8,                    // 1
  group_id: [u8; 32],
  parent_op_hash: Option<[u8; 32]>, // causal head this op extends (optional v1)
  signer: PublicKey,
  nonce: u64,                   // per (group_id, signer) monotonic counter
  op: GroupOp,                  // borsh enum; see §4
  signature: [u8; 64],          // ed25519 over canonical domain-separated bytes
}
```

**Signing domain:** `b"calimero.group.v1"` concatenated with a borsh encoding of the **signable struct** (version, group_id, parent, signer, nonce, op) — exact layout to be defined in code with a test vector.

**Transport:** Serialized `SignedGroupOp` published to **gossip** on topic **`group/<hex(group_id)>`** (same string scheme as `NodeClient::subscribe_group` / `broadcast_group_mutation`).

**BroadcastMessage:** New variant (e.g. `SignedGroupOpV1`) in `calimero-node-primitives` **or** extend existing group notification path **without** breaking existing `GroupMutationNotification` decode (prefer **new enum variant** for clarity).

---

## 4. `GroupOp` enum (v1 minimal subset)

Start with a **minimal** set; expand as needed.

| Variant | Purpose |
|---------|---------|
| `Noop` | Reserved for tests / padding. |
| `MemberAdded { member_pk, role }` | Add member (exact semantics match existing group roles). |
| `MemberRemoved { member_pk }` | Remove member (cascade rules TBD vs match chain). |

Further variants (later phases): capabilities, visibility, context registration, invites, upgrade policy, etc.

**Rule:** Every variant must be **deterministically** borsh-encoded for signing and hashing.

---

## 5. Ordering, forks, and replay

**Replay protection:** Reject if `nonce` is not **strictly greater** than last stored nonce for `(group_id, signer)` when that signer is still authorized to submit ops (or use **epoch** bump on role change — see below).

**Forks:** If two valid ops share the same parent but differ (concurrent admins), **v1** resolution:

- **Primary rule:** **Higher `nonce` wins** only for the **same** signer; concurrent **different** signers require an **admin epoch** (monotonic `u64` in group meta, only increasable by quorum or single admin key — **TBD**: for MVP, **single admin chain** via `parent_op_hash` + **reject** second head until merge).

**MVP simplification (explicit):** For the first implementation milestone, support **one admin signer** per group OR **strict linear chain** via `parent_op_hash` so the DAG of ops is a **list** (no concurrent heads). Document **multi-admin fork** as a **follow-up**.

**Idempotency:** Store **applied op hash** set (or rolling window) to avoid double-apply of the same gossip message.

---

## 6. Privacy and threat model

**Gossip payload:** **Plaintext** signed bytes on the **group topic** in v1.

- **Anyone who can subscribe** to `group/<hex>` can **read** ops (topic name is not secret if `group_id` is known).
- **Transport encryption** (Noise) does **not** hide content from other mesh peers on the topic.

**Secrets** (invitation tokens, PII): **must not** appear in plaintext gossip in v1; deliver **out-of-band** or add **E2E-encrypted** fields in a later phase.

**Integrity:** Relies on **ed25519** signatures and **nonce** checks, not on topic secrecy.

---

## 7. Relationship to existing `GroupMutationNotification`

Today, `broadcast_group_mutation` sends **hints** (`GroupMutationKind`) after local changes. Under **`local`** governance:

- **Preferred:** **Single path** — local mutations produce **`SignedGroupOp`**, **apply** locally, **publish** to gossip; **optional** lightweight notification can be deprecated or derived.
- **Avoid:** Applying the same logical change twice (hint + full op).

---

## 8. Materialization

After a valid `SignedGroupOp` is applied, update **`group_store`** (and related keys) so **`sync_group`**-equivalent **read** paths see consistent state **without** chain.

**Chain parity:** Map each `GroupOp` variant to the same **invariants** as the contract where feasible; document gaps.

---

## 9. Success criteria for Phase 0 (this document)

- [x] Governance modes documented (`external` vs `local`).
- [x] Signed payload format and topic naming fixed.
- [x] Replay and fork rules for MVP explicitly stated (linear chain / single admin).
- [x] Privacy limitations of gossip documented.
- [x] Relationship to existing notifications documented.
- [x] Phased plan to **remove blockchain integration from the node** documented (§11).

---

## 10. Next steps (Phase 1+)

1. ~~Implement **types + signing/verification** in crates (context primitives / crypto).~~ **Done:** `calimero_context_primitives::local_governance` (`GroupOp`, `SignedGroupOp`, `signable_bytes`, `op_content_hash`, tests).
2. Add **network** variant and **publish/subscribe** handling for `SignedGroupOp`.
3. **Apply** to `group_store` behind **`local`** mode flag.
4. Tests: two nodes, one group, convergent state.
5. Track **§11** for **full removal** of chain code once **`local`** paths cover all required behavior.

---

## 11. Removing blockchain code from the node (full removal)

This is a **separate track** from implementing **`local`** governance: first **parity** (groups + any remaining context-config flows that still hit NEAR), then **delete** dead paths and **trim** dependencies. Do **not** remove chain code until **`local`** is validated for the product surfaces you support.

### 11.1 Target state

- **No** NEAR JSON-RPC / relayer calls from **`merod`** for normal operation in the supported deployment profile.
- **No** mandatory **`[signer]` / `near` / `contract_id`** blocks in config for that profile.
- **Optional:** A **`minimal`** or **`no-chain`** Cargo feature set that builds **without** `near-*` crates where feasible (see `calimero-context-config` features today).

### 11.2 What counts as “blockchain code” in `core` (checklist)

Audit and remove or gate:

| Area | Examples |
|------|----------|
| **Context config client** | `calimero-context-config` **NEAR** transport, relayer transport, `Client::from_config` wiring in `crates/context/config`. |
| **Init / CLI** | `merod init` NEAR defaults, relayer URL, contract id prompts (`crates/merod/src/cli/init.rs`). |
| **Context manager** | Handlers that require `external_config.params["near"]` for group/context flows (`sync_group`, `join_group_context`, invitations, upgrades touching chain). |
| **Relayer** | `mero-relayer` usage assumptions; constants defaulting to NEAR RPC (`crates/relayer`). |
| **Dependencies** | `near-jsonrpc-*`, `near-primitives`, etc., on code paths only used for chain — drop after refactors. |

**Note:** **Application** WASM state and **context** `StateDelta` are **not** “blockchain”; keep them. This removal is about **L1/L2 config contracts** and **group/context membership** backed by chain.

### 11.3 Phased removal strategy

| Phase | Goal |
|-------|------|
| **R1 — Gate** | All group (and agreed context-config) flows work under **`group.governance = local`** without calling external clients. **`external`** still compiles and works for legacy. |
| **R2 — Default** | New installs / docs default to **no chain** for supported SKU; **`external`** opt-in. |
| **R3 — Delete** | Remove **`external`** code paths, NEAR/relayer modules, and unused deps; shrink **`Cargo.toml`** / features; update CI matrices. |
| **R4 — Verify** | `cargo tree` / `cargo check` with **`--no-default-features`** or **`minimal`** feature; integration tests with **no** RPC endpoints. |

### 11.4 Preconditions before R3 (delete)

- [ ] **Feature parity** checklist signed off (groups, invites, upgrades, visibility — whatever you ship) on **`local`** only.
- [ ] **Migration guide** for deployments that still use NEAR (export state, re-bootstrap, or stay on old release).
- [ ] **Downstream** repos (`contracts`, infra, SDKs) updated or explicitly decoupled.

### 11.5 Success criteria for “blockchain removed”

- [ ] No `near` / `relayer` **required** in default `config.toml` for the no-chain profile.
- [ ] CI passes with **chain stubs** disabled or features off.
- [ ] Docs state **single** governance story for that profile (signed gossip ops + local store).

---

## References

- Node client group topic: `crates/node/primitives/src/client.rs` (`subscribe_group`, `broadcast_group_mutation`).
- Group storage: `crates/context/src/group_store.rs`, `crates/store/src/key/group.rs`.
