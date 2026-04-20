# ADR 0001 — Auto-follow group membership

- **Status:** proposed
- **Authors:** Claude Opus 4.7 (with Sandi Fatic)
- **Date:** 2026-04-20

## Context

Group membership in Calimero core is explicit at every level: members of a parent group are not automatically members of nested subgroups, and members of a group do not automatically replicate new contexts created within that group. Each must be triggered by an explicit op (`MemberAdded`, `join_context`).

This works for human-driven clients but breaks two concrete use cases:

1. **TEE fleet HA.** A fleet node is admitted to a namespace via `POST /admin-api/tee/fleet-join`. Today `fleet_join.rs:153-202` walks the namespace's direct contexts and joins each. Contexts created after that call, or contexts living in subgroups, are never picked up. The mero-tee sidecar therefore has no way to converge when namespaces evolve.
2. **Regular members observing a growing group.** Same gap — a member who joined when the group had 3 contexts sees nothing when a 4th is added unless the client explicitly calls `join_context`.

The current workaround under consideration is a client-side resync driven by the manager: `enableHaForNamespace` re-issues with the latest group list every time the UI notices a change. This pushes correctness into every client and adds a polling hot-path to the manager.

A DAG-native solution is materially cheaper and benefits all members, not just fleet nodes.

## Decision

### Summary

Add an opt-in, per-member `auto_follow` flag pair (`{ contexts, subgroups }`) to `GroupMember`. A node-side handler subscribes to governance-DAG op-apply events and, for any op affecting a group where self has the relevant flag set, emits the matching join/admission op. All propagation and catch-up run through the existing governance DAG — no reconcile loop, no pubsub-only signaling.

### Decisions

**1. Opt-in granularity — per-member flag in DAG state.**
`GroupMember` gains `auto_follow: AutoFollowFlags { contexts: bool, subgroups: bool }`. Toggled by a new `NamespaceOp::MemberSetAutoFollow { target, flags }`. A member can set their own flags; an admin can set anyone's. Defaults: `false/false` for regular members, `true/true` for `ReadOnlyTee` (set automatically at admission, no user action).

**2. Subgroup role — inherit, or refuse.**
When auto_follow triggers admission into a newly-nested subgroup, the joining role matches the parent role. If the subgroup's admission policy would reject that role, the auto-follow fails for that specific subgroup (logged with `group_id` and `op_id`). No silent clamping, no admin-lift, no bypass.

**3. TEE subgroup re-attestation — re-attest, reuse the quote.**
Q4 of research confirmed `build_report_data(&nonce, pk_hash)` binds the TDX quote to the node's public key hash, not the group_id. One quote therefore covers N subgroups in the same namespace with no per-subgroup TDX regeneration. Every subgroup's `tee_admission_policy` is still re-validated on its own `TeeAttestationAnnounce` — admission is never skipped.

**4. Admission invariant — no bypass, ever.**
Every auto-join traverses the same `admit_tee_node` / `MemberAdded` path as a human-driven admission. The handler is a producer of ops, never an applier of state.

**5. Propagation — governance DAG.**
The handler reacts to DAG op-apply events (not pubsub). Every op it emits is itself a DAG op. Consequence: offline catch-up is just DAG replay on restart; no reconcile loop needed.

**6. Rate limit — per-node 20 `join_context`/min with FIFO overflow queue.**
Bounds amplification when a chatty namespace (many members, rapid context creation) intersects with auto_follow enabled across many peers.

**7. Observability — flags in member-list response + structured log per auto-join.**
Each auto-join emits a log line with initiating op id, target group_id, and resulting context_id or child_group_id. Enough for postmortems; no new metrics backend required.

### Non-goals

- **Group-level policy** ("admin wants everyone in this group to auto-follow"). Per-member consent is simpler and safer. Can be added later as a convenience on top of per-member flags.
- **Auto-leave on parent-leave.** If a member leaves a parent group, we don't retroactively remove them from subgroups they auto-joined. Explicit `cascade_remove_member_from_group_tree` already handles admin-driven removal.
- **Eventual consistency guarantees beyond the DAG.** If the DAG replay finds a member's auto-follow op was applied after a subgroup-nest op, the handler still fires on replay and emits the join op then. This matches the governance model's existing semantics.

