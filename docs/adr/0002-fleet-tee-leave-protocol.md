# ADR 0002 — Fleet TEE leave hygiene (post-eviction cleanup)

| | |
|---|---|
| **Status** | Proposed |
| **Date** | 2026-06-02 |
| **Deciders** | Calimero core team, mero-tee KMS team |
| **Context** | [mdma#155](https://github.com/calimero-network/mdma/issues/155) — HA disable doesn't propagate to merod; `[[project-ha-no-convergence-loop]]` — parent reconcile-loop gap |
| **Constrains** | Fleet sidecar reconcile loop (no issue yet); local soft-vs-hard cleanup choice for evicted TEE members |

> Earlier drafts of this ADR scoped a full pull-based leave protocol (sidecar polls steady-state, fleet-leave path, KMS forget). That over-reached: research (see §Context) showed `MemberRemoved` *already* does cryptographic eviction via the existing key-rotation pipeline. The user-visible "I disabled HA but still see the fleet node" complaint is solved client-side by tauri-app issuing `remove_group_members` on disable — tracked separately as a tauri-app issue. What remains is the *hygiene* tail of that flow: stale sidecar cache, local data purge on the evicted node, and the broader sidecar-reconcile-loop story.

## Context

When the owner publishes `MemberRemoved` for a `ReadOnlyTee` member (the path that fires when the desktop client calls `remove_group_members` after a successful cloud `disable-ha`), the existing pipeline at `crates/context/src/group_store/group_governance_publisher.rs:230-245` already handles cryptographic eviction:

```rust
let key_rotation = if let Some(removed) = removed_member {
    if encrypting_group_id == self.group_id {
        let new_group_key: [u8; 32] = OsRng.gen();         // fresh random
        let _ = store_group_key(...);
        Some(build_key_rotation(
            ..., &new_group_key, signer_sk,
            Some(removed),                                  // EXCLUDE
        )?)
    } else { None }
} else { None };
```

Effects on the evicted fleet node:

- **Membership row removed** on the owner's merod (apply side, `group_store/mod.rs:1306` MemberRemoved arm).
- **Membership row removed** on the fleet node's merod when it receives the gossip event; entry added to `deny_list` to prevent state-delta replay-in (`mod.rs:1332`).
- **Forward secrecy on the namespace's new writes**: fleet node never receives a wrapped copy of the new key (`build_key_rotation` excludes it). Cannot decrypt anything written after eviction.
- **Historical encrypted blobs on the fleet's disk**: untouched. Fleet node's keyring still has the *old* group key, can still decrypt its locally-stored historical content.
- **Fleet sidecar's `STATE_FILE` cache** (`mero-tee` repo's fleet sidecar): unchanged — the sidecar polls mdma's `/should-join` which no longer returns this namespace, but the sidecar doesn't act on namespace-absence today. The cache says the sidecar joined a namespace it no longer needs.

So the open hygiene tail, narrowed to what's actually missing:

1. **Soft-vs-hard local cleanup on the evicted node.** The `architecture/membership-and-leave.html` doc labels this "left as a follow-up — current behavior is 'soft': no purge, membership rows removed but encrypted blobs and keys remain on the local node." For an honest fleet operator who lost authorization, the residual blob can't decrypt new writes anyway. For a compromised TEE, the threat model already assumed the operator could read what was sent. So this is hygiene, not a forward-secrecy defect.
2. **Fleet sidecar reconcile loop.** The sidecar's `STATE_FILE` going stale is a cosmetic-but-real bug: the sidecar may waste cycles or surface confusing status for namespaces it's no longer in. The fix is small (diff steady-state against the cache, drop entries that disappeared) but composes with the broader `[[project-ha-no-convergence-loop]]` rework.
3. **KMS forgetting.** Whether the KMS in `mero-tee` should release attestation-bound key material on eviction is a mero-tee-team decision. Independent of core; included here as a cross-team flag.

Concrete prod evidence (2026-06-01): on a test namespace (id redacted), cloud HA disabled, owner-side eviction NOT yet wired in tauri. Owner's `list_group_members` returns the fleet peer as `ReadOnlyTee`. Once tauri lands the `remove_group_members` call, the membership row will go away cleanly; everything in this ADR is what to do AFTER that landing.

## Decision

**Treat eviction hygiene as three independent follow-ups, each scoped narrowly, none blocking the user-visible feature.**

### (a) Soft-vs-hard local cleanup — defer with explicit rationale

Current "soft leave" (no purge) is the correct default for now:

