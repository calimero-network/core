# Local group governance (Proposal 1) — Phase 0 design

This document fixes **decisions and scope** before implementation. It describes **signed group operations** replicated over the existing **per-group gossip topic** (`group/<group_id_hex>`), with **local materialization** into node storage (`group_store`), as an alternative to **NEAR**-backed group state.

**Branch:** `feat/local-group-governance-ops`  
**Related:** [GROUP-FEATURE-OVERVIEW.md](./GROUP-FEATURE-OVERVIEW.md) (product behavior; chain-oriented sections will gain a “local governance” counterpart). **Migration:** [LOCAL-GROUP-GOVERNANCE-MIGRATION.md](./LOCAL-GROUP-GOVERNANCE-MIGRATION.md).

---

## 1. Problem statement

Today, group metadata and permissions are **synchronized from on-chain** contracts (`sync_group`, external client). We want a **node-only mode** where:

- **Authoritative mutations** are **signed operations** agreed by peers.
- **Replication** uses **gossip** on the **group topic** (same channel already used for `GroupMutationNotification`).
- **Local state** remains compatible with existing **storage keys** and **admin / meroctl** flows where possible.

**Non-goals (Phase 0):** Delete blockchain integration from the tree (that is a **later phase** — see §11); change context **application** `StateDelta` encryption.

**Update:** Off-chain **join with invitation** is implemented for `local` governance via `JoinWithInvitationClaim` (admin-signed `GroupInvitationFromAdmin` + joiner-signed `GroupRevealPayloadData`), without NEAR commit/reveal transactions.

---

## 2. Governance modes

| Mode | Behavior |
|------|----------|
| **`external`** (default today) | Group state **canonical source** = chain queries / sync; gossip notifications remain **hints** for refresh. |
| **`local`** | Group state **canonical source** = **ordered application** of **verified signed ops** received from gossip and/or applied locally; **no** chain required for group policy. |

**Configuration:** In **`config.toml`**, under **`[context]`**, set **`group_governance = "external"`** (default) or **`"local"`**. New nodes: **`merod init`** defaults to **`external`**; use **`merod init --group-governance local`** for a **no NEAR protocol block** in **`[context.config]`** (no `network` / `contract_id` under flattened protocol params) and **no relayer signer** in the context client config. Local-only group flows do not use chain RPC. Add **`[protocols.near]`** later if you need chain-backed contexts or **`join_group_context`** bootstrap that reads NEAR params.

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

## 4. `GroupOp` enum (v1 — contract-aligned surface)

Implemented in `calimero_context_primitives::local_governance::GroupOp` and applied in
`group_store::apply_local_signed_group_op` when `group_governance = local`.

| Variant | Purpose |
|---------|---------|
| `Noop` | Reserved for tests / padding. |
| `MemberAdded { member, role }` | Add member (roles match `GroupMemberRole`). **Signer:** admin. |
| `MemberRemoved { member }` | Remove member; rejects removing the last admin. **Signer:** admin. |
| `MemberRoleSet { member, role }` | Change role (cannot demote the last admin). **Signer:** admin. |
| `MemberCapabilitySet { member, capabilities }` | Per-member capability bitmask. **Signer:** admin. |
| `DefaultCapabilitiesSet { capabilities }` | Default capabilities for new members. **Signer:** admin. |
| `UpgradePolicySet { policy }` | Updates `GroupMetaValue.upgrade_policy`. **Signer:** admin. |
| `TargetApplicationSet { app_key, target_application_id }` | Updates `GroupMetaValue` target app / app key. **Signer:** admin. |
| `GroupMigrationSet { migration }` | Updates `GroupMetaValue.migration` (`Option<Vec<u8>>`). **Signer:** admin. Used with `TargetApplicationSet` for local upgrades. |
| `ContextRegistered { context_id }` | `register_context_in_group` index. **Signer:** group admin **or** member with `CAN_CREATE_CONTEXT`. |
| `ContextDetached { context_id }` | `unregister_context_from_group` (must match this group). **Signer:** admin. |
| `DefaultVisibilitySet { mode }` | `0` = Open, `1` = Restricted. **Signer:** admin. |
| `ContextVisibilitySet { context_id, mode, creator }` | Per-context visibility + creator pubkey; if Restricted, creator is added to allowlist when missing. **Signer:** admin **or** `creator` (context creator). |
| `ContextAllowlistReplaced { context_id, members }` | Full allowlist replace. **Signer:** admin **or** context creator (matches HTTP handler). |
| `ContextAliasSet { context_id, alias }` | Context alias within the group. **Signer:** admin **or** context creator (matches stored `GroupContextVisibility.creator`). |
| `MemberAliasSet { member, alias }` | Member alias. **Signer:** admin **or** `member` (self). |
| `GroupAliasSet { alias }` | Group alias. **Signer:** admin. |
| `GroupDelete` | Local-only group teardown (no registered contexts); mirrors CLI delete cleanup. **Signer:** admin. |
| `JoinWithInvitationClaim { signed_invitation, invitee_signature_hex }` | Adds joiner as `Member` after verifying admin signature on `GroupInvitationFromAdmin` and joiner signature on `GroupRevealPayloadData` (same crypto as NEAR reveal). **Outer `SignedGroupOp.signer`:** joiner. |