## Research findings (Phase 0)

Answers to the six gating questions, with code citations:

**Q1 — Is `nest_group` a DAG op?**
Yes. `nest_group` in `crates/context/src/group_store/namespace.rs:96` mutates state, but only as the side-effect of `NamespaceOp::Root(RootOp::GroupNested { parent_group_id, child_group_id })` applied via `execute_group_nested` (`namespace_governance.rs:532-549`). Handlers emit it through `sign_apply_and_publish_namespace_op`. ✓

**Q2 — Does DAG op-apply emit events?**
**No.** `apply_signed_namespace_op` (`handlers/apply_signed_namespace_op.rs:7-37`, `governance_dag.rs:56-73`) calls `NamespaceGovernanceApplier::apply()` which returns `ApplyNamespaceOpResult { pending_deliveries, key_unwrap_failures }` — diagnostic only, no broadcast. Adding a channel costs ~200-300 lines: `enum OpEvent`, a `tokio::sync::broadcast` in `ContextManager`, emit sites in `namespace_governance.rs`. **→ Phase 1 is required, not conditional.**

**Q3 — `GroupMember` migration story.**
`GroupMemberValue` at `crates/store/src/key/group/mod.rs:1172` is Borsh-serialized. Borsh is strict — missing fields fail deserialization and `#[serde(default)]` is ignored. Migration path: introduce `GroupMemberValueV2` with the new field, add a store-layer read that tries V2 first and falls back to V1-then-upcast, write V2 on every update. Approx 50 lines.

**Q4 — TDX quote scope.**
Pubkey-scoped. `fleet_join.rs:66-68` computes `pk_hash = Sha256(our_public_key)` and `build_report_data(&nonce, Some(&pk_hash))`. `build_report_data` (`tee-attestation/src/generate.rs:187`) packs nonce||pk_hash into the 64-byte report_data; no group_id is mixed in. A quote generated for the namespace covers all subgroups under it with no regeneration cost. ✓

**Q5 — Existing rate-limit utility?**
None. A scan of `network-primitives`, `node`, `server` crates found only discovery-specific throttling (`is_rendezvous_discover_throttled` in `discovery/state.rs`). **→ Build a small local token-bucket for the handler, ~100 lines.**

**Q6 — Does `list_group_contexts` recurse?**
No. `list_group_contexts.rs:26-31` → `aliases.rs:44-57` → `contexts.rs:32-39` → `ContextTreeService::enumerate_contexts` (`context_tree.rs:68-79`) — scans `GroupContextIndex` rows with a direct group_id prefix. Subgroup contexts are not included. **→ Consequence for fleet_join**: its current initial-loop already misses subgroup contexts. Once auto-follow ships, the handler covers this — on self-join, enumerate existing subgroups, emit self-admission ops for each (subject to the rate limiter). The initial-loop in `fleet_join.rs:153-202` can be simplified or removed entirely.

## Implementation plan

Six phases, each shippable independently, each with its own PR.

### Phase 1 — DAG op-apply event channel (required)

New `OpEvent` enum covering the op variants the handler reacts to (`GroupNestedApplied`, `ContextRegisteredApplied`, `MemberJoinedViaTeeAttestationApplied`, `MemberAddedApplied`, `MemberSetAutoFollowApplied`). `tokio::sync::broadcast` channel stored in `ContextManager`. Emit events from the side-effect blocks inside `apply_signed_namespace_op`. No behavior change.

Files touched: `context/src/group_store/namespace_governance.rs`, `context/src/group_store.rs`, new `context/src/events.rs`. ~250 lines.

### Phase 2 — `auto_follow` state + governance op

- `GroupMemberValueV2` at `store/src/key/group/mod.rs` with `auto_follow: AutoFollowFlags`. Store-layer migration reads V1 records and upcasts.
- New `NamespaceOp::MemberSetAutoFollow { target, flags }` with authorization: admin-or-self.
- `meroctl group member set-auto-follow <GROUP_ID> [--contexts] [--subgroups] [--member <pubkey>]`.

Files touched: `store/src/key/group/mod.rs`, `context/primitives/src/local_governance/mod.rs`, `context/src/group_store/namespace_governance.rs`, `meroctl/src/cli/group/`. ~400 lines.

