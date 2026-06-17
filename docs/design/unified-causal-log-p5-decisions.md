# Unified Causal Log — Phase 5/6 open decisions (§9)

Status: **DRAFT for ratification.** The Phase-5 migration (route storage +
governance through one `ScopeState::apply`, wire one `authorize`, delete
`state_hash`) cannot be written correctly until these are settled — each shapes
`OpPayload`, `authorize`, `ScopeState`, the key/scope tree, or the cutover.

For each: the question, the options, and a **recommended default** grounded in
how the system behaves *today* and in what the P5 scaffold (`crates/op`,
`crates/authz`, `crates/projection`) already assumes. Ratify or override.

The scaffold + the convergence/scope-isolation harness already exist
(`feat/unified-op-p5`); these decisions unblock turning them into the live
migration.

---

## §9.1 Concurrent-revoke semantics — ✅ DECIDED: **causal-honor**

When a member/writer is revoked *concurrently* with an op they authored, does
that op survive? **Causal-honor** (chosen 2026-06-12): an op authored before
the revocation *in causal order* stays valid, regardless of the order a
receiver observes the revocation.

- Matches today's `writers_at(parents)` / forward-only `acl_view_at` walk —
  lowest behavioral risk, no convergence-semantics change.
- Already implemented in the scaffold: `ScopeState::acl_view_at(log, parents)`
  folds only the ancestry of an op's parents.
- Rejected: *revoke-wins* (retroactive invalidation — harder to make
  deterministic across peers) and *quarantine* (new held state + resolution
  policy — most work).

---

## §9.2 Who may rotate an object's writers? Is "owner" distinct from "writer"?

**Recommendation: owner = the `OpMask::ADMIN`-capability holder on the object —
NOT a separate owner identity.** A `SetWriters` op is authorized iff its author
holds `ADMIN` on that object in the ACL view at its causal cut.

- Matches today's rule: a writer-set rotation is accepted only when its signer
  held `ADMIN` in the prior set (`writers_at_authenticated`'s ADMIN-chain).
- Already in the scaffold: `AclView::is_owner(author, object) =
  may(author, object, ADMIN)`; `authorize(SetWriters) → is_owner`.
- Bootstrap: the object's creator seeds the initial writer set with `ADMIN`
  (the genesis `SetWriters`, self-authorizing — see §9.3 note).
- Alternatives rejected: *any current writer may rotate* (too permissive — any
  WRITE holder could lock others out); *a separate owner identity distinct from
  the capability set* (extra state, no current need).

**Open sub-point to confirm:** may a non-writer **group admin** rotate an
object's writers (admin override), or only an object `ADMIN` holder? Recommend
**object-`ADMIN` only** for data objects (least authority); group/root admins
act through the membership/admin planes, not by reaching into object ACLs.

---

## §9.3 OpMask lattice — required-bit map, grant/revoke authority, monotonicity

**Recommendation: keep today's 3-bit lattice and the scaffold's mapping.**

- Bits: `WRITE` (0b001), `DELETE` (0b010), `ADMIN` (0b100); `FULL` = all;
  `NONE` = 0. `contains` = superset test. (Unchanged from
  `calimero_storage::entities::OpMask`.)
- Op → required capability (already in `authz::required_mask_for` + `authorize`):
  `Put → WRITE`, `Delete → DELETE`, `SetWriters → ADMIN` (owner),
  `Member* → group-admin`, `Admin*/Policy*/SubgroupCreated → root-admin`.
- **Grant/revoke:** an `ADMIN` holder sets the entire writer/cap map for an
  object via `SetWriters` (grant = add an entry, revoke = drop/lower it).
- **Monotone?** **No.** `SetWriters` replaces the set, so capabilities can be
  revoked; safety comes from causal-honor (§9.1) + per-entry authentication,
  not from monotonicity. (A monotone-only lattice would forbid revocation,
  which the product needs.)
- Future-compat: bits are a `u8`; new capabilities (e.g. `GRANT` distinct from
  `ADMIN`) are an additive bit, not a wire break.

---

## §9.4 Scope tree & key boundaries — which levels are separate key domains?

**Recommendation: a scope is a key domain iff it *restricts* membership.**

- **Root governance scope (`gov-N-root`)** — one per namespace, members = all
  namespace participants. Always shared between any two members; carries the
  owner/admin + root policy + the *non-restricted* group structure. Its key is
  the namespace-wide key.
- **Restricted subgroup → its own scope + its own key.** Matches today's
  per-group `GroupKeyring`: a restricted subgroup's data ops *and* the
  governance ops about it live only in its member-only scope.
- **Open subgroup (inherited membership via `CAN_JOIN_OPEN_SUBGROUPS`) →
  inherits the parent scope/key** (no separate restriction, so no separate key
  domain). Membership is derived by the ancestor walk, not a separate key.
- **A context** is a scope under its owning group, keyed by that group's key.

Net: separate key domain ⇔ restricted membership. This preserves the existing
key model and Invariant 0 (a non-member of a restricted scope never holds its
key, never receives its ops, never computes its root).

---

## §9.5 Existence- vs content-hiding for restricted subgroups (drives §3.4)

**Recommendation: existence-hiding.** A restricted subgroup's existence,
membership, and root must not be derivable by a non-member.

- A restricted child scope's `scope_root` **never** appears inside a visible
  parent's root (no upward leak, §3.4) — a non-member can't even tell the
  subgroup exists.
- Matches today's posture (restricted-subgroup governance lives in the
  member-only scope, not the visible namespace root).