**Rule:** Every variant must be **deterministically** borsh-encoded for signing and hashing. `GroupOp` / `SignedGroupOp` do **not** derive `PartialEq` (nested `SignedGroupOpenInvitation` is not comparable).

**Upgrade / migration:** The on-chain **`GroupUpgradeValue`** progress state machine remains **local store** only (not a `GroupOp`). Under `local`, upgrades use **`TargetApplicationSet`** + optional **`GroupMigrationSet`**, then existing propagator / lazy logic; `GroupMutationKind::Upgraded` may still be broadcast on **`external`** paths.

---

### 4.1 Context manager: `local` vs `external`

When **`group_governance = local`**, handlers that would call the NEAR group client instead **`sign_apply_local_group_op_borsh`** (or the join path above) and **`publish_signed_group_op`**, and skip chain calls. When **`external`**, behavior matches the legacy chain + local store / broadcast paths.

| Area | `local` | `external` |
|------|---------|------------|
| Create / delete group, members, roles, caps, defaults, visibility, aliases, detach, allowlist | Signed `GroupOp` + publish | Chain (where applicable) + store + `broadcast_group_mutation` hints |
| Join group (`join_group`) | `JoinWithInvitationClaim` if metadata exists locally; no chain bootstrap | Commit/reveal on chain + `add_group_member` + sync |
| Join group context (`join_group_context`) | Skip `join_context_via_group`; local `register_context_in_group` + sync | Chain + local register |
| Create context in group | `ContextRegistered` + publish; signed `ContextVisibilitySet` + publish (group default visibility, or **Open** if unset); then optional `ContextAliasSet`; peers converge on visibility + alias; no `ContextAttached` broadcast | Chain register + store + broadcasts |
| Delete group context | `ContextDetached` + publish (detach applied in `apply_local`) | Chain unregister + local unregister + broadcast |
| Group upgrade | `TargetApplicationSet` (+ `GroupMigrationSet` when needed) + publish; no `set_group_target` | `set_group_target` + existing upgrade flow |

**Implementation note:** `join_group_context` still loads **`near`** protocol params from node config (for proxy contract lookup when the context is unknown) even when `group_governance = local`; it skips **`join_context_via_group`** in that mode.

**Gossip ingestion:** `ApplySignedGroupOp` applies only when mode is `local` (see `apply_signed_group_op.rs`).

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

Today, `broadcast_group_mutation` sends **hints** (`GroupMutationKind`) after some changes. Under **`external`**, those hints often line up with chain-driven refreshes. Under **`local`**:

| Source of truth | **`SignedGroupOp`** on the group gossip topic (`publish_signed_group_op`). Peers apply via `ApplySignedGroupOp` / `apply_local_signed_group_op`. |
|-----------------|--------------------------------------------------------------------------------------------------------------------------------------------------------|
| **`GroupMutationKind` hints** | **Not** relied on for policy under `local`. Most handlers skip `broadcast_group_mutation` when `group_governance = local` (see `create_context`, member ops, etc.). |
| **Upgrade (`Upgraded`)** | **`external` only:** after a successful upgrade path, the node may still broadcast `GroupMutationKind::Upgraded` as a refresh hint. Under **`local`**, upgrade target/migration are already **`TargetApplicationSet` / `GroupMigrationSet`** gossip; the **`Upgraded`** hint is **not** broadcast (DHT blob announce still runs). |
| **Clients / UIs** | Should treat **`group_store`** (and inbound signed ops) as authoritative under `local`; use mutation notifications only where you know the deployment uses **`external`**, or as a best-effort refresh signal where still emitted. |

