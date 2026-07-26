# Namespace-join divergence: design

Date: 2026-07-26
Status: proposed
Scope: `crates/context`, `crates/governance-types`, `crates/governance-store`, `crates/node`, `crates/network`, `apps/scaffolding-e2e`

## Problem

A node that joins a namespace while its governance broadcast cannot reach existing members becomes **permanently invisible to the group**.
The joiner records itself as a member locally and believes the join succeeded.
Existing members never learn about it.
No code path in the current tree repairs the split.

This is not a slow-convergence window.
It is terminal for the lifetime of the namespace, unless something unrelated happens to force a fresh join.

The condition is reached whenever the joiner's `MemberJoinedAt` broadcast fails to reach existing members, including:

- an existing member is down or restarting at join time (the originally reported case),
- a network partition between joiner and members,
- the joiner's gossip mesh is too thin at the moment it publishes,
- packet loss at the wrong instant.

The E2E scenario `group-join-mesh-not-ready` reproduces this at roughly a 50% per-attempt rate.
It is currently treated as a flake and retried until green.

## Evidence

Two failing CI runs were analysed from node logs, alongside two passing runs of the same scenario.

The assertion that fails is `wait_for_sync` on `groupStateHash`, which is
`SHA256(group_id ‖ admin_identity ‖ owner_identity ‖ target_application_id ‖ sorted(member, role)*)`
(`crates/governance-store/src/meta.rs:159-182`).
No DAG heads enter the hash.
In this scenario the only divergent term is the member set: node-1 holds `{node-1}`, node-2 holds `{node-1, node-2}`.

In all four runs, passing and failing alike, the joiner's publish reports zero acknowledgements:

```
namespace governance op published op_kind="member_joined_at" acks=0 readiness="degraded"
```

The discriminator between pass and fail is whether node-1 ever receives that gossip frame:

| Run | `triggering proactive backfill` on node-1 | Result |
|---|---|---|
| 30156763733 | present | pass |
| 30088060099 | present | pass |
| 30157246398 | absent | fail |
| 30160568254 | absent | fail |

In the passing runs the frame arrives roughly 20 seconds after the partition heals, its parents are found missing, and the existing parent-pull path repairs the state.
In the failing runs it never arrives and nothing re-sends it.

Two distinct reasons the frame is lost were observed.

**Class A, zero peers** (run 30160568254).
The two nodes connected over QUIC only; the TCP dial failed its handshake.
QUIC's idle timeout closed the connection about 20 seconds into the partition, leaving `total_peers=0`.
No redial occurred for the remainder of the run.

**Class B, peer present, frame lost** (run 30157246398).
The TCP connection survived the partition and the mesh reported one peer throughout.
The frame was published into the blackholed path and never landed.
Nothing re-sent it.

## Root cause

Membership is durable state, but its only delivery mechanism is a best-effort, at-most-once broadcast with no outbox and no reconciliation.
Four recovery paths exist on paper.
All four are inoperative in this scenario.