- Rejected: *content-only hiding* (existence visible, payloads encrypted) —
  leaks the subgroup's existence + membership-set size, a weaker privacy
  guarantee than the product currently offers.
- Consequence for the migration: `gov-N-root` reflects only owner/root-policy +
  non-restricted structure; the sync `shared` set is computed from
  *stream-authenticated* membership so a restricted scope is never offered to a
  non-member (it's never named on the wire).

---

## §9.6 Encryption granularity

**Recommendation: confirm — `Op.payload` is ciphertext at rest under the
scope's symmetric key** for the data and membership/admin arms; op metadata
(id, scope, parents, author, hlc, signature, expected_scope_root) stays
cleartext so non-content sync/causality works.

- Matches today's group-key encryption of state deltas + governance ops.
- The scaffold currently stores cleartext payloads (noted in `crates/op`); the
  migration wraps payloads with the scope key at the boundary, leaving the op
  envelope + projection logic unchanged.

---

## §9.7 Ambition — ✅ DECIDED: **full P5 + P6**

Proceed through the merge-core (P5) and sync-core (P6) unification — not
P2-only or through-P4-only. (Chosen 2026-06-12.)

---

## §9.8 Upgrade / cutover — RESOLVED main, **one sub-question open**

**Resolved (design §0.1):** flag-day, no versioned coexistence — a coordinated
redeploy, no mixed-version cluster. (The P2–P4 wire breaks already landed on
this basis in #2745.)

**✅ RESOLVED 2026-06-12: wipe + re-sync — NO re-projection, no backwards
compatibility.** ("I don't care about backwards compatibility.") The P5
migration ships as a clean flag-day: nodes re-bootstrap on the new engine; no
data-preservation/re-projection step is built. This matches the norm already
in effect — #2745 itself changed the on-disk format (rotation-log side-store →
`UnorderedMap` children; `GovernancePosition` borsh layout; Shared-anchor
hashing), so a re-bootstrap across this line of work is already required.

---

## What unblocks once ratified

§9.2/§9.3 → finalize `authorize` + the `OpMask` mapping (mostly confirming the
scaffold). §9.4/§9.5 → define `ScopeId` derivation + the key-domain boundary +
the visibility rule for cross-scope parent edges. §9.6 → the encryption wrap
boundary. §9.8-sub → whether the migration ships a re-projection step or a
wipe. With these settled, the P5 migration (task #16) can begin on a fresh PR
off master, validated against the scope-isolation harness + e2e at each step.

---

# Slice 2 design — wiring `authorize` into the live path

Wiring the one `authorize` in surfaces three semantic questions the per-plane
folds never had to answer together. Recommended resolutions below; ratify, then
the implementation is a **shadow → cutover** (run new alongside old, assert
agreement on live traffic, then flip — never a blind swap of the security
boundary).

## S2.1 — granularity: per-delta membership gate vs per-op capability

Today two *different* checks run: a **coarse** per-delta gate ("is the author a
member of the context's group at the cut?", `authorize_delta_at_edge`) **and** a
per-action signature/writer check (`writers_at_authenticated`). The unified
`authorize` asks one **finer** question per op: `Put → may(author, entity,
WRITE)`. These don't trivially match — a member who isn't an entity writer would
pass the coarse gate but fail `may`.

- **Recommendation: default-write = membership.** A non-restricted context
  grants every member `WRITE` on its entities; the ACL is *seeded from the
  member set*. A restricted object carries an explicit `SetWriters`. Then
  `may(author, entity, WRITE)` reproduces today's "members can write" for
  default contexts and is strictly finer for restricted ones. The migration
  seeds each context's ACL from its membership at re-projection (or, under the
  §9.8 wipe, from genesis membership as contexts re-bootstrap).

## S2.2 — where the `AclView` comes from during shadow

`authorize(op, acl_view_at(parents))` needs an `AclView` at the cut, which
ultimately comes from a live `ScopeState` (slice 3). But slice 2 can run
*before* `ScopeState` is the live projection.

- **Recommendation: shadow off the current resolvers.** In slice 2 build the
  `AclView` from the *existing* resolution (`writers_at_authenticated` → `acl`,
  `acl_view_at`/membership → `groups`, governance meta → `root_admin`), run
  `authorize` against it, and **compare** to the current decision behind a
  divergence metric — acting on the current decision. This validates
  `authorize ≡ the old folds` on live traffic with zero behavioral risk, and
  decouples slice 2 from slice 3. Slice 3 then swaps the `AclView` source to
  `ScopeState::acl_view_at` (same interface).

## S2.3 — OpPayload coverage for the live op stream  *(COMPLETE)*

Coverage is now closed in `calimero-op-adapter`. Every op the live stream can
carry either maps to an `OpPayload` arm or is **out-of-model by design** with a
documented rationale. The dividing line is the unified `authorize` decision,
which is **role**-based (`is_group_admin` ⇔ `role == Admin`, `is_owner` ⇔ root
admin) — exactly like today's cross-DAG `membership_status_at` check. An op is
in-model iff it moves that decision (or the data/ACL/scope-tree state under it).

**In-model — mapped:**
- data: `Action::Add/Update` → `Put`, `DeleteRef` → `Delete` (`Compare` is a
  sync hint, not a state change → `None`).
- ACL: `RotationLogEntry` → `SetWriters`.
- membership: `GroupOp::MemberAdded/MemberRoleSet` → `MemberAdded`;
  `MemberRemoved/MemberLeft` → `MemberRemoved`;
  `MemberJoinedViaTeeAttestation` → `MemberAdded` (attestation evidence is the
  admission gate's, not the projection's); `RootOp::MemberJoined` → `MemberAdded`
  (group_id + role decoded off the admin-signed invitation, no escalation);
  `RootOp::MemberJoinedOpen` → `MemberAdded` (Member).
- ownership: `GroupOp::TransferOwnership` → `AdminChanged` (owner ⇔ ADMIN, §9.2;
  authored in the group's scope, so it sets that scope's root admin).
- admin/policy: `RootOp::AdminChanged` → `AdminChanged`, `PolicyUpdated` →
  `PolicyUpdated`.
- scope tree: `RootOp::GroupCreated` → `SubgroupCreated`, `GroupReparented` →
  `SubgroupReparented`, `GroupDeleted` → `SubgroupDeleted` (one per cascaded
  scope; `restricted` resolved from policy at the live call site).

**Out-of-model by design (`None`) — never enters the auth decision:**
- key transport: `KeyDelivery` (§9.6 — payloads are scope-key-encrypted; the
  key delivery rides its own channel), `Noop` (padding).
- capability refinement: `MemberCapabilitySet`, `DefaultCapabilitiesSet`,
  `ContextCapabilityGranted/Revoked` — a separate, deferred permission layer
  (§9.3 keeps the simple lattice). Capabilities never gate `authorize`.
- app/upgrade/migration config: `UpgradePolicySet`, `TargetApplicationSet`,
  `GroupMigrationSet`, the `Cascade*` ops — owned by app-version / migrations-v2.
- metadata + policy knobs: `GroupMetadataSet`, `MemberMetadataSet`,
  `ContextMetadataSet`, `SubgroupVisibilitySet`, `TeeAdmissionPolicySet`,
  `MemberSetAutoFollow`.
- context↔group binding: `ContextRegistered/ContextDetached`, `GroupDelete` —
  `authorize` derives a context's group from that binding at auth time (the P4
  context→group lookup), so it lives in that index, not a scope's `ScopeState`.

The adapter uses `_ => None` catch-alls (not exhaustive matches) so it keeps
compiling as master grows the governance enums; any genuinely auth-relevant new
variant must be armed explicitly, and slice-3's coverage/divergence tests would
surface one that slipped through.

## Slice 2 implementation order (once ratified)
1. `AclView`-from-current-resolvers builder + `authorize` shadow at the
   gossip-receive site, divergence metric, act-on-current. (Additive-ish.)
2. Extend shadow to the other auth sites (snapshot-replay, drain, DAG-catchup,
   manager, writer-plane resolve). e2e: divergence metric stays zero.
3. Cut over: `authorize` becomes the decision; old folds deleted in slice 5.