**Invitations (`create_group_invitation`):** Under **`local`**, invitation payloads use synthetic **`protocol` / `network` / `contract_id`** (`"local"` each) so admins do **not** need `[protocols.near]` in config. **`JoinWithInvitationClaim`** verification is unchanged (hash over the full invitation). Under **`external`**, behavior is unchanged: coordinates come from **`[protocols.near]`**.

**Create context without group default visibility:** If **`GroupDefaultVis`** is unset, **`create_context`** still publishes **`ContextVisibilitySet`** with mode **`0` (Open)** so peers do not diverge on visibility.

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
2. ~~Add **network** variant and **publish/subscribe** handling for `SignedGroupOp`.~~ **Done:** `BroadcastMessage::SignedGroupOpV1 { payload }` in `calimero-node-primitives` (opaque `borsh(SignedGroupOp)` bytes), `MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES`, `NodeClient::publish_signed_group_op`, and inbound handling in `node/src/handlers/network_event.rs` (decode, topic vs `group_id`, `verify_signature`; apply to `group_store` deferred to Phase 3).
3. ~~**Apply** to `group_store` behind **`group_governance = local`**.~~ **Done:** `GroupGovernanceMode` in `[context]` (`external` \| `local`, default `external`), `ContextManager` + `NodeState` wiring, store key `GroupLocalGovNonce`, `group_store::apply_local_signed_group_op`, `ContextClient::apply_signed_group_op` / `ApplySignedGroupOp`, inbound apply in `network_event.rs` when mode is `local`.
4. ~~Tests: two nodes, one group, convergent state.~~ **Done:** `crates/context/tests/local_group_governance_convergence.rs` (plus target/migration and join-invitation sequences); `crates/network/tests/gossipsub_group_topic.rs` for libp2p mesh delivery.
5. ~~**Context manager handlers** under `local` (skip NEAR, signed ops).~~ **Done:** create/delete group, members, settings, capabilities, visibility, aliases, detach, allowlist, join (invitation claim), join context, create/delete context in group, upgrades (`TargetApplicationSet` / `GroupMigrationSet`).
6. **Follow-ups:** ~~Signed gossip for **context alias** / **visibility** on create~~ **Done**. ~~**`create_group_invitation`** without NEAR config under `local`~~ **Done** (synthetic `local`/`local`/`local` coordinates). ~~**`GroupMutationKind` vs `local`**~~ **Documented in §7**; **`Upgraded`** hint skipped under `local` where redundant with signed upgrade ops.
7. ~~Track **§11** preconditions~~ **Ongoing:** migration guide shipped; feature parity / downstream / R3 delete remain product-driven.

---

## 11. Removing blockchain code from the node (full removal)

This is a **separate track** from implementing **`local`** governance: first **parity** (groups + any remaining context-config flows that still hit NEAR), then **delete** dead paths and **trim** dependencies. Do **not** remove chain code until **`local`** is validated for the product surfaces you support.