### Phase 3 — the handler

New subscriber on `OpEvent::*` channel in a new module `node/src/handlers/auto_follow.rs`:

- `ContextRegisteredApplied { group, context }` → if self is member of `group` with `auto_follow.contexts=true` → emit `JoinContext { context }`.
- `GroupNestedApplied { parent, child }` → if self is member of `parent` with `auto_follow.subgroups=true`:
  - `ReadOnlyTee`: reuse existing TDX quote, emit `TeeAttestationAnnounce` on `child`.
  - Regular roles: emit self-admission on `child` carrying inherited role. If child policy rejects, log and skip.
- `MemberJoinedViaTeeAttestationApplied { group, member=self }` → enumerate existing subgroups + contexts and emit the corresponding join ops (covers the "joined after state already exists" case without a reconcile loop).

Token bucket: 20 emissions/min per node, overflow to bounded queue drained as budget frees. Dropped emissions only on queue-full (log loudly).

Files touched: new `node/src/handlers/auto_follow.rs`, `node/src/lib.rs` wiring. ~500 lines.

### Phase 4 — TEE default

In `admit_tee_node.rs`, after `MemberJoinedViaTeeAttestation` is published, immediately publish `MemberSetAutoFollow { target: new_member, flags: { contexts: true, subgroups: true } }` on the same member. One additional op per TEE admission.

~30 lines.

### Phase 5 — tests

- **Unit**: `MemberSetAutoFollow` authorization; handler dispatch per op type; token bucket at budget edge.
- **E2E** (extending `e2e-tests/group-nesting`): HA namespace with TEE fleet, nest subgroup after admission, assert TEE admitted to subgroup. Create context after admission, assert auto-join. Node offline during context creation, restart, assert DAG replay catches up.
- **Chaos**: 50 contexts created in 5 s → rate limiter drains without drop.

### Phase 6 — deprecate client workarounds

- `mero-tee/ansible/roles/merotee/templates/fleet-sidecar.sh.j2`: call `meroctl tee fleet-join` once per namespace instead of once per group.
- `tauri-app/.../Namespaces.tsx`: drop the about-to-be-added client-side resync — the manager's `enableHaForNamespace` call becomes truly one-shot.
- Update `mdma/manager/app/routers/cloud.py`: `HaRequest` per-group rows stay (needed for MDMA's billing/quota accounting), but the re-sync trigger path is removed.

## Risks

- **Resource amplification** — mitigated by token bucket; monitor dropped-event logs.
- **Admission policy drift** — mitigated by "inherit-or-fail" rule (decision 2) + "no bypass" invariant (decision 4). Log loudly on inherit-fail so admins can reconcile.
- **Migration regressions** — covered by V1→V2 upcast path + a test that seeds a V1-format record and confirms deserialization succeeds.
- **Governance race** — two admins racing to auto-admit + explicit-admit into a subgroup could produce duplicate `MemberAdded`. Existing governance ordering deduplicates; add an e2e test to confirm.

## Alternatives considered

**Client-side resync.** Every client polls its namespace topology and calls `enable-ha-namespace` on change. Pushes correctness into every client, adds a polling hot-path to the manager, doesn't benefit non-cloud users. Rejected.

**Pubsub-only event channel.** Use namespace-topic messages directly without adding a DAG op-apply channel. Loses offline catch-up (pubsub is best-effort), forces a reconcile loop back into the design. Rejected.

**Per-group policy instead of per-member flag.** Admin sets "everyone in this group auto-follows". Coercive, harder to reason about. Rejected — can be added later as sugar on top of per-member flags if desired.

## Open questions (deferred to implementation)

- Should the `OpEvent` broadcast channel be lossy (`broadcast::channel`) or guaranteed-delivery? Leaning lossy with a replay-from-DAG fallback; the handler is robust to dropped events since DAG is authoritative.
- Should rate-limit parameters be configurable via node config? Default 20/min is a guess based on typical namespaces; real numbers from prod would refine.
- For `meroctl group member set-auto-follow`, should the default target be self when no `--member` is given? Leaning yes — ergonomic for human use.