- Cryptographic forward secrecy already achieved (rotation pipeline).
- Historical blob the fleet retains is decryptable *only* with the key the fleet always had — eviction doesn't change that capability, so purging is a hygiene win, not a security one.
- Hard purge requires careful sequencing (don't delete a blob mid-sync that another peer is requesting from us, etc.) — design work disproportionate to the security delta.

Revisit if/when:
- A compliance promise (GDPR-style "right to forget on revocation") makes purge a contractual requirement.
- A TEE operator class emerges that cannot be trusted with retained blobs even cryptographically; current threat model assumes any fleet TEE got what it got.

### (b) Fleet sidecar reconcile — small, in mero-tee

Sidecar logic today: on each `/should-join` poll, compute `should_join - STATE_FILE` (new joins, action) and treat `STATE_FILE - should_join` as no-op. Change the latter half:

- Treat `STATE_FILE - should_join` as **stale**: drop those entries from the cache.
- Optionally: log `info!("namespace {} dropped from should-join, releasing local cache entry")` so operators can correlate.

No protocol changes, no merod changes. Sidecar-internal, two-file diff in `mero-tee`. Counts as a 30-line PR.

This does NOT include having the sidecar call any merod-level leave operation — because the owner's tauri-driven `remove_group_members` already triggered the cryptographic eviction. The sidecar just needs to stop holding stale references.

### (c) KMS forget — cross-team flag

When eviction happens, the fleet node's KMS could:

- **Today**: nothing. The key material the KMS issued for the namespace remains usable; the fleet node's TEE can decrypt anything in its local store with it.
- **Eventually**: on receiving notice that membership was removed (e.g. via the fleet sidecar reading `should-join` and observing the drop), the KMS could invalidate the attestation-bound key it held for this namespace.

This requires:
- A KMS-side API for namespace-key invalidation.
- A trigger path from the fleet sidecar to the KMS.
- A clear threat model: what does "the KMS forgot the key" defend against, given that the fleet TEE could have copied the plaintext anywhere in the time it was admitted?

Filed here as a cross-team flag; not a core decision. Owned by mero-tee team.

## Alternatives considered

### Full pull-based leave protocol (the original ADR draft)

The previous version of this ADR proposed redefining `/should-join`'s semantics to "namespaces I should currently be in" (set state, not delta), adding a `fleet_leave` path in core mirroring `fleet_join`, and wiring the sidecar to issue `meroctl tee fleet-leave` on observed absence.

**Rejected** because:
- The `meroctl tee fleet-leave` call would have invoked `leave_group` (self-leave) on the fleet's merod. But `leave_group`'s key rotation is the deferred two-phase design (per the leave_group docstring, `crates/context/src/handlers/leave_group.rs:10-13`). Eviction-from-the-fleet-side would have *worse* forward-secrecy guarantees than the already-working `MemberRemoved`-from-owner path.
- The owner's `remove_group_members` already publishes `MemberRemoved` with the working key-rotation pipeline; the fleet sidecar issuing a separate leave op would be redundant or actively wrong (two ops, one rotation each, on the same eviction).
- The "non-tauri owners" justification for pull was speculative; today the cloud disable signal originates in tauri, which is always online at the click.

### Cloud-to-fleet push notification

mdma sends a webhook to fleet nodes on disable. **Rejected**: introduces server surface on fleet sidecars, addressability requirement on mdma, and doesn't solve anything the tauri-driven `remove_group_members` doesn't already solve.

### Gossip-only (owner publishes, fleet's merod handles)

Owner publishes `MemberRemoved` via gossip, fleet sidecar listens to merod events for own-removal. **Rejected**: the publish *already happens* (that's what `MemberRemoved` is); the part that's missing is the sidecar reacting to the local apply (subscribing to merod's event stream). That's the same problem (b) above solves more directly via the polling diff — no event-stream subscription needed.

## Consequences

### What we lock in

- The user-visible HA-disable flow stays client-driven: tauri-app's responsibility to call `remove_group_members` on the owner's local merod after a successful cloud disable. Tracked as a separate tauri-app issue, NOT a core change.
- Soft-leave remains the default for evicted TEE members. The architecture doc's "soft vs hard local cleanup" decision is recorded here as *intentionally soft for now*, with the revisit triggers above.
- Fleet sidecar gets a small reconcile change (drop stale `STATE_FILE` entries on `should-join` absence). Filed against `mero-tee`.
- KMS forget remains an open mero-tee-team conversation, not a core decision.

### What we deliberately DON'T do

- Add a `fleet_leave` path in core. The owner-side `remove_group_members` is the correct eviction primitive; adding a fleet-initiated leave would be redundant or worse.
- Redefine `/should-join`'s semantics. The sidecar reconcile fits within today's response shape.
- Add a hard-purge code path on the evicted node. Not blocking the feature; revisit on triggers above.

### Failure modes we accept

- **Historical-blob retention on evicted fleet TEE.** Acknowledged in §Decision(a); no forward-secrecy delta.
- **Stale sidecar cache for ~1 poll cycle.** Bounded by `should-join` poll interval (1s).
- **KMS holding unused key material**. Until KMS-side forget lands. Acceptable given the threat model.

## Open questions

1. **KMS forget semantics** — owned by mero-tee team.
2. **Hard-purge code path on evicted node** — when does the trigger fire (compliance, threat model evolution)?
3. **Sidecar reconcile composition with the broader `[[project-ha-no-convergence-loop]]` rework** — small fix here, full reconcile loop is the parent gap. Worth a separate ADR or design note when that work is picked up.

## Implementation sketch

Minimal:

- **tauri-app** (small): on successful `POST /disable-ha`, immediately call `POST /admin-api/dev/groups/{ns}/members/remove` against the local merod with the `ReadOnlyTee` identity. Idempotent retry on failure. — *Tracked as separate tauri-app issue, not in this ADR's scope but referenced here for completeness.*
- **mero-tee fleet sidecar** (small): in the should-join polling loop, after computing new joins from `should_join - STATE_FILE`, also compute stale entries from `STATE_FILE - should_join` and drop them from the cache. Two-file diff.
- **mero-tee KMS** (open, cross-team): TBD.
- **core** (this ADR's scope): NO CHANGE. The eviction primitive is already correct.

## Related

- `[[project-ha-no-convergence-loop]]` — parent architectural gap (sidecar has no reconcile loop in general; this ADR's (b) is one slice).
- `[[project-ha-fleet-join-working-recipe]]` — sister recipe; the leave hygiene must compose without regressing join.
- mdma#155 — issue that surfaced the gap.
- mdma#149 — sibling cleanup gap (cloud-side `HaContextStatus` drift).
- `architecture/membership-and-leave.html` §5, §6 — soft-vs-hard local cleanup framing.
- `crates/context/src/group_store/group_governance_publisher.rs:230-245` — the existing `MemberRemoved` key-rotation pipeline this ADR relies on.