**1. Re-broadcast when a peer returns: deleted.**
Commit `e75cbd62` (#3266) removed a bounded 120-second background re-publisher that polled for mesh readiness before publishing.
The removed code's own comment described this exact failure:

> The announce is otherwise fire-and-forget, so a gated-out publish silently strands us, current members never learn we joined, and when we later sync the authoritative group state (which excludes us) we reconcile our own locally-added membership away and are stuck "not a member".

The commit's primary change was correct.
It decoupled the local apply from transport readiness, fixing a separate 23% flake in migration-e2e where the local DAG head failed to advance.
The retry was removed in the same change, justified by the assumption that "the stored op reaches peers via sync even when the immediate publish acks are thin".
That assumption does not hold, for the reasons below.

**2. Parent-pull on receive: needs a received message.**
`parent_pull` fires when an incoming op references parents the receiver lacks.
node-1 receives no op at all, so there is nothing to trigger it.

**3. Namespace heartbeat: detects and ignores.**
`crates/node/src/handlers/network_event/namespace.rs:424-486` computes `we_missing` and `peer_missing`, then logs.
Phase 11.2 (#2237) deliberately removed the active catch-up arms.
Observed every 30 seconds in the failing run:

```
namespace heartbeat: divergence detected (liveness-only - recovery via publish_and_await_ack / parent_pull / readiness beacon)
```

**4. Readiness beacon: deadlocked.**
`verify_readiness_beacon` (`crates/governance-store/src/governance_broadcast.rs:193-201`) drops any beacon whose signer holds no membership row.
That check is correct and deliberate: it prevents unverified peers from populating other nodes' `ReadinessCache`.
But node-1 holds no membership row for node-2 precisely because the `MemberJoinedAt` op was lost.
Observed 24 consecutive drops in the failing run.

The beacon is the only trigger for a governance pull on an established member.
`sync_namespace` has exactly three callers: join, boot, and the beacon handler.
There is no periodic namespace governance sync.
`sync_namespace_from_peer` is pull-only and never offers local heads, so the joiner cannot push its membership upward either.

The result is a circular dependency.
To be told that a peer joined, a node must already believe that peer joined.

## Non-goals

- Reworking gossipsub delivery semantics.
- Adding a general-purpose reliable-delivery layer for all governance ops.
- Making namespace sync bidirectional. This is a genuine structural gap and worth doing, but it is a wire-protocol change with a wider blast radius and is deferred.
- Restoring active catch-up arms to the namespace heartbeat. Deferred for the same reason.
- Changing the E2E scenario so that it converges with help. See Change 4.

## Design

Four changes.
Three are product fixes; the fourth is test-harness honesty.

### Change 1: bounded re-publisher on zero acknowledgements

**File:** `crates/context/src/handlers/join_group.rs`

Keep the unconditional local apply introduced by #3266.
That part was correct and must not be reverted.

`sign_apply_and_publish_namespace_op` returns a `DeliveryReport { op_hash, acked_by, elapsed_ms, readiness }`
(`crates/governance-store/src/governance_broadcast.rs:279-290`).
When `acked_by` is empty, the broadcast reached nobody.
That is the retry trigger.

```rust
let report = sign_apply_and_publish_namespace_op(...).await?;   // local apply unchanged
if report.acked_by.is_empty() {
    schedule_member_joined_republish(report.op_hash, namespace_id, ...);
}
```

Three constraints on the retry.

**Re-broadcast the same signed op; do not re-sign.**
The op is already applied and stored locally with a fixed `op_hash`.
Load it back and republish via `publish_and_await_ack_namespace`, which accepts an already-signed `SignedNamespaceOp`.
This sidesteps the duplicate-nonce concern that forced the original implementation to break out after a single attempt.

**Re-arm on peer-subscribed, not on a fixed countdown.**
The original used a 120-second deadline measured from join time and polled every 500ms.
That races reconnection: in Class A the connection is not restored until the redial ladder fires, and a fixed countdown can expire first.

Drive the retry from the existing event instead.
`crates/node/src/handlers/network_event/subscriptions.rs:83-88` already handles a peer subscribing to a namespace topic and emits an out-of-cycle beacon at that point.
That is exactly the moment a previously unreachable member becomes reachable, and it is the correct trigger for the re-publish.

Bound the pending re-publish at 10 minutes as a backstop, so a genuinely partitioned joiner's registration does not linger indefinitely.
The cap can be generous because the task is event-driven and idle, not polling.
Ten minutes comfortably exceeds the 300-second mDNS re-discovery interval, which is the slowest realistic reconnection path.

**Stop on first acknowledgement.**
One ack means at least one member has applied the op, and normal gossip and parent-pull carry it from there.

### Change 2: beacon admission proof

**Files:** `crates/governance-types/src/wire.rs`, `crates/governance-store/src/governance_broadcast.rs`, `crates/node/src/handlers/network_event/readiness.rs`

Break the deadlock cryptographically rather than by relaxing trust.

An invitation is self-contained and signature-verifiable.
It carries the inviter's identity, and the signature verifies against it.
A joiner can therefore prove admission without the group having applied its `MemberJoinedAt` op.

All three required primitives already exist:

| Requirement | Existing implementation |
|---|---|
| Verify invitation signature | `verify_open_invitation_signature`, `crates/governance-store/src/namespace/membership.rs:216` |
| Check inviter held invite rights | `require_inviter_permission` (`CAN_INVITE_MEMBERS`), same file, line 234 |
| Replay protection by nonce | `ReentryRepository::is_invitation_consumed` / `mark_invitation_consumed`, `crates/governance-store/src/reentry.rs:152,167` |

**Wire change.**
Extend the existing struct rather than adding an enum variant.
Keep `signature` as the final field.

```rust
// crates/governance-types/src/wire.rs:118
pub struct SignedReadinessBeacon {
    pub namespace_id: NamespaceId,
    pub peer_pubkey: PublicKey,
    pub dag_head: [u8; 32],
    pub applied_through: u64,
    pub ts_millis: u64,
    pub strong: bool,
    pub admission_proof: Option<SignedGroupOpenInvitation>,   // new
    pub signature: [u8; 64],
}
```

`SignableReadinessBeacon` gains the same field so the signature covers it.
Without that, an attacker could staple a valid invitation lifted from elsewhere onto their own beacon.

`Option` keeps the steady-state cost at one byte.
The proof is attached only while the joiner's membership has not yet been reflected back to it.
It is cleared as soon as the joiner observes its own membership in a peer's state, or after 10 minutes, matching the re-publisher cap in Change 1.
Both bounds exist so that a joiner whose admission was genuinely rejected does not broadcast a proof indefinitely.

A sign-domain bump is not required.
A six-field and a seven-field signable cannot serialise identically, because `SignedGroupOpenInvitation` cannot encode to zero bytes.

**Receive-side algorithm.**

```
decode NamespaceTopicMsg -> ReadinessBeacon(b)
  1. verify b.signature over DOMAIN || borsh(signable)
  2. reject if b.ts_millis is stale
  3. if b.peer_pubkey is a known member -> accept, unchanged behaviour
  4. else if b.admission_proof is Some(inv):
       a. inv.invitation.namespace_id == b.namespace_id
       b. verify_open_invitation_signature(&inv)
       c. inviter holds CAN_INVITE_MEMBERS in our current state
       d. inv admits b.peer_pubkey
       e. nonce not consumed by a different identity; peer not blocked
       -> on success: schedule a debounced, authenticated sync_namespace(from = peer)
  5. else -> drop, unchanged behaviour
```

**Step 4 never writes a membership row.**
It only unlocks a pull.
Membership continues to arrive exclusively through a verified `MemberJoinedAt` op applied on the normal path, preserving a single source of truth.

### Change 3: remove the mDNS exclusion from the disconnect redial path

**File:** `crates/network/src/handlers/stream/swarm.rs:196-201`

The direct re-dial added by `c95fd1d8` (#3301) names this scenario in its own comment, but sits inside a branch that excludes mDNS-discovered peers:

```rust
} else if !self.discovery.state.is_peer_rendezvous(&peer_id)
    && !self.discovery.state
        .is_peer_discovered_via(&peer_id, PeerDiscoveryMechanism::Mdns)
{
    // redial_direct(...) plus rendezvous force-discover and re-fire ladder
```

In CI there is no rendezvous server and mDNS is the only discovery mechanism, so the fix never executes for the case it was written for.
Re-discovery then waits on `mdns::Config::default()`, which is a 300-second query interval and a 360-second TTL in `libp2p-mdns` 0.48.0, far beyond the scenario's 120-second budget.

Remove the mDNS predicate so a peer for which a confirmed direct address is held is re-dialled regardless of how it was discovered.
The rendezvous force-discover and its re-fire ladder remain gated as they are today.

### Change 4: test-harness honesty

**Files:** `apps/scaffolding-e2e/workflows/group-join-mesh-not-ready.yml`, `.github/actions/setup-merobox/action.yml`

The scenario's final step declares `trigger_sync: true`, and its comment claims the step drives recovery:

> trigger a namespace governance sync between the two nodes and block until they converge. That forces the redial + gossipsub mesh re-GRAFT

None of that happens.
merobox only issues a sync RPC when the target is a `context`; this scenario declares only `group_id`, so the step is a pure passive poll.

The passive behaviour is correct and must be preserved.
A real user does not trigger a sync by hand, so a convergence test must not either.
Making the trigger fire would let a manual nudge mask the product bug and turn CI green while users still hit permanent divergence.

Therefore:

- Delete the `trigger_sync: true` line. It is dead configuration that misdescribes the step and invites a future "fix" that would silently disable the regression guard.
- Rewrite the comment to state what the step actually is: wait up to 120 seconds for the two nodes to converge unaided.
- Pin merobox to an exact version. `merobox>=0.6.43` with no upper bound allows CI behaviour to change without a commit, which is an independent source of flakiness.

No behavioural change to the scenario.

## Failure-class coverage

| | Class A: zero peers | Class B: peer present, frame lost |
|---|---|---|
| Change 1, re-publisher | No. No peer exists to publish to. | **Yes.** Retries after heal, node-1 receives, parent-pull converges. |
| Change 2, beacon proof | No. Beacons reach nobody. | Yes, as an independent second net. |
| Change 3, mDNS redial | **Yes.** Restores the connection, after which 1 and 2 can act. | Not applicable; the connection never dropped. |

Changes 1 and 3 together are required for a green CI.
Change 2 is what prevents the bug being permanent when Change 1's window is exceeded, for example a partition longer than the cap or a restart mid-window.

## Security analysis

An earlier variant of Change 2 was considered and rejected: treating any unverifiable beacon as a hint to pull from that peer, protected only by a rate limit.
It was rejected because it introduces attacks that do not exist today.

- **Sybil-amplified denial of service.** Peer IDs are free to generate, so a per-peer rate limit is defeated by identity rotation. A cheap gossip message would cost the victim a stream open and a round trip.
- **Outbound connection coercion.** An attacker would choose when a node initiates connections and to whom.
- **Membership oracle.** Today an unverifiable beacon is dropped silently. A visible reaction only for namespaces the node belongs to converts the drop into a membership probe.
- **Evicted-member nuisance.** A removed member could force pulls indefinitely.

The invitation-proof design avoids all four.
The trigger is authenticated by a signature chain rooted in the namespace admin key, so there is no unauthenticated path to amplify.
An attacker without a valid invitation fails at step 4b and observes exactly today's silent drop, so no oracle is created.

Residual considerations, all addressed in Testing:

- The beacon signature must cover `admission_proof`.
- Stale beacons must be rejected on `ts_millis` to bound replay.
- The invitation's namespace must match the beacon's namespace.
- Pulls must be debounced per namespace.

## Compatibility and rollout

Change 2 alters the borsh layout of `SignedReadinessBeacon`.
`NamespaceTopicMsg` is decoded with `borsh::from_slice` (`crates/node/src/handlers/network_event/namespace.rs:36`), which rejects trailing bytes.

An un-upgraded node receiving a new-format beacon therefore fails closed, by either of two independent guards:

1. `from_slice` returns `Err` on the trailing bytes; the message is dropped and the error is scoped to that one inner payload.
2. Even if a parse succeeded, the new signature covers seven fields while an old node builds a six-field signable, so verification fails.

The consequence to accept: during a staggered rollout, an un-upgraded node drops **every** beacon from an upgraded node, not only proof-carrying ones, because the struct changed for all of them.
Beacon-driven liveness and divergence detection go dark between mismatched pairs until the rollout completes, then self-heal.

This is consistent with the existing policy stated at `crates/governance-types/src/wire.rs:343-345`, that adding to these wire types requires a coordinated cluster upgrade pre-1.0.
It must nonetheless be named explicitly in the PR description.

Changes 1, 3, and 4 carry no protocol impact.

## Testing

merobox validates a uniform network of identical builds against fresh state.
By construction it cannot exercise mixed-version interop, nor can it produce malformed or hostile input, because every node is an honest `merod` of the same build.
The tests below are split accordingly.

**Unit and integration**

1. A new-format beacon decoded against the old struct definition returns a clean `Err`, with no panic and no misparse. The old definition is declared locally in the test, since it will not exist in the tree after the change.
2. A checked-in byte fixture of a current-format beacon still decodes under the new code.
3. `admission_proof: None` round-trips, and the steady-state wire size grows by exactly one byte.
4. The signature covers `admission_proof`: flipping a byte inside the proof invalidates the beacon.
5. Negative admission cases, one test each: wrong namespace, inviter lacking `CAN_INVITE_MEMBERS`, consumed nonce, blocked peer, invitation admitting a different public key than the beacon signer.
6. A stale `ts_millis` is rejected.
7. The pull debounce bounds attempts per namespace.
8. The re-publisher fires only when `acked_by` is empty, stops on the first acknowledgement, and exits at its cap.
9. The re-publisher republishes the stored op by hash and does not sign a new one.

**End to end**

10. `group-join-mesh-not-ready` passes: partitioned join, heal, unaided convergence within the 120-second budget. This is the headline regression test and the one currently failing.

## Sequencing

| PR | Contents | Rationale |
|---|---|---|
| 1 | Change 1 and Change 3 | Turns CI green. No protocol change. Small and independently reviewable. |
| 2 | Change 2 | Closes the permanent-divergence hole. Carries the borsh migration tests and the rollout note. |
| 3 | Change 4 | Harness honesty. Independent of the others. |

PR 1 lands first.
It has the largest immediate effect for the smallest diff and does not block on the wire-format discussion.

## Verification plan

Before claiming the flake is fixed, measure it.

1. Baseline: run `group-join-mesh-not-ready` 20 times on master, record the pass rate.
2. Run the same 20 iterations on the fix branch.
3. Compare using the known predictor: node-1's log contains `triggering proactive backfill` on a converged run.

Include both numbers in the PR description, per the repository's definition of done.

## Residual risk

Two failing runs were characterised in depth.
The observed 50% per-attempt rate is consistent with the two classes described, but a third tail cannot be ruled out from a sample of two.
The verification plan above is what closes that gap; a 20-iteration run that is not clean means a class remains uncharacterised, and the investigation should reopen rather than the retry count being raised.