**§11 quick links:** [11.4 Preconditions](#114-preconditions-before-r3-delete) · [11.7 Staging parity](#117-staging-parity-pass-product) · [11.8 Downstream](#118-downstream-inventory-r3) · [11.9 Minimal build](#119-minimal-build-sketch-r3) · [11.10 CI guardrail](#1110-ci-guardrail-placeholder)

### 11.1 Target state

- **No** NEAR JSON-RPC / relayer calls from **`merod`** for normal operation in the supported deployment profile.
- **No** mandatory **`[signer]` / `near` / `contract_id`** blocks in config for that profile.
- **Optional:** A **`minimal`** or **`no-chain`** Cargo feature set that builds **without** `near-*` crates where feasible (see `calimero-context-config` features today).

### 11.2 What counts as “blockchain code” in `core` (checklist)

Audit and remove or gate:

| Area | Examples |
|------|----------|
| **Context config client** | `calimero-context-config` **NEAR** transport, relayer transport, `Client::from_config` wiring in `crates/context/config`. |
| **Init / CLI** | `merod init` NEAR defaults by default; **`merod init --group-governance local`** omits NEAR protocol blocks in context client config (`crates/merod/src/cli/init.rs`) and the **relayer** signer entry. |
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

- [ ] **Feature parity** checklist signed off (groups, invites, upgrades, visibility — whatever you ship) on **`local`** only — use **[§11.7](#117-staging-parity-pass-product)**.
- [x] **Migration guide:** [LOCAL-GROUP-GOVERNANCE-MIGRATION.md](./LOCAL-GROUP-GOVERNANCE-MIGRATION.md) (new installs, `external` → `local`, rollback, automation pointers).
- [ ] **Downstream** repos (`contracts`, infra, SDKs) updated or explicitly decoupled — track in **[§11.8](#118-downstream-inventory-r3)**.

**Scope:** Optional **`relayer`** in the context client signer and omitting it for **`merod init --group-governance local`** is an **R1** configuration item (see §11.5). It does **not** check off R3: parity sign-off, downstream decoupling, and **CI / builds without NEAR crates on the link line** remain separate workstreams.

### 11.5 Success criteria for “blockchain removed”

- [x] **No NEAR protocol params** in generated **`[context.config]`** for **`merod init --group-governance local`** (R1 gate). **`relayer`** is optional in the context client signer config and omitted for **`local`** init.
- [ ] CI passes with **chain stubs** disabled or features off *(see **[§11.10](#1110-ci-guardrail-placeholder)**; workspace **`cargo test`** already includes **`calimero-context`**; a build **without** NEAR crates on the link line is **R3+**)*.
- [x] Docs state **single** governance story for **`local`** (this file + §2 / §7).

### 11.6 R3+ kickoff (next engineering passes)

Use this as a **starting order**; adjust with product priority.

1. **Parity sign-off (§11.4)** — Schedule and run **[§11.7](#117-staging-parity-pass-product)** on staging; file gaps before deleting **`external`** code.
2. **Dependency audit** — Run `scripts/audit-near-deps-for-r3.sh`; read **§11.9** (“Typical edges”) for the current summary. Map each edge to “required for **`external`** only” vs removable.
3. **Feature sketch for `no-chain` / `minimal`** — See **[§11.9](#119-minimal-build-sketch-r3)**; extend `calimero-context-config` (and dependents) with features that stub or omit NEAR transports first; only then drop `near-*` from default `merod` builds.
4. **Downstream inventory** — Fill in **[§11.8](#118-downstream-inventory-r3)** (`contracts`, infra, SDKs, `mero-tee`, automation); decide migrate vs document vs out-of-scope.
5. **CI guardrail** — After (3), follow **[§11.10](#1110-ci-guardrail-placeholder)** (exact `cargo check` flags TBD once features land).

### 11.7 Staging parity pass (product)

**Purpose:** Prove **`group_governance = local`** is acceptable for the surfaces you ship **before** R3 deletes **`external`** paths. This is a **scheduled staging exercise**, not a substitute for automated tests.

#### Schedule (fill in)

| Field | Value |
|-------|--------|
| **Owner** | *(product or engineering DRI)* |
| **Target window** | *(e.g. sprint / date range)* |
| **Build** | *(image tag, `merod` commit, or release candidate)* |
| **Staging environment** | *(cluster name, namespace, or runbook link)* |
| **Participants** | *(QA, SRE, app team)* |

#### Topology (minimum)

- **Two or more** peered nodes with **`[context].group_governance = "local"`** and **no** NEAR protocol block / relayer signer in context client config (e.g. `merod init --group-governance local` per [migration](./LOCAL-GROUP-GOVERNANCE-MIGRATION.md)).
- Connectivity: nodes reach each other on the **group** gossip path (same expectations as production P2P).

#### Checklist — `local` only

Use this as the **§11.4 parity** record; add rows if your SKU exposes more APIs.

**Groups & membership**

- [ ] Create group; second node learns group metadata via gossip (or defined bootstrap path).
- [ ] Add / remove members; both nodes agree on membership.
- [ ] Delete group (if supported for your deployment).

**Invitations & join**

- [ ] Create group invitation (commit / reveal path your product uses).
- [ ] Join group via invitation on a node that was not the creator.
- [ ] Join context within group (`join_group_context` / equivalent) without NEAR bootstrap for **known** group state.

**Contexts**

- [ ] Create context in group; register / unregister as applicable.
- [ ] **Visibility** and **allowlist** changes propagate (restricted / open as you ship).
- [ ] **Aliases** (group / member / context) as applicable.

**Upgrades**

- [ ] **Target application** / **migration** set (signed ops path under `local`); no spurious **`Upgraded`** hint requirement if your doc says it is skipped under `local`.

**Multi-node convergence**

- [ ] Two-node scenario: operations on node A appear in group state on node B within expected time (ordering / nonce expectations per §5–§7).

#### Sign-off

| Role | Name | Date | Notes |
|------|------|------|--------|
| Product | | | |
| Engineering | | | |

When this table is complete, check **§11.4 — Feature parity** and archive a link to the staging ticket or runbook in your tracker.

### 11.8 Downstream inventory (R3)

**Purpose:** Before removing **`external`** code paths, know which **other repos and pipelines** still assume chain-backed group policy or a relayer line in node config. This table is the working record for **§11.4 — Downstream**.

#### How to use

Add one row per surface; set **Decision** when triaged. **N/A** is fine for areas that never touched group governance.

| Area | Repo / path / doc | Assumption today (`external` / relayer / NEAR block) | Decision (migrate · document · N/A) | Owner | Status |
|------|---------------------|------------------------------------------------------|---------------------------------------|-------|--------|
| On-chain contracts | | | | | |
| K8s / Helm / infra templates | | | | | |
| `merod` / node image init (`mero-tee`, MDMA, etc.) | | | | | |
| Client SDKs (JS, Rust, etc.) | | | | | |
| Internal runbooks & dashboards | | | | | |

#### Notes

- **`local`**-only operators may still add **`[protocols.near]`** later for chain-backed apps; inventory should capture **defaults** and **automation** that still inject **`external`** or relayer URLs without an explicit choice.
- When every row has a **Decision** and owners agree, check **§11.4 — Downstream** and link the filled table (or ticket) from your release plan.

### 11.9 Minimal build sketch (R3)

**Purpose:** Track how to reach a **`merod`** binary (or test surface) that does **not** link `near-*` for **`local`**-only SKUs. **`calimero-context-config`** now has a **`client-base`** split (see table); **`merod`** still pulls **`near-crypto`** directly until that is gated separately.

#### Dependency audit (how to refresh)

Run from the workspace root:

```bash
./scripts/audit-near-deps-for-r3.sh
```

**Typical edges into `near-*` from `merod` today** (inverse tree; always re-run the script before acting):

- **`calimero-context-config`**, feature **`client-base`** → HTTP relayer + env helpers **without** `near-*` crates. Feature **`near_client`** (included in **`client`**) adds NEAR JSON-RPC transport and typed local signers.
- **`merod`** depends on **`near-crypto`** directly (init / signer helpers).
- **`mero-auth`** pulls **`near-primitives`** / JSON-RPC for chain-adjacent auth paths.
- Other workspace crates (`calimero-node`, `calimero-server`, …) transitively depend on the same stacks.

#### Proposed layering (working table)

| Crate / area | Today | Direction for `minimal` / `no-chain` |
|--------------|-------|--------------------------------------|
| `calimero-context-config` | **`client-base`** = relayer + `EmptyNearSlot`; **`near_client`** adds JSON-RPC + `Credentials` / `NearTransport`; **`client`** = **`near_client`** (alias) | Next: gate **`merod`**’s direct **`near-crypto`** |
| `merod` | Always **`near-crypto`** | Gate init/signing paths behind a **`merod`** feature or split binary (TBD) |
| `mero-auth` | NEAR types in tree | Feature-gate chain-backed providers vs embedded-only |
| `calimero-node` / `calimero-server` | Transitive | After config + auth, trim imports or use trait objects where feasible |

Update this table as spikes land; link PRs in your tracker.

### 11.10 CI guardrail

**Purpose:** Satisfy §11.5 (“CI passes with chain stubs disabled or features off”) incrementally.

**In CI today (`.github/workflows/ci-checks.yml`):**

```bash
cargo check -p calimero-context-config --features client-base
cargo test -p calimero-context-config --features client
```

**Still TBD:** a **`merod`** (or workspace) `cargo check` that omits **`near-crypto`** on the `merod` crate itself — track in §11.9 **`merod`** row.

---

## References

- Node client group topic: `crates/node/primitives/src/client.rs` (`subscribe_group`, `broadcast_group_mutation`).
- Group storage: `crates/context/src/group_store.rs`, `crates/store/src/key/group.rs`.
